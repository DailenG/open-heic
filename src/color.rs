//! YCbCr 4:2:0 to 8-bit RGB conversion.
//!
//! The matrix comes from the `colr`/`nclx` property (ITU-T H.273 `MatrixCoefficients`);
//! without one HEIF defaults to BT.601 with limited ("video") range. No gamut or transfer
//! conversion is performed: the decoded values are handed back as the file's own colour
//! space, which for photographs is sRGB or Display P3.

use rust_h265::{Frame, PixelData};

use crate::error::{Error, Result};
use crate::props::Nclx;

/// Luma weights `(Kr, Kb)` and range of a YCbCr encoding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ColorSpec {
    pub kr: f64,
    pub kb: f64,
    pub full_range: bool,
}

impl Default for ColorSpec {
    /// BT.601, limited range: the HEIF default when no `nclx` is present.
    fn default() -> Self {
        ColorSpec {
            kr: 0.299,
            kb: 0.114,
            full_range: false,
        }
    }
}

impl ColorSpec {
    pub fn from_nclx(nclx: Option<Nclx>) -> Result<Self> {
        let Some(n) = nclx else {
            return Ok(Self::default());
        };
        let (kr, kb) = match n.matrix {
            // 2 = unspecified: fall back to the HEIF default.
            2 => (0.299, 0.114),
            1 => (0.2126, 0.0722),
            4 => (0.30, 0.11),
            5 | 6 => (0.299, 0.114),
            7 => (0.212, 0.087),
            9 | 10 => (0.2627, 0.0593),
            0 => return Err(Error::Unsupported("identity (RGB) matrix with 4:2:0 HEVC")),
            _ => return Err(Error::Unsupported("colour matrix coefficients")),
        };
        Ok(ColorSpec {
            kr,
            kb,
            full_range: n.full_range,
        })
    }
}

/// Fixed-point (Q16) conversion constants for one bit depth.
struct Coeffs {
    y_off: i32,
    c_off: i32,
    y_scale: i32,
    r_cr: i32,
    g_cb: i32,
    g_cr: i32,
    b_cb: i32,
}

impl Coeffs {
    fn new(spec: ColorSpec, bit_depth: u8) -> Self {
        let s = f64::from(1u32 << (bit_depth - 8));
        let max = f64::from((1u32 << bit_depth) - 1);
        let (y_off, y_span, c_span) = if spec.full_range {
            (0.0, max, max)
        } else {
            (16.0 * s, 219.0 * s, 224.0 * s)
        };
        let kg = 1.0 - spec.kr - spec.kb;
        let fy = 255.0 / y_span;
        let fc = 255.0 / c_span;
        let q = |v: f64| (v * 65536.0).round() as i32;
        Coeffs {
            y_off: y_off as i32,
            c_off: 1 << (bit_depth - 1),
            y_scale: q(fy),
            r_cr: q(2.0 * (1.0 - spec.kr) * fc),
            g_cb: q(2.0 * spec.kb * (1.0 - spec.kb) / kg * fc),
            g_cr: q(2.0 * spec.kr * (1.0 - spec.kr) / kg * fc),
            b_cb: q(2.0 * (1.0 - spec.kb) * fc),
        }
    }
}

fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Validate that `frame` is a well-formed 4:2:0 picture at least `w` x `h` and return the
/// chroma plane width.
fn check_frame(frame: &Frame, w: u32, h: u32) -> Result<usize> {
    if !(8..=16).contains(&frame.bit_depth) {
        return Err(Error::Unsupported("HEVC bit depth"));
    }
    if frame.width < w || frame.height < h {
        return Err(Error::Malformed(
            "decoded picture smaller than the image extents",
        ));
    }
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    let (cw, ch) = (fw.div_ceil(2), fh.div_ceil(2));
    let ok = |p: &PixelData, n: usize| p.len() >= n;
    if !(ok(&frame.y, fw * fh) && ok(&frame.u, cw * ch) && ok(&frame.v, cw * ch)) {
        return Err(Error::Malformed("decoded picture planes too small"));
    }
    Ok(cw)
}

/// Convert the top-left `w` x `h` region of `frame` to RGBA (alpha 255).
pub(crate) fn frame_to_rgba(frame: &Frame, spec: ColorSpec, w: u32, h: u32) -> Result<Vec<u8>> {
    let c_stride = check_frame(frame, w, h)?;
    let k = Coeffs::new(spec, frame.bit_depth);
    let g = Geometry {
        y_stride: frame.width as usize,
        c_stride,
        c_rows: (frame.height as usize).div_ceil(2),
        w: w as usize,
        h: h as usize,
    };
    match (&frame.y, &frame.u, &frame.v) {
        (PixelData::U8(y), PixelData::U8(u), PixelData::U8(v)) => Ok(convert(y, u, v, &g, &k)),
        (PixelData::U16(y), PixelData::U16(u), PixelData::U16(v)) => Ok(convert(y, u, v, &g, &k)),
        _ => Err(Error::Malformed("mixed sample widths in decoded picture")),
    }
}

/// Interpret the luma plane of `frame` as an alpha plane (full range, scaled to 8 bits)
/// for the top-left `w` x `h` region.
pub(crate) fn frame_to_alpha(frame: &Frame, w: u32, h: u32) -> Result<Vec<u8>> {
    check_frame(frame, w, h)?;
    let (w, h, stride) = (w as usize, h as usize, frame.width as usize);
    let max = (1u32 << frame.bit_depth) - 1;
    let mut out = Vec::with_capacity(w * h);
    let mut row = |samples: &mut dyn Iterator<Item = u32>| {
        out.extend(samples.map(|s| ((s.min(max) * 255 + max / 2) / max) as u8));
    };
    match &frame.y {
        PixelData::U8(y) => {
            for line in y.chunks_exact(stride).take(h) {
                row(&mut line[..w].iter().map(|&s| u32::from(s)));
            }
        }
        PixelData::U16(y) => {
            for line in y.chunks_exact(stride).take(h) {
                row(&mut line[..w].iter().map(|&s| u32::from(s)));
            }
        }
    }
    Ok(out)
}

/// Samples widen losslessly to `i32` (`u8` and `u16`).
trait Sample: Copy + Into<i32> {}
impl Sample for u8 {}
impl Sample for u16 {}

/// Plane geometry shared by the three planes of a 4:2:0 picture.
struct Geometry {
    y_stride: usize,
    c_stride: usize,
    c_rows: usize,
    /// Output region.
    w: usize,
    h: usize,
}

/// 4:2:0 to RGBA with triangle-filtered (3:1) chroma upsampling, chroma samples centred
/// between their luma samples.
fn convert<T: Sample>(y: &[T], u: &[T], v: &[T], g: &Geometry, k: &Coeffs) -> Vec<u8> {
    let Geometry {
        y_stride,
        c_stride,
        c_rows,
        w,
        h,
    } = *g;
    let c_cols = w.div_ceil(2);
    let mut out = vec![255u8; w * h * 4];
    // Chroma rows blended vertically, still at chroma horizontal resolution, scaled by 4.
    let mut cb_row = vec![0i32; c_cols];
    let mut cr_row = vec![0i32; c_cols];
    for (row, dst) in out.chunks_exact_mut(w * 4).enumerate() {
        let near = (row / 2).min(c_rows - 1);
        let far = if row % 2 == 0 {
            near.saturating_sub(1)
        } else {
            (near + 1).min(c_rows - 1)
        };
        let (n0, f0) = (near * c_stride, far * c_stride);
        for x in 0..c_cols {
            cb_row[x] = 3 * u[n0 + x].into() + u[f0 + x].into();
            cr_row[x] = 3 * v[n0 + x].into() + v[f0 + x].into();
        }
        let luma = &y[row * y_stride..row * y_stride + w];
        for (x, (&ys, px)) in luma.iter().zip(dst.chunks_exact_mut(4)).enumerate() {
            let near = x / 2;
            let far = if x % 2 == 0 {
                near.saturating_sub(1)
            } else {
                (near + 1).min(c_cols - 1)
            };
            // Scaled by 16 so far, +8 rounds the final >> 4.
            let cb = ((3 * cb_row[near] + cb_row[far] + 8) >> 4) - k.c_off;
            let cr = ((3 * cr_row[near] + cr_row[far] + 8) >> 4) - k.c_off;
            let yy = (ys.into() - k.y_off) * k.y_scale;
            px[0] = clamp8((yy + k.r_cr * cr + 32768) >> 16);
            px[1] = clamp8((yy - k.g_cb * cb - k.g_cr * cr + 32768) >> 16);
            px[2] = clamp8((yy + k.b_cb * cb + 32768) >> 16);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame8(w: u32, h: u32, y: u8, u: u8, v: u8) -> Frame {
        let cw = (w as usize).div_ceil(2);
        let ch = (h as usize).div_ceil(2);
        Frame {
            y: PixelData::U8(vec![y; (w * h) as usize]),
            u: PixelData::U8(vec![u; cw * ch]),
            v: PixelData::U8(vec![v; cw * ch]),
            width: w,
            height: h,
            pic_order_cnt: 0,
            bit_depth: 8,
        }
    }

    fn first(rgba: &[u8]) -> [u8; 3] {
        [rgba[0], rgba[1], rgba[2]]
    }

    #[test]
    fn bt601_limited_black_white_grey() {
        let spec = ColorSpec::default();
        assert_eq!(
            first(&frame_to_rgba(&frame8(4, 4, 16, 128, 128), spec, 4, 4).unwrap()),
            [0, 0, 0]
        );
        assert_eq!(
            first(&frame_to_rgba(&frame8(4, 4, 235, 128, 128), spec, 4, 4).unwrap()),
            [255, 255, 255]
        );
        let g = first(&frame_to_rgba(&frame8(4, 4, 126, 128, 128), spec, 4, 4).unwrap());
        assert!(g.iter().all(|&c| (126..=129).contains(&c)), "{g:?}");
    }

    #[test]
    fn bt601_limited_primaries() {
        let spec = ColorSpec::default();
        // BT.601 limited-range encodings of pure red / green / blue.
        for (yuv, rgb) in [
            ([81u8, 90, 240], [255i32, 0, 0]),
            ([145, 54, 34], [0, 255, 0]),
            ([41, 240, 110], [0, 0, 255]),
        ] {
            let f = frame8(4, 4, yuv[0], yuv[1], yuv[2]);
            let out = first(&frame_to_rgba(&f, spec, 4, 4).unwrap());
            for c in 0..3 {
                assert!(
                    (i32::from(out[c]) - rgb[c]).abs() <= 3,
                    "{yuv:?} -> {out:?}, want {rgb:?}"
                );
            }
        }
    }

    #[test]
    fn matrices_differ() {
        let nclx = |matrix, full_range| Nclx {
            primaries: 1,
            transfer: 13,
            matrix,
            full_range,
        };
        let f = frame8(4, 4, 81, 90, 240);
        let rgb = |spec: ColorSpec| first(&frame_to_rgba(&f, spec, 4, 4).unwrap());
        let b601 = rgb(ColorSpec::from_nclx(Some(nclx(6, false))).unwrap());
        let b709 = rgb(ColorSpec::from_nclx(Some(nclx(1, false))).unwrap());
        assert_ne!(b601, b709);
        assert_eq!(b601, rgb(ColorSpec::default()));
        assert_eq!(
            rgb(ColorSpec::from_nclx(Some(nclx(2, false))).unwrap()),
            b601
        );
        // BT.709 limited: Y=63, Cb=102, Cr=240 is pure red.
        let f = frame8(4, 4, 63, 102, 240);
        let out = first(
            &frame_to_rgba(
                &f,
                ColorSpec::from_nclx(Some(nclx(1, false))).unwrap(),
                4,
                4,
            )
            .unwrap(),
        );
        assert!(out[0] >= 252 && out[1] <= 3 && out[2] <= 3, "{out:?}");
        // BT.2020 NCL limited: Y=74, Cb=97, Cr=240 is pure red.
        let f = frame8(4, 4, 74, 97, 240);
        let out = first(
            &frame_to_rgba(
                &f,
                ColorSpec::from_nclx(Some(nclx(9, false))).unwrap(),
                4,
                4,
            )
            .unwrap(),
        );
        assert!(out[0] >= 252 && out[1] <= 3 && out[2] <= 3, "{out:?}");
    }

    #[test]
    fn full_range_uses_the_whole_code_space() {
        let spec = ColorSpec {
            full_range: true,
            ..ColorSpec::default()
        };
        assert_eq!(
            first(&frame_to_rgba(&frame8(2, 2, 0, 128, 128), spec, 2, 2).unwrap()),
            [0, 0, 0]
        );
        assert_eq!(
            first(&frame_to_rgba(&frame8(2, 2, 255, 128, 128), spec, 2, 2).unwrap()),
            [255, 255, 255]
        );
        assert_eq!(
            first(&frame_to_rgba(&frame8(2, 2, 100, 128, 128), spec, 2, 2).unwrap()),
            [100, 100, 100]
        );
    }

    #[test]
    fn ten_bit_is_scaled_not_truncated() {
        let mut f = frame8(2, 2, 0, 0, 0);
        f.bit_depth = 10;
        f.y = PixelData::U16(vec![940; 4]); // 10-bit limited white
        f.u = PixelData::U16(vec![512; 1]);
        f.v = PixelData::U16(vec![512; 1]);
        assert_eq!(
            first(&frame_to_rgba(&f, ColorSpec::default(), 2, 2).unwrap()),
            [255, 255, 255]
        );
        f.y = PixelData::U16(vec![64; 4]);
        assert_eq!(
            first(&frame_to_rgba(&f, ColorSpec::default(), 2, 2).unwrap()),
            [0, 0, 0]
        );
    }

    #[test]
    fn chroma_is_interpolated_across_a_boundary() {
        // 4x2 picture, chroma 2x1: left chroma neutral, right chroma strongly red.
        let mut f = frame8(4, 2, 126, 128, 128);
        f.u = PixelData::U8(vec![128, 128]);
        f.v = PixelData::U8(vec![128, 176]);
        let rgba = frame_to_rgba(&f, ColorSpec::default(), 4, 2).unwrap();
        let red = |x: usize| rgba[x * 4];
        assert!(
            red(0) < red(1) && red(1) < red(2) && red(2) < red(3),
            "{:?}",
            [red(0), red(1), red(2), red(3)]
        );
    }

    #[test]
    fn crops_to_the_requested_region() {
        let f = frame8(8, 8, 235, 128, 128);
        let rgba = frame_to_rgba(&f, ColorSpec::default(), 5, 3).unwrap();
        assert_eq!(rgba.len(), 5 * 3 * 4);
        assert!(frame_to_rgba(&f, ColorSpec::default(), 9, 3).is_err());
    }

    #[test]
    fn short_planes_are_rejected() {
        let mut f = frame8(8, 8, 1, 1, 1);
        f.u = PixelData::U8(vec![0; 3]);
        assert!(frame_to_rgba(&f, ColorSpec::default(), 8, 8).is_err());
        let mut f = frame8(8, 8, 1, 1, 1);
        f.v = PixelData::U16(vec![0; 16]);
        assert!(frame_to_rgba(&f, ColorSpec::default(), 8, 8).is_err());
    }

    #[test]
    fn alpha_plane_scaling() {
        let f = frame8(4, 2, 200, 128, 128);
        assert_eq!(frame_to_alpha(&f, 3, 2).unwrap(), vec![200; 6]);
        let mut f = frame8(2, 2, 0, 128, 128);
        f.bit_depth = 10;
        f.y = PixelData::U16(vec![0, 1023, 512, 1023]);
        f.u = PixelData::U16(vec![512]);
        f.v = PixelData::U16(vec![512]);
        assert_eq!(frame_to_alpha(&f, 2, 2).unwrap(), vec![0, 255, 128, 255]);
    }

    #[test]
    fn unsupported_matrices() {
        let n = |matrix| Nclx {
            primaries: 1,
            transfer: 1,
            matrix,
            full_range: false,
        };
        assert!(ColorSpec::from_nclx(Some(n(0))).is_err());
        assert!(ColorSpec::from_nclx(Some(n(8))).is_err());
        assert!(ColorSpec::from_nclx(Some(n(14))).is_err());
        assert_eq!(ColorSpec::from_nclx(None).unwrap(), ColorSpec::default());
    }
}
