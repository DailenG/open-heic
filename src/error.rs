use std::fmt;

/// Everything that can go wrong while reading a HEIC/HEIF file.
///
/// No input makes this crate panic: malformed, hostile or merely unsupported files all end
/// up as one of these values.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The bytes are not an ISO base media file with a HEIF/AVIF brand (for example a JPEG
    /// or PNG that was renamed `.heic`).
    NotHeif,
    /// The file is valid but uses a feature this crate does not implement (AV1, JPEG-coded
    /// items, image overlays, non-4:2:0 HEVC, ...). The caller should fall back to another
    /// decoder.
    Unsupported(&'static str),
    /// A [`Limits`](crate::Limits) bound would be exceeded. Nothing was allocated for the
    /// offending image.
    LimitExceeded(&'static str),
    /// The container or a descriptor is structurally invalid or truncated.
    Malformed(&'static str),
    /// The HEVC decoder rejected the coded picture.
    Hevc(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotHeif => f.write_str("not a HEIF/HEIC file"),
            Error::Unsupported(m) => write!(f, "unsupported HEIF feature: {m}"),
            Error::LimitExceeded(m) => write!(f, "limit exceeded: {m}"),
            Error::Malformed(m) => write!(f, "malformed HEIF file: {m}"),
            Error::Hevc(m) => write!(f, "HEVC decoding failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;
