//! Bounds-checked byte reader and ISO base media file format box iteration
//! (ISO/IEC 14496-12, clause 4.2 "Object structure").

use crate::error::{Error, Result};

pub(crate) type FourCc = [u8; 4];

/// Upper bound on boxes visited inside any single container, so a hostile file cannot make
/// the parser loop or allocate proportionally to its size in tiny boxes.
pub(crate) const MAX_BOXES_PER_CONTAINER: usize = 1 << 17;

const TRUNCATED: Error = Error::Malformed("truncated data");

/// Big-endian cursor over a byte slice. Every read is bounds-checked.
#[derive(Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub fn remaining(&self) -> usize {
        self.data.len()
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.data.len() {
            return Err(TRUNCATED);
        }
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        Ok(head)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Big-endian unsigned integer of `n` bytes (0..=8); zero bytes read as 0.
    pub fn uint(&mut self, n: usize) -> Result<u64> {
        if n > 8 {
            return Err(Error::Malformed("integer field wider than 8 bytes"));
        }
        let b = self.take(n)?;
        Ok(b.iter().fold(0u64, |acc, &x| (acc << 8) | u64::from(x)))
    }

    pub fn fourcc(&mut self) -> Result<FourCc> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }

    /// NUL-terminated string (terminator consumed, not returned).
    pub fn cstr(&mut self) -> Result<&'a [u8]> {
        let pos = self.data.iter().position(|&b| b == 0).ok_or(TRUNCATED)?;
        let s = self.take(pos)?;
        self.skip(1)?;
        Ok(s)
    }

    /// Everything not yet consumed.
    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.data)
    }

    /// FullBox header: `(version, flags)`.
    pub fn full_box(&mut self) -> Result<(u8, u32)> {
        let v = self.u8()?;
        let f = self.uint(3)? as u32;
        Ok((v, f))
    }
}

/// One box: its four-character type and its payload (header stripped).
#[derive(Clone, Copy, Debug)]
pub(crate) struct BmffBox<'a> {
    pub kind: FourCc,
    pub body: &'a [u8],
}

/// Iterator over the boxes laid end to end in `data`.
///
/// Yields an error (then ends) if a box header is truncated or its size is impossible.
/// Fewer than 8 trailing bytes are treated as padding.
pub(crate) struct Boxes<'a> {
    data: &'a [u8],
    seen: usize,
    failed: bool,
}

pub(crate) fn boxes(data: &[u8]) -> Boxes<'_> {
    Boxes {
        data,
        seen: 0,
        failed: false,
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = Result<BmffBox<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.data.len() < 8 {
            return None;
        }
        self.seen += 1;
        if self.seen > MAX_BOXES_PER_CONTAINER {
            self.failed = true;
            return Some(Err(Error::Malformed("too many boxes")));
        }
        let mut r = Reader::new(self.data);
        let parsed = (|| {
            let size32 = r.u32()?;
            let kind = r.fourcc()?;
            let (header_len, size) = match size32 {
                1 => (16usize, r.u64()?),
                0 => (8usize, self.data.len() as u64),
                n => (8usize, u64::from(n)),
            };
            if size < header_len as u64 || size > self.data.len() as u64 {
                return Err(Error::Malformed("box size outside its container"));
            }
            Ok((kind, header_len, size as usize))
        })();
        match parsed {
            Ok((kind, header_len, size)) => {
                let body = &self.data[header_len..size];
                self.data = &self.data[size..];
                Some(Ok(BmffBox { kind, body }))
            }
            Err(e) => {
                self.failed = true;
                Some(Err(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_reads_big_endian_and_bounds_checks() {
        let mut r = Reader::new(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16().unwrap(), 0x0203);
        assert_eq!(r.uint(3).unwrap(), 0x040506);
        assert_eq!(r.uint(0).unwrap(), 0);
        assert_eq!(r.remaining(), 3);
        assert!(r.u32().is_err());
        assert_eq!(r.remaining(), 3, "a failed read must not consume");
        assert_eq!(r.rest(), &[7, 8, 9]);
        assert!(r.u8().is_err());
        assert!(Reader::new(&[0; 16]).uint(9).is_err());
    }

    #[test]
    fn reader_cstr() {
        let mut r = Reader::new(b"abc\0def");
        assert_eq!(r.cstr().unwrap(), b"abc");
        assert!(r.cstr().is_err());
    }

    #[test]
    fn box_iteration_handles_all_size_forms() {
        let mut data = Vec::new();
        data.extend_from_slice(&12u32.to_be_bytes());
        data.extend_from_slice(b"abcd");
        data.extend_from_slice(&[1, 2, 3, 4]);
        // 64-bit largesize
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(b"wide");
        data.extend_from_slice(&18u64.to_be_bytes());
        data.extend_from_slice(&[9, 9]);
        // size 0: to the end
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(b"last");
        data.extend_from_slice(&[7; 5]);
        let v: Vec<_> = boxes(&data).collect::<Result<_>>().unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(&v[0].kind, b"abcd");
        assert_eq!(v[0].body, &[1, 2, 3, 4]);
        assert_eq!(&v[1].kind, b"wide");
        assert_eq!(v[1].body, &[9, 9]);
        assert_eq!(&v[2].kind, b"last");
        assert_eq!(v[2].body, &[7; 5]);
    }

    #[test]
    fn box_iteration_rejects_bad_sizes() {
        // size larger than the container
        let mut data = 100u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"abcdxxxx");
        assert!(boxes(&data).next().unwrap().is_err());
        // size smaller than the header
        let mut data = 4u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"abcd");
        assert!(boxes(&data).next().unwrap().is_err());
        // largesize smaller than its own header
        let mut data = 1u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"abcd");
        data.extend_from_slice(&8u64.to_be_bytes());
        assert!(boxes(&data).next().unwrap().is_err());
        // the iterator stops after an error
        let mut it = boxes(&data);
        assert!(it.next().is_some());
        assert!(it.next().is_none());
    }

    #[test]
    fn trailing_padding_is_ignored() {
        let mut data = 8u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"free");
        data.extend_from_slice(&[0; 5]);
        assert_eq!(boxes(&data).count(), 1);
    }
}
