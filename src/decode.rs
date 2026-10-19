//! The decoding pipeline: item graph (coded / grid / identity), limits, alpha, transforms.
//!
//! Every allocation is preceded by a limit check computed from descriptor values
//! (`ispe`, grid descriptor, SPS), never from the amount of data in the file.

use crate::color::{ColorSpec, frame_to_alpha, frame_to_rgba};
use crate::container::{Heif, Item, ItemProps};
use crate::error::{Error, Result};
use crate::geom::{Plane, apply_ops, output_dims};
use crate::hevc::{self, MONOCHROME, SpsInfo};
use crate::props::Nclx;
use crate::reader::{FourCc, Reader};
use crate::{Codec, Image, Info, Limits};

/// Longest chain of `iden` derivations followed before giving up.
const MAX_DERIVATION_DEPTH: u8 = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Color,
    Alpha,
}

impl Kind {
    fn channels(self) -> usize {
        match self {
            Kind::Color => 4,
            Kind::Alpha => 1,
        }
    }
}

/// Immutable decoding context.
struct Env<'h, 'a> {
    heif: &'h Heif<'a>,
    limits: &'h Limits,
}

impl Env<'_, '_> {
    fn check_dims(&self, w: u32, h: u32) -> Result<()> {
        if w == 0 || h == 0 {
            return Err(Error::Malformed("image with a zero dimension"));
        }
        if w > self.limits.max_width {
            return Err(Error::LimitExceeded("image width"));
        }
        if h > self.limits.max_height {
            return Err(Error::LimitExceeded("image height"));
        }
        if u64::from(w) * u64::from(h) > self.limits.max_pixels {
            return Err(Error::LimitExceeded("image pixel count"));
        }
        Ok(())
    }

    /// Fail if the sum of `parts` (bytes held at the same time) exceeds the memory limit.
    fn reserve(&self, parts: &[u64]) -> Result<()> {
        let total = parts.iter().fold(0u64, |acc, &p| acc.saturating_add(p));
        if total > self.limits.max_memory_bytes {
            return Err(Error::LimitExceeded("memory estimate"));
        }
        Ok(())
    }
}

fn plane_bytes(w: u32, h: u32, channels: usize) -> u64 {
    (u64::from(w) * u64::from(h)).saturating_mul(channels as u64)
}

/// Allocation size for a pixel buffer; only reachable after the limit checks.
fn alloc_len(w: u32, h: u32, channels: usize) -> Result<usize> {
    usize::try_from(plane_bytes(w, h, channels))
        .map_err(|_| Error::LimitExceeded("memory estimate"))
}

/// Worst-case working memory of the HEVC decoder for one picture: three planes of 16-bit
/// samples (8-bit streams use half) and one more copy inside the decoder.
fn decode_bytes(sps: &SpsInfo) -> u64 {
    let sample = if sps.bit_depth > 8 { 2 } else { 1 };
    (u64::from(sps.width) * u64::from(sps.height)).saturating_mul(6 * sample)
}

fn unsupported_item(kind: &FourCc) -> Error {
    match kind {
        b"av01" => Error::Unsupported("AV1 (AVIF) image item"),
        b"jpeg" => Error::Unsupported("JPEG-coded HEIF image item"),
        b"unci" => Error::Unsupported("uncompressed image item"),
        b"iovl" => Error::Unsupported("image overlay (iovl)"),
        _ => Error::Unsupported("image item type"),
    }
}

fn is_hevc(kind: &FourCc) -> bool {
    matches!(kind, b"hvc1" | b"hev1")
}

/// `ImageGrid` (ISO/IEC 23008-12 clause 6.6.2.3).
struct Grid {
    rows: u32,
    cols: u32,
    width: u32,
    height: u32,
}

fn parse_grid(data: &[u8]) -> Result<Grid> {
    let mut r = Reader::new(data);
    if r.u8()? != 0 {
        return Err(Error::Unsupported("ImageGrid version"));
    }
    let flags = r.u8()?;
    let rows = u32::from(r.u8()?) + 1;
    let cols = u32::from(r.u8()?) + 1;
    let (width, height) = if flags & 1 != 0 {
        (r.u32()?, r.u32()?)
    } else {
        (u32::from(r.u16()?), u32::from(r.u16()?))
    };
    Ok(Grid {
        rows,
        cols,
        width,
        height,
    })
}

/// Pre-transform dimensions of an image item, from its descriptor.
fn base_dims(heif: &Heif<'_>, item: &Item, props: &ItemProps<'_>, depth: u8) -> Result<(u32, u32)> {
    match &item.kind {
        b"grid" => {
            let g = parse_grid(&heif.item_data(item)?)?;
            Ok((g.width, g.height))
        }
        b"iden" if props.ispe.is_none() => {
            if depth >= MAX_DERIVATION_DEPTH {
                return Err(Error::Malformed("derivation chain too deep"));
            }
            let input = identity_input(heif, item)?;
            let p = heif.item_props(input)?;
            let (w, h) = base_dims(heif, input, &p, depth + 1)?;
            output_dims(w, h, &p.ops)
        }
        _ => props
            .ispe
            .ok_or(Error::Malformed("image item without ispe")),
    }
}

fn identity_input<'h>(heif: &'h Heif<'_>, item: &Item) -> Result<&'h Item> {
    match heif.dimg_inputs(item.id) {
        Some([one]) => heif.item(*one),
        _ => Err(Error::Malformed("identity item needs exactly one input")),
    }
}

/// Render the pre-transform pixels of `item` (without its own transformative properties).
fn render_base(
    env: &Env<'_, '_>,
    item: &Item,
    kind: Kind,
    held: u64,
    inherit: Option<Nclx>,
    depth: u8,
) -> Result<Plane> {
    let props = env.heif.item_props(item)?;
    let nclx = props.nclx.or(inherit);
    match &item.kind {
        k if is_hevc(k) => render_coded(env, item, &props, kind, held, nclx),
        b"grid" => render_grid(env, item, kind, held, nclx),
        b"iden" => {
            if depth >= MAX_DERIVATION_DEPTH {
                return Err(Error::Malformed("derivation chain too deep"));
            }
            let input = identity_input(env.heif, item)?;
            render_final(env, input, kind, held, nclx, depth + 1)
        }
        k => Err(unsupported_item(k)),
    }
}

/// `render_base` followed by the item's own `clap` / `irot` / `imir`.
fn render_final(
    env: &Env<'_, '_>,
    item: &Item,
    kind: Kind,
    held: u64,
    inherit: Option<Nclx>,
    depth: u8,
) -> Result<Plane> {
    let props = env.heif.item_props(item)?;
    let (bw, bh) = base_dims(env.heif, item, &props, depth)?;
    env.check_dims(bw, bh)?;
    let (ow, oh) = output_dims(bw, bh, &props.ops)?;
    env.check_dims(ow, oh)?;
    let base = plane_bytes(bw, bh, kind.channels());
    let transformed = if props.ops.is_empty() {
        0
    } else {
        plane_bytes(ow, oh, kind.channels())
    };
    env.reserve(&[held, base, transformed])?;
    let plane = render_base(env, item, kind, held, inherit, depth)?;
    apply_ops(plane, &props.ops)
}

fn render_coded(
    env: &Env<'_, '_>,
    item: &Item,
    props: &ItemProps<'_>,
    kind: Kind,
    held: u64,
    nclx: Option<Nclx>,
) -> Result<Plane> {
    let (w, h) = props
        .ispe
        .ok_or(Error::Malformed("coded image item without ispe"))?;
    env.check_dims(w, h)?;
    let hvcc = props
        .hvcc
        .ok_or(Error::Malformed("coded image item without hvcC"))?;
    let out = plane_bytes(w, h, kind.channels());
    let data = env.heif.item_data(item)?;
    let frame = hevc::decode(hvcc, &data, |sps| {
        env.check_dims(sps.width, sps.height)?;
        env.reserve(&[held, out, decode_bytes(sps)])
    })?;
    let channels = kind.channels();
    let data = match kind {
        Kind::Color => frame_to_rgba(&frame, ColorSpec::from_nclx(nclx)?, w, h)?,
        Kind::Alpha => frame_to_alpha(&frame, w, h)?,
    };
    Ok(Plane {
        width: w,
        height: h,
        channels,
        data,
    })
}

fn render_grid(
    env: &Env<'_, '_>,
    item: &Item,
    kind: Kind,
    held: u64,
    nclx: Option<Nclx>,
) -> Result<Plane> {
    let grid = parse_grid(&env.heif.item_data(item)?)?;
    env.check_dims(grid.width, grid.height)?;
    let channels = kind.channels();
    let canvas_bytes = plane_bytes(grid.width, grid.height, channels);
    env.reserve(&[held, canvas_bytes])?;

    let tiles = env
        .heif
        .dimg_inputs(item.id)
        .ok_or(Error::Malformed("grid item without dimg reference"))?;
    if u64::try_from(tiles.len()) != Ok(u64::from(grid.rows) * u64::from(grid.cols)) {
        return Err(Error::Malformed(
            "grid tile count does not match rows x columns",
        ));
    }
    let first = env.heif.item(tiles[0])?;
    if !is_hevc(&first.kind) {
        return Err(unsupported_item(&first.kind));
    }
    let (tw, th) = env
        .heif
        .item_props(first)?
        .ispe
        .ok_or(Error::Malformed("grid tile without ispe"))?;
    env.check_dims(tw, th)?;
    // The tiles must cover the output exactly: no hole, and no tile entirely outside it.
    // This also bounds the total decoded area to under four times the output area.
    let covers = |n: u32, t: u32, out: u32| {
        u64::from(n) * u64::from(t) >= u64::from(out)
            && u64::from(n - 1) * u64::from(t) < u64::from(out)
    };
    if !covers(grid.cols, tw, grid.width) || !covers(grid.rows, th, grid.height) {
        return Err(Error::Malformed("grid tiles do not match the output size"));
    }

    let (cw, ch) = (grid.width as usize, channels);
    let mut canvas = vec![0u8; alloc_len(grid.width, grid.height, channels)?];
    for (i, id) in tiles.iter().enumerate() {
        let tile_item = env.heif.item(*id)?;
        if !is_hevc(&tile_item.kind) {
            return Err(unsupported_item(&tile_item.kind));
        }
        let tile_props = env.heif.item_props(tile_item)?;
        if tile_props.ispe != Some((tw, th)) {
            return Err(Error::Malformed("grid tiles differ in size"));
        }
        let tile = render_coded(env, tile_item, &tile_props, kind, held + canvas_bytes, nclx)?;
        let (row, col) = (i / grid.cols as usize, i % grid.cols as usize);
        let (x0, y0) = (col * tw as usize, row * th as usize);
        let copy_w = (tw as usize).min(cw - x0);
        let copy_h = (th as usize).min(grid.height as usize - y0);
        for y in 0..copy_h {
            let src = y * tw as usize * ch;
            let dst = ((y0 + y) * cw + x0) * ch;
            canvas[dst..dst + copy_w * ch].copy_from_slice(&tile.data[src..src + copy_w * ch]);
        }
    }
    Ok(Plane {
        width: grid.width,
        height: grid.height,
        channels,
        data: canvas,
    })
}

/// Combine the colour plane with an alpha plane (nearest-neighbour if the sizes differ).
fn merge_alpha(color: &mut Plane, alpha: &Plane) {
    let (w, h) = (color.width as usize, color.height as usize);
    let (aw, ah) = (alpha.width as usize, alpha.height as usize);
    for (y, row) in color.data.chunks_exact_mut(w * 4).enumerate() {
        let ay = y * ah / h;
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            px[3] = alpha.data[ay * aw + x * aw / w];
        }
    }
}

/// Decode `item` to straight-alpha RGBA with its own transformative properties applied.
/// Transformative properties are per item: a thumbnail is not affected by the primary
/// image's `irot`/`imir` (iPhone files carry the rotation on both items).
fn render_image(env: &Env<'_, '_>, item: &Item) -> Result<Image> {
    let heif = env.heif;
    let props = heif.item_props(item)?;
    let (bw, bh) = base_dims(heif, item, &props, 0)?;
    env.check_dims(bw, bh)?;
    let ops = &props.ops;
    let (ow, oh) = output_dims(bw, bh, ops)?;
    env.check_dims(ow, oh)?;
    // Peak during the final transform: source and destination buffers.
    let canvas = plane_bytes(bw, bh, 4);
    env.reserve(&[
        canvas,
        if ops.is_empty() {
            0
        } else {
            plane_bytes(ow, oh, 4)
        },
    ])?;

    let mut color = render_base(env, item, Kind::Color, 0, None, 0)?;
    if let Some(alpha_item) = heif.alpha_item(item.id) {
        let alpha_bytes = plane_bytes(bw, bh, 1);
        env.reserve(&[canvas, alpha_bytes])?;
        match render_base(env, alpha_item, Kind::Alpha, canvas, None, 0) {
            Ok(alpha) => merge_alpha(&mut color, &alpha),
            // Alpha is an enhancement: when it cannot be decoded (Apple writes it as
            // monochrome HEVC, which the HEVC decoder lacks) the image stays opaque.
            Err(Error::Unsupported(m)) if m == MONOCHROME => {}
            Err(e @ Error::LimitExceeded(_)) => return Err(e),
            Err(_) => {}
        }
    }
    let out = apply_ops(color, ops)?;
    Ok(Image {
        width: out.width,
        height: out.height,
        rgba: out.data,
    })
}

pub(crate) fn decode(heif: &Heif<'_>, limits: &Limits) -> Result<Image> {
    let env = Env { heif, limits };
    render_image(&env, heif.primary_item()?)
}

/// Candidate thumbnails of the primary item as `(long edge, item)`.
fn thumbnails<'h>(heif: &'h Heif<'_>) -> Vec<(u32, &'h Item)> {
    let mut out: Vec<(u32, &Item)> = heif
        .sources_referencing(heif.primary, b"thmb")
        .filter_map(|it| {
            let props = heif.item_props(it).ok()?;
            let (w, h) = base_dims(heif, it, &props, 0).ok()?;
            Some((w.max(h), it))
        })
        .collect();
    out.sort_by_key(|&(edge, it)| (edge, it.id));
    out
}

pub(crate) fn decode_thumbnail(heif: &Heif<'_>, max_edge: u32, limits: &Limits) -> Result<Image> {
    let env = Env { heif, limits };
    // Smallest thumbnail that is large enough, then the remaining ones from the largest down.
    let thumbs = thumbnails(heif);
    let split = thumbs.partition_point(|&(edge, _)| edge < max_edge);
    let order = thumbs[split..].iter().chain(thumbs[..split].iter().rev());
    for &(_, thumb) in order {
        match render_image(&env, thumb) {
            Ok(img) => return Ok(img),
            Err(e @ Error::LimitExceeded(_)) => return Err(e),
            // An unusable thumbnail (for example JPEG-coded) is not fatal: try the next
            // candidate and finally the primary image.
            Err(_) => {}
        }
    }
    render_image(&env, heif.primary_item()?)
}

fn codec_of(heif: &Heif<'_>, item: &Item, depth: u8) -> Codec {
    match &item.kind {
        k if is_hevc(k) => Codec::Hevc,
        b"av01" => Codec::Av1,
        b"jpeg" => Codec::Jpeg,
        b"unci" => Codec::Uncompressed,
        b"grid" | b"iden" | b"iovl" if depth < MAX_DERIVATION_DEPTH => {
            let input = heif
                .dimg_inputs(item.id)
                .and_then(<[u32]>::first)
                .and_then(|&id| heif.item(id).ok());
            input.map_or(Codec::Other(item.kind), |i| codec_of(heif, i, depth + 1))
        }
        k => Codec::Other(*k),
    }
}

fn bit_depth_of(heif: &Heif<'_>, item: &Item, props: &ItemProps<'_>, depth: u8) -> u8 {
    if let Some(h) = props.hvcc {
        return h.bit_depth_luma;
    }
    if matches!(&item.kind, b"grid" | b"iden" | b"iovl") && depth < MAX_DERIVATION_DEPTH {
        let input = heif
            .dimg_inputs(item.id)
            .and_then(<[u32]>::first)
            .and_then(|&id| heif.item(id).ok());
        if let Some(i) = input {
            if let Ok(p) = heif.item_props(i) {
                return bit_depth_of(heif, i, &p, depth + 1);
            }
        }
    }
    props.pixi_depth.filter(|&d| d > 0).unwrap_or(8)
}

pub(crate) fn info(heif: &Heif<'_>) -> Result<Info> {
    let item = heif.primary_item()?;
    let props = heif.item_props(item)?;
    let (bw, bh) = base_dims(heif, item, &props, 0)?;
    let (width, height) = output_dims(bw, bh, &props.ops)?;
    Ok(Info {
        width,
        height,
        has_alpha: heif.alpha_item(item.id).is_some(),
        has_thumbnail: heif.sources_referencing(item.id, b"thmb").next().is_some(),
        bit_depth: bit_depth_of(heif, item, &props, 0),
        codec: codec_of(heif, item, 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_descriptor_16_and_32_bit_fields() {
        // version 0, flags 0, rows-1 = 2, columns-1 = 3, 16-bit output size 4032 x 3024
        let g = parse_grid(&[0, 0, 2, 3, 0x0F, 0xC0, 0x0B, 0xD0]).unwrap();
        assert_eq!((g.rows, g.cols, g.width, g.height), (3, 4, 4032, 3024));
        // flags bit 0: 32-bit output size; trailing bytes are ignored
        let mut d = vec![0, 1, 0, 0];
        d.extend_from_slice(&70_000u32.to_be_bytes());
        d.extend_from_slice(&80_000u32.to_be_bytes());
        d.push(0xAA);
        let g = parse_grid(&d).unwrap();
        assert_eq!((g.rows, g.cols, g.width, g.height), (1, 1, 70_000, 80_000));
    }

    #[test]
    fn grid_descriptor_errors() {
        assert!(matches!(
            parse_grid(&[1, 0, 0, 0, 0, 1, 0, 1]),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            parse_grid(&[0, 0, 0, 0, 0, 1]),
            Err(Error::Malformed(_))
        ));
        assert!(matches!(
            parse_grid(&[0, 1, 0, 0, 0, 0, 0, 1, 0, 0]),
            Err(Error::Malformed(_))
        ));
        assert!(matches!(parse_grid(&[]), Err(Error::Malformed(_))));
    }

    #[test]
    fn memory_estimates_saturate() {
        assert_eq!(plane_bytes(u32::MAX, u32::MAX, 4), u64::MAX);
        let sps = SpsInfo {
            width: u32::MAX,
            height: u32::MAX,
            chroma_format_idc: 1,
            bit_depth: 10,
        };
        assert_eq!(decode_bytes(&sps), u64::MAX);
        let limits = Limits {
            max_memory_bytes: u64::MAX,
            ..Limits::default()
        };
        let file = include_bytes!("../tests/fixtures/quad.heic");
        let heif = Heif::parse(file).unwrap();
        let env = Env {
            heif: &heif,
            limits: &limits,
        };
        assert!(env.reserve(&[u64::MAX, u64::MAX]).is_ok());
        let limits = Limits {
            max_memory_bytes: 10,
            ..Limits::default()
        };
        let env = Env {
            heif: &heif,
            limits: &limits,
        };
        assert!(env.reserve(&[6, 5]).is_err());
        assert!(env.reserve(&[6, 4]).is_ok());
    }
}
