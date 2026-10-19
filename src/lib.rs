//! # open-heic
//!
//! A pure-Rust, `#![forbid(unsafe_code)]` reader for HEIC/HEIF still images, written for
//! untrusted input (thumbnailers, indexers, upload pipelines).
//!
//! * Container: ISO/IEC 14496-12 / 23008-12 `meta` parsing with file and `idat` item
//!   storage, multi-extent items, thumbnails, auxiliary (alpha) images, grids and identity
//!   derivations, and the `irot`, `imir`, `clap`, `colr`, `pixi`, `ispe` properties.
//! * Pixels: HEVC (Main / Main 10, 4:2:0) through the pure-Rust [`rust_h265`] decoder,
//!   converted to 8-bit straight-alpha RGBA with the colour matrix the file declares.
//! * Safety: every dimension and memory bound in [`Limits`] is checked from the file's
//!   descriptors *before* anything is allocated, all arithmetic on file-controlled values is
//!   checked, and no input makes the crate panic.
//!
//! ```no_run
//! use open_heic::{decode, decode_thumbnail, Limits};
//!
//! let bytes = std::fs::read("photo.heic")?;
//! let limits = Limits::default();
//! let thumb = decode_thumbnail(&bytes, 256, &limits)?; // fast: uses the embedded thumbnail
//! let full = decode(&bytes, &limits)?;
//! assert_eq!(full.rgba.len(), full.width as usize * full.height as usize * 4);
//! # let _ = thumb;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! HEVC may be covered by third-party patents; see the README.

#![forbid(unsafe_code)]

mod color;
mod container;
mod decode;
mod error;
mod geom;
mod hevc;
mod props;
mod reader;

pub use error::Error;

use container::Heif;

/// Resource bounds applied to every decode. All of them are checked against the file's
/// declared sizes before the corresponding allocation is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest accepted image width in pixels (of any item decoded and of the output).
    pub max_width: u32,
    /// Largest accepted image height in pixels.
    pub max_height: u32,
    /// Largest accepted `width * height`.
    pub max_pixels: u64,
    /// Upper bound on the estimated peak heap use of a decode, in bytes. The estimate
    /// covers the output canvas, one decoded tile or picture with the HEVC decoder's
    /// working copies, and the buffer made by orientation/crop transforms.
    pub max_memory_bytes: u64,
}

impl Default for Limits {
    /// 16384 x 16384, 256 megapixels, 1 GiB.
    fn default() -> Self {
        Limits {
            max_width: 16384,
            max_height: 16384,
            max_pixels: 256_000_000,
            max_memory_bytes: 1 << 30,
        }
    }
}

/// A decoded image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels, after `clap` cropping and `irot`/`imir` orientation.
    pub width: u32,
    /// Height in pixels, after `clap` cropping and `irot`/`imir` orientation.
    pub height: u32,
    /// 8-bit RGBA, straight (non-premultiplied) alpha, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// The coding format of an image item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// HEVC (`hvc1` / `hev1`).
    Hevc,
    /// AV1 (AVIF); detected, not decodable here.
    Av1,
    /// JPEG-coded HEIF item (`jpeg`); detected, not decodable here.
    Jpeg,
    /// Uncompressed image item (`unci`); detected, not decodable here.
    Uncompressed,
    /// Any other item type, as its four-character code.
    Other([u8; 4]),
}

/// Facts about a file obtained from its container alone (nothing is decoded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// Width of the primary image as [`decode`] would return it (after crop/rotation).
    pub width: u32,
    /// Height of the primary image as [`decode`] would return it (after crop/rotation).
    pub height: u32,
    /// The primary image has an alpha auxiliary image.
    pub has_alpha: bool,
    /// The primary image has at least one embedded thumbnail item.
    pub has_thumbnail: bool,
    /// Bits per sample of the coded primary image (8 when the file does not say).
    pub bit_depth: u8,
    /// Coding format of the primary image (of its first tile for grids).
    pub codec: Codec,
}

/// Read container-level facts about the primary image without decoding it.
///
/// # Errors
///
/// [`Error::NotHeif`] when `data` is not an ISO base media file with a HEIF/AVIF brand,
/// [`Error::Malformed`] when the container is damaged.
pub fn info(data: &[u8]) -> Result<Info, Error> {
    decode::info(&Heif::parse(data)?)
}

/// Decode the primary image: cropped (`clap`), rotated (`irot`) and mirrored (`imir`) as the
/// file specifies, converted to straight-alpha RGBA.
///
/// # Errors
///
/// See [`Error`]. Files that are not HEVC-coded (AV1, JPEG-coded items) yield
/// [`Error::Unsupported`] so the caller can fall back to another decoder.
pub fn decode(data: &[u8], limits: &Limits) -> Result<Image, Error> {
    decode::decode(&Heif::parse(data)?, limits)
}

/// Decode a thumbnail suitable for a `max_edge` pixel preview.
///
/// Chooses the smallest embedded thumbnail whose longer side is at least `max_edge`; if none
/// is that large, the largest thumbnail; if the file has no usable thumbnail, the primary
/// image. The result is never scaled: the caller resizes to its exact target size.
/// Orientation is applied. A thumbnail that cannot be decoded (for example because it is
/// JPEG-coded) is skipped in favour of the next candidate.
///
/// # Errors
///
/// See [`Error`].
pub fn decode_thumbnail(data: &[u8], max_edge: u32, limits: &Limits) -> Result<Image, Error> {
    decode::decode_thumbnail(&Heif::parse(data)?, max_edge, limits)
}
