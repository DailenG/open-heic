//! Item property boxes found in `ipco` (ISO/IEC 23008-12, clause 6.5 and the codec
//! configuration boxes of ISO/IEC 14496-15, clause 8.3.3 for HEVC).

use crate::error::{Error, Result};
use crate::reader::{BmffBox, Reader};

/// `urn:mpeg:mpegB:cicp:systems:auxiliary:alpha` (MPEG-B alpha plane).
const ALPHA_URN_MPEGB: &[u8] = b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha";
/// `urn:mpeg:hevc:2015:auxid:1` (alpha plane of an HEVC auxiliary image, ISO/IEC 23008-2 Annex F).
const ALPHA_URN_HEVC: &[u8] = b"urn:mpeg:hevc:2015:auxid:1";

/// `colr` payload with colour type `nclx`: CICP code points (ITU-T H.273).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Nclx {
    #[allow(dead_code)] // parsed for completeness; no colour management is applied
    pub primaries: u16,
    #[allow(dead_code)]
    pub transfer: u16,
    pub matrix: u16,
    pub full_range: bool,
}

/// `colr` box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Colr {
    Nclx(Nclx),
    /// An ICC profile is present (not applied).
    Icc,
}

/// `clap` clean aperture: width/height/offsets as fractions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Clap {
    pub width: (u32, u32),
    pub height: (u32, u32),
    pub horiz_off: (i32, u32),
    pub vert_off: (i32, u32),
}

impl Clap {
    /// The crop rectangle `(left, top, width, height)` inside an image of `w` x `h`.
    ///
    /// ISO/IEC 14496-12 12.1.4.1: the aperture centre is `(w-1)/2 + horizOff` and its width
    /// is `cleanApertureWidth`, so `left = (w - cw)/2 + horizOff` (same vertically). The
    /// fractions are evaluated exactly, then rounded down.
    pub fn rect(&self, w: u32, h: u32) -> Result<(u32, u32, u32, u32)> {
        let axis = |size: u32, clean: (u32, u32), off: (i32, u32)| -> Result<(u32, u32)> {
            if clean.1 == 0 || off.1 == 0 {
                return Err(Error::Malformed("clap fraction with zero denominator"));
            }
            let (cn, cd) = (i128::from(clean.0), i128::from(clean.1));
            let (on, od) = (i128::from(off.0), i128::from(off.1));
            let len = cn / cd;
            // left = (size - cn/cd)/2 + on/od, over the common denominator 2*cd*od.
            let num = (i128::from(size) * cd - cn) * od + 2 * cd * on;
            let left = num.div_euclid(2 * cd * od);
            if len < 1 || left < 0 || left + len > i128::from(size) {
                return Err(Error::Malformed("clap aperture outside the image"));
            }
            Ok((left as u32, len as u32))
        };
        let (l, cw) = axis(w, self.width, self.horiz_off)?;
        let (t, ch) = axis(h, self.height, self.vert_off)?;
        Ok((l, t, cw, ch))
    }
}

/// HEVCDecoderConfigurationRecord (`hvcC`) header fields; the NAL arrays are kept raw.
#[derive(Clone, Debug)]
pub(crate) struct Hvcc {
    pub length_size: usize,
    pub bit_depth_luma: u8,
    /// Starts at `numOfArrays`.
    pub arrays: Vec<u8>,
}

impl Hvcc {
    fn parse(body: &[u8]) -> Result<Self> {
        if body.len() < 23 {
            return Err(Error::Malformed("hvcC too short"));
        }
        Ok(Hvcc {
            bit_depth_luma: (body[17] & 7) + 8,
            length_size: usize::from(body[21] & 3) + 1,
            arrays: body[22..].to_vec(),
        })
    }
}

/// A parsed `ipco` child. Properties this crate does not understand are kept as
/// [`Property::Unknown`] so that `ipma` indices stay valid.
#[derive(Clone, Debug)]
pub(crate) enum Property {
    Hvcc(Hvcc),
    Ispe {
        width: u32,
        height: u32,
    },
    Irot(u8),
    Imir(u8),
    Clap(Clap),
    Colr(Colr),
    Pixi {
        first_depth: u8,
    },
    AuxC {
        alpha: bool,
    },
    Unknown,
    /// A known property whose payload does not parse.
    Invalid,
}

pub(crate) fn parse_property(b: BmffBox<'_>) -> Property {
    parse_known(b).unwrap_or(Property::Invalid)
}

fn parse_known(b: BmffBox<'_>) -> Result<Property> {
    let mut r = Reader::new(b.body);
    Ok(match &b.kind {
        b"hvcC" => Property::Hvcc(Hvcc::parse(b.body)?),
        b"ispe" => {
            r.full_box()?;
            Property::Ispe {
                width: r.u32()?,
                height: r.u32()?,
            }
        }
        b"irot" => Property::Irot(r.u8()? & 3),
        b"imir" => Property::Imir(r.u8()? & 1),
        b"clap" => Property::Clap(Clap {
            width: (r.u32()?, r.u32()?),
            height: (r.u32()?, r.u32()?),
            horiz_off: (r.i32()?, r.u32()?),
            vert_off: (r.i32()?, r.u32()?),
        }),
        b"colr" => match &r.fourcc()? {
            b"nclx" => {
                let primaries = r.u16()?;
                let transfer = r.u16()?;
                let matrix = r.u16()?;
                let full_range = r.u8()? & 0x80 != 0;
                Property::Colr(Colr::Nclx(Nclx {
                    primaries,
                    transfer,
                    matrix,
                    full_range,
                }))
            }
            b"rICC" | b"prof" => Property::Colr(Colr::Icc),
            _ => Property::Unknown,
        },
        b"pixi" => {
            r.full_box()?;
            let channels = r.u8()?;
            let first_depth = if channels == 0 { 0 } else { r.u8()? };
            Property::Pixi { first_depth }
        }
        b"auxC" => {
            r.full_box()?;
            let urn = r.cstr()?;
            Property::AuxC {
                alpha: urn == ALPHA_URN_MPEGB || urn == ALPHA_URN_HEVC,
            }
        }
        _ => Property::Unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((8 + body.len()) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn parse(kind: &[u8; 4], body: &[u8]) -> Property {
        let data = bx(kind, body);
        let b = crate::reader::boxes(&data).next().unwrap().unwrap();
        parse_property(b)
    }

    #[test]
    fn ispe() {
        let mut body = vec![0; 4];
        body.extend_from_slice(&4032u32.to_be_bytes());
        body.extend_from_slice(&3024u32.to_be_bytes());
        assert!(matches!(
            parse(b"ispe", &body),
            Property::Ispe {
                width: 4032,
                height: 3024
            }
        ));
        assert!(matches!(parse(b"ispe", &body[..9]), Property::Invalid));
    }

    #[test]
    fn irot_and_imir_use_the_low_bits() {
        assert!(matches!(parse(b"irot", &[0xFF]), Property::Irot(3)));
        assert!(matches!(parse(b"imir", &[0xFE]), Property::Imir(0)));
        assert!(matches!(parse(b"imir", &[0x01]), Property::Imir(1)));
        assert!(matches!(parse(b"irot", &[]), Property::Invalid));
    }

    #[test]
    fn colr_nclx_and_icc() {
        let mut body = b"nclx".to_vec();
        body.extend_from_slice(&[0, 12, 0, 13, 0, 6, 0x80]);
        match parse(b"colr", &body) {
            Property::Colr(Colr::Nclx(n)) => {
                assert_eq!(
                    (n.primaries, n.transfer, n.matrix, n.full_range),
                    (12, 13, 6, true)
                );
            }
            p => panic!("{p:?}"),
        }
        body[10] = 0; // matrix low byte stays; flip range bit off
        let last = body.len() - 1;
        body[last] = 0;
        assert!(matches!(
            parse(b"colr", &body),
            Property::Colr(Colr::Nclx(Nclx {
                full_range: false,
                ..
            }))
        ));
        let mut icc = b"prof".to_vec();
        icc.extend_from_slice(&[1, 2, 3]);
        assert!(matches!(parse(b"colr", &icc), Property::Colr(Colr::Icc)));
        assert!(matches!(parse(b"colr", b"nclx\0"), Property::Invalid));
    }

    #[test]
    fn pixi_and_auxc() {
        assert!(matches!(
            parse(b"pixi", &[0, 0, 0, 0, 3, 8, 8, 8]),
            Property::Pixi { first_depth: 8 }
        ));
        let mut body = vec![0; 4];
        body.extend_from_slice(ALPHA_URN_MPEGB);
        body.push(0);
        assert!(matches!(
            parse(b"auxC", &body),
            Property::AuxC { alpha: true }
        ));
        let mut body = vec![0; 4];
        body.extend_from_slice(ALPHA_URN_HEVC);
        body.push(0);
        assert!(matches!(
            parse(b"auxC", &body),
            Property::AuxC { alpha: true }
        ));
        let mut body = vec![0; 4];
        body.extend_from_slice(b"urn:mpeg:hevc:2015:auxid:2\0");
        assert!(matches!(
            parse(b"auxC", &body),
            Property::AuxC { alpha: false }
        ));
        assert!(matches!(
            parse(b"auxC", &[0, 0, 0, 0, b'a']),
            Property::Invalid
        ));
    }

    #[test]
    fn hvcc_header() {
        let mut body = vec![0u8; 23];
        body[0] = 1;
        body[16] = 0xFD; // chroma_format_idc 1
        body[17] = 0xFA; // bit depth luma 10
        body[21] = 0x03;
        match parse(b"hvcC", &body) {
            Property::Hvcc(h) => {
                assert_eq!((h.bit_depth_luma, h.length_size), (10, 4));
                assert_eq!(h.arrays, vec![0]);
            }
            p => panic!("{p:?}"),
        }
        assert!(matches!(parse(b"hvcC", &body[..22]), Property::Invalid));
    }

    #[test]
    fn unknown_property_is_kept() {
        assert!(matches!(parse(b"zzzz", &[1, 2]), Property::Unknown));
    }

    fn clap(w: (u32, u32), h: (u32, u32), x: (i32, u32), y: (i32, u32)) -> Clap {
        Clap {
            width: w,
            height: h,
            horiz_off: x,
            vert_off: y,
        }
    }

    #[test]
    fn clap_centered_and_offset() {
        // centred 100x50 aperture in 160x120: left = 30, top = 35
        let c = clap((100, 1), (50, 1), (0, 1), (0, 1));
        assert_eq!(c.rect(160, 120).unwrap(), (30, 35, 100, 50));
        // shifted left by 30 and down by 5
        let c = clap((100, 1), (50, 1), (-30, 1), (5, 1));
        assert_eq!(c.rect(160, 120).unwrap(), (0, 40, 100, 50));
        // fractional offset: -0.5 = -1/2 with odd slack
        let c = clap((159, 1), (120, 1), (-1, 2), (0, 1));
        assert_eq!(c.rect(160, 120).unwrap(), (0, 0, 159, 120));
        // fractional width: 100/3 -> 33
        let c = clap((100, 3), (120, 1), (0, 1), (0, 1));
        assert_eq!(c.rect(160, 120).unwrap().2, 33);
    }

    #[test]
    fn clap_rejects_bad_apertures() {
        let ok = clap((10, 1), (10, 1), (0, 1), (0, 1));
        assert!(ok.rect(10, 10).is_ok());
        assert!(ok.rect(9, 10).is_err()); // larger than image
        assert!(clap((10, 0), (10, 1), (0, 1), (0, 1)).rect(10, 10).is_err());
        assert!(clap((10, 1), (10, 1), (0, 0), (0, 1)).rect(10, 10).is_err());
        assert!(clap((0, 1), (10, 1), (0, 1), (0, 1)).rect(10, 10).is_err());
        assert!(clap((10, 1), (10, 1), (1, 1), (0, 1)).rect(10, 10).is_err()); // pushed out
        assert!(
            clap((u32::MAX, 1), (u32::MAX, 1), (i32::MIN, 1), (i32::MAX, 1))
                .rect(10, 10)
                .is_err()
        );
    }
}
