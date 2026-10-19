//! HEVC glue: turn an `hvcC` + length-prefixed item payload into an Annex B stream, vet the
//! sequence parameter sets before the decoder sees them, and run `rust_h265` with panics
//! contained.
//!
//! References: ISO/IEC 14496-15 clause 8.3.3 (HEVCDecoderConfigurationRecord), ITU-T H.265
//! Annex B (byte stream) and clauses 7.3.2.2 / 7.3.3 (SPS and profile_tier_level syntax).

use std::panic::{AssertUnwindSafe, catch_unwind};

use rust_h265::{DecodeError, Decoder, Frame, NalUnitType, parse_annex_b};

use crate::error::{Error, Result};
use crate::props::Hvcc;
use crate::reader::Reader;

/// Message of the [`Error::Unsupported`] returned for 4:0:0 streams (HEIC alpha planes
/// written by Apple are monochrome). The caller treats it as "no alpha available".
pub(crate) const MONOCHROME: &str = "monochrome (4:0:0) HEVC";

/// The fields of a sequence parameter set needed to vet an image before decoding it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpsInfo {
    /// Output width after the conformance window, in luma samples.
    pub width: u32,
    /// Output height after the conformance window, in luma samples.
    pub height: u32,
    pub chroma_format_idc: u32,
    pub bit_depth: u8,
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bit(&mut self) -> Result<u32> {
        let byte = self
            .data
            .get(self.pos / 8)
            .ok_or(Error::Malformed("truncated SPS"))?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(u32::from(bit))
    }

    fn bits(&mut self, n: u32) -> Result<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len() * 8);
        self.pos = end.ok_or(Error::Malformed("truncated SPS"))?;
        Ok(())
    }

    /// Exp-Golomb `ue(v)` (H.265 clause 9.2).
    fn ue(&mut self) -> Result<u64> {
        let mut zeros = 0u32;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 32 {
                return Err(Error::Malformed("invalid Exp-Golomb code in SPS"));
            }
        }
        let suffix = u64::from(self.bits(zeros)?);
        Ok((1u64 << zeros) - 1 + suffix)
    }
}

/// Parse the leading part of an SPS RBSP (emulation prevention already removed).
pub(crate) fn parse_sps(rbsp: &[u8]) -> Result<SpsInfo> {
    let mut b = Bits::new(rbsp);
    b.skip(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = b.bits(3)?;
    if max_sub_layers_minus1 > 6 {
        return Err(Error::Malformed("SPS sub-layer count out of range"));
    }
    b.skip(1)?; // sps_temporal_id_nesting_flag
    // profile_tier_level(1, max_sub_layers_minus1): general part is 2+1+5+32+48+8 bits.
    b.skip(96)?;
    let mut profile_present = [false; 7];
    let mut level_present = [false; 7];
    for i in 0..max_sub_layers_minus1 as usize {
        profile_present[i] = b.bit()? == 1;
        level_present[i] = b.bit()? == 1;
    }
    if max_sub_layers_minus1 > 0 {
        b.skip(2 * (8 - max_sub_layers_minus1 as usize))?; // reserved_zero_2bits
    }
    for i in 0..max_sub_layers_minus1 as usize {
        if profile_present[i] {
            b.skip(88)?;
        }
        if level_present[i] {
            b.skip(8)?;
        }
    }
    b.ue()?; // sps_seq_parameter_set_id
    let chroma_format_idc = b.ue()?;
    if chroma_format_idc > 3 {
        return Err(Error::Malformed("invalid chroma_format_idc"));
    }
    if chroma_format_idc == 3 {
        b.skip(1)?; // separate_colour_plane_flag
    }
    let width = b.ue()?;
    let height = b.ue()?;
    let (mut left, mut right, mut top, mut bottom) = (0u64, 0u64, 0u64, 0u64);
    if b.bit()? == 1 {
        left = b.ue()?;
        right = b.ue()?;
        top = b.ue()?;
        bottom = b.ue()?;
    }
    let bit_depth_luma = b.ue()?;
    let bit_depth_chroma = b.ue()?;
    if bit_depth_luma > 8 || bit_depth_chroma > 8 {
        return Err(Error::Unsupported("HEVC bit depth above 16"));
    }
    if bit_depth_luma != bit_depth_chroma {
        return Err(Error::Unsupported("HEVC luma and chroma bit depths differ"));
    }
    // Conformance window offsets are in chroma sample units (H.265 clause 7.4.3.2.1).
    let (sub_w, sub_h) = match chroma_format_idc {
        1 => (2, 2),
        2 => (2, 1),
        _ => (1, 1),
    };
    let crop = |size: u64, a: u64, b: u64, unit: u64| -> Result<u32> {
        let cut = a.saturating_add(b).saturating_mul(unit);
        let out = size
            .checked_sub(cut)
            .filter(|&o| o > 0)
            .ok_or(Error::Malformed("empty SPS picture"))?;
        u32::try_from(out).map_err(|_| Error::Malformed("SPS picture too large"))
    };
    Ok(SpsInfo {
        width: crop(width, left, right, sub_w)?,
        height: crop(height, top, bottom, sub_h)?,
        chroma_format_idc: chroma_format_idc as u32,
        bit_depth: 8 + bit_depth_luma as u8,
    })
}

/// Split the NAL arrays of an HEVCDecoderConfigurationRecord and the length-prefixed payload
/// of an item into one Annex B byte stream.
fn annex_b(hvcc: &Hvcc, data: &[u8]) -> Result<Vec<u8>> {
    const START: [u8; 4] = [0, 0, 0, 1];
    let mut out = Vec::with_capacity(hvcc.arrays.len() + data.len() + 64);
    let mut r = Reader::new(&hvcc.arrays);
    let arrays = r.u8()?;
    for _ in 0..arrays {
        r.skip(1)?; // array_completeness | reserved | NAL_unit_type
        let count = r.u16()?;
        for _ in 0..count {
            let len = usize::from(r.u16()?);
            out.extend_from_slice(&START);
            out.extend_from_slice(r.take(len)?);
        }
    }
    let mut r = Reader::new(data);
    while r.remaining() >= hvcc.length_size {
        let len = usize::try_from(r.uint(hvcc.length_size)?)
            .map_err(|_| Error::Malformed("NAL unit length too large"))?;
        let nal = r
            .take(len)
            .map_err(|_| Error::Malformed("NAL unit extends past the item"))?;
        if nal.is_empty() {
            continue;
        }
        out.extend_from_slice(&START);
        out.extend_from_slice(nal);
    }
    Ok(out)
}

fn map_decode_error(e: DecodeError) -> Error {
    match e {
        DecodeError::Unsupported(m) => Error::Unsupported(m),
        other => Error::Hevc(other.to_string()),
    }
}

/// Decode the single picture of an HEVC-coded image item.
///
/// `admit` is called for every SPS in the stream *before* any decoding; returning an error
/// from it (for example because the picture would exceed the caller's limits) aborts the
/// decode without the decoder having allocated anything.
pub(crate) fn decode(
    hvcc: &Hvcc,
    data: &[u8],
    mut admit: impl FnMut(&SpsInfo) -> Result<()>,
) -> Result<Frame> {
    let stream = annex_b(hvcc, data)?;
    // Only the base layer is decoded; HEVC extensions (multiview, scalable) are ignored.
    let nals: Vec<_> = parse_annex_b(&stream)
        .into_iter()
        .filter(|n| n.nuh_layer_id == 0)
        .collect();

    let mut sps_seen = false;
    for nal in nals.iter().filter(|n| n.nal_unit_type == NalUnitType::Sps) {
        let sps = parse_sps(&nal.rbsp)?;
        match sps.chroma_format_idc {
            1 => {}
            0 => return Err(Error::Unsupported(MONOCHROME)),
            _ => return Err(Error::Unsupported("HEVC chroma format other than 4:2:0")),
        }
        admit(&sps)?;
        sps_seen = true;
    }
    if !sps_seen {
        return Err(Error::Malformed("no SPS in HEVC item"));
    }

    // rust_h265 is itself safe Rust, but it is a young decoder: contain any panic so that
    // hostile input can never take the caller down.
    let run = || -> Result<Frame> {
        let mut dec = Decoder::new();
        for nal in &nals {
            if let Some(frame) = dec.decode_nal(nal).map_err(map_decode_error)? {
                return Ok(frame);
            }
        }
        dec.flush()
            .ok_or_else(|| Error::Hevc("no picture decoded".into()))
    };
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| Err(Error::Hevc("decoder panicked".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb() {
        // 1 -> 0; 010 -> 1; 011 -> 2; 00100 -> 3; 00111 -> 6
        let mut b = Bits::new(&[0b1010_0110, 0b0100_0011, 0b1000_0000]);
        assert_eq!(b.ue().unwrap(), 0);
        assert_eq!(b.ue().unwrap(), 1);
        assert_eq!(b.ue().unwrap(), 2);
        assert_eq!(b.ue().unwrap(), 3);
        assert_eq!(b.ue().unwrap(), 6);
        assert!(Bits::new(&[0, 0, 0, 0, 0, 0]).ue().is_err());
        assert!(Bits::new(&[0]).ue().is_err());
    }

    #[test]
    fn annex_b_assembly() {
        let hvcc = Hvcc {
            length_size: 2,
            bit_depth_luma: 8,
            // one array, completeness byte, 1 NAL of 3 bytes
            arrays: vec![1, 0xA1, 0, 1, 0, 3, 0x42, 0x01, 0xFF],
        };
        let data = [0, 2, 0x26, 0x01, 0, 0, 0, 1, 0x99, 0];
        let out = annex_b(&hvcc, &data).unwrap();
        assert_eq!(
            out,
            vec![
                0, 0, 0, 1, 0x42, 0x01, 0xFF, 0, 0, 0, 1, 0x26, 0x01, 0, 0, 0, 1, 0x99
            ]
        );
        // a NAL running past the end of the item is an error
        assert!(annex_b(&hvcc, &[0, 9, 1, 2]).is_err());
        // truncated parameter set array
        let bad = Hvcc {
            arrays: vec![1, 0xA1, 0, 1, 0, 9, 1],
            ..hvcc
        };
        assert!(annex_b(&bad, &data).is_err());
    }

    /// SPS of the committed quad fixture: 160x120, 4:2:0, 8-bit.
    #[test]
    fn parses_the_quad_fixture_sps() {
        let file = include_bytes!("../tests/fixtures/quad.heic");
        let heif = crate::container::Heif::parse(file).unwrap();
        let item = heif.primary_item().unwrap();
        let props = heif.item_props(item).unwrap();
        let hvcc = props.hvcc.unwrap();
        let stream = annex_b(hvcc, &heif.item_data(item).unwrap()).unwrap();
        let nals = parse_annex_b(&stream);
        let sps_nal = nals
            .iter()
            .find(|n| n.nal_unit_type == NalUnitType::Sps)
            .unwrap();
        let sps = parse_sps(&sps_nal.rbsp).unwrap();
        assert_eq!(
            sps,
            SpsInfo {
                width: 160,
                height: 120,
                chroma_format_idc: 1,
                bit_depth: 8
            }
        );
    }

    #[test]
    fn sps_garbage_is_an_error_not_a_panic() {
        for len in 0..40 {
            let _ = parse_sps(&vec![0xFFu8; len]);
            let _ = parse_sps(&vec![0u8; len]);
            let _ = parse_sps(&vec![0x55u8; len]);
        }
    }
}
