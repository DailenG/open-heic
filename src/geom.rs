//! Transformative item properties: `clap` (crop), `irot` (rotation) and `imir` (mirror)
//! (ISO/IEC 23008-12 clause 6.5), applied in the order they are associated with the item.
//!
//! The operations are composed on an affine [`View`] over the source pixels and the output
//! is materialised once, so an image is copied at most one extra time however many
//! properties it carries.

use crate::error::Result;
use crate::props::Clap;

/// A transformative property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Clap(Clap),
    /// `irot`: anti-clockwise rotation by `angle * 90` degrees (`angle` in 0..=3).
    Rotate(u8),
    /// `imir`: axis 0 exchanges top and bottom, axis 1 exchanges left and right.
    Mirror(u8),
}

/// Interleaved 8-bit pixels (1 channel for an alpha plane, 4 for RGBA).
#[derive(Debug)]
pub(crate) struct Plane {
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub data: Vec<u8>,
}

/// Output pixel `(x, y)` is source pixel `(ox + x*dxx + y*dyx, oy + x*dxy + y*dyy)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    ox: i64,
    oy: i64,
    dxx: i64,
    dxy: i64,
    dyx: i64,
    dyy: i64,
    w: u32,
    h: u32,
}

impl View {
    fn identity(w: u32, h: u32) -> Self {
        View {
            ox: 0,
            oy: 0,
            dxx: 1,
            dxy: 0,
            dyx: 0,
            dyy: 1,
            w,
            h,
        }
    }

    fn apply(&mut self, op: Op) -> Result<()> {
        match op {
            Op::Clap(c) => {
                let (l, t, cw, ch) = c.rect(self.w, self.h)?;
                let (l, t) = (i64::from(l), i64::from(t));
                self.ox += l * self.dxx + t * self.dyx;
                self.oy += l * self.dxy + t * self.dyy;
                self.w = cw;
                self.h = ch;
            }
            Op::Rotate(angle) => {
                for _ in 0..(angle & 3) {
                    // out(x, y) = in(w - 1 - y, x)
                    let last = i64::from(self.w) - 1;
                    let v = *self;
                    self.ox = v.ox + last * v.dxx;
                    self.oy = v.oy + last * v.dxy;
                    self.dxx = v.dyx;
                    self.dxy = v.dyy;
                    self.dyx = -v.dxx;
                    self.dyy = -v.dxy;
                    self.w = v.h;
                    self.h = v.w;
                }
            }
            Op::Mirror(axis) => {
                if axis & 1 == 0 {
                    // top <-> bottom: out(x, y) = in(x, h - 1 - y)
                    let last = i64::from(self.h) - 1;
                    self.ox += last * self.dyx;
                    self.oy += last * self.dyy;
                    self.dyx = -self.dyx;
                    self.dyy = -self.dyy;
                } else {
                    // left <-> right: out(x, y) = in(w - 1 - x, y)
                    let last = i64::from(self.w) - 1;
                    self.ox += last * self.dxx;
                    self.oy += last * self.dxy;
                    self.dxx = -self.dxx;
                    self.dxy = -self.dxy;
                }
            }
        }
        Ok(())
    }
}

fn build_view(w: u32, h: u32, ops: &[Op]) -> Result<View> {
    let mut v = View::identity(w, h);
    for &op in ops {
        v.apply(op)?;
    }
    Ok(v)
}

/// Dimensions of a `w` x `h` image after `ops`.
pub(crate) fn output_dims(w: u32, h: u32, ops: &[Op]) -> Result<(u32, u32)> {
    let v = build_view(w, h, ops)?;
    Ok((v.w, v.h))
}

/// Apply `ops` to `plane`. Returns the input untouched when the result is identical.
pub(crate) fn apply_ops(plane: Plane, ops: &[Op]) -> Result<Plane> {
    let v = build_view(plane.width, plane.height, ops)?;
    if v == View::identity(plane.width, plane.height) {
        return Ok(plane);
    }
    Ok(materialize(&plane, &v))
}

fn materialize(src: &Plane, v: &View) -> Plane {
    let ch = src.channels;
    let (out_w, out_h) = (v.w as usize, v.h as usize);
    let mut out = vec![0u8; out_w * out_h * ch];
    let sw = i64::from(src.width);
    let c = ch as i64;
    let step_x = (v.dxy * sw + v.dxx) * c;
    let step_y = (v.dyy * sw + v.dyx) * c;
    let base = (v.oy * sw + v.ox) * c;
    if out_w == 0 || out_h == 0 {
        return Plane {
            width: v.w,
            height: v.h,
            channels: ch,
            data: out,
        };
    }
    let row_bytes = out_w * ch;
    for (y, dst) in out.chunks_exact_mut(row_bytes).enumerate() {
        let row_start = base + y as i64 * step_y;
        if v.dxx == 1 && v.dxy == 0 {
            let s = row_start as usize;
            dst.copy_from_slice(&src.data[s..s + row_bytes]);
        } else {
            let mut idx = row_start;
            for px in dst.chunks_exact_mut(ch) {
                let s = idx as usize;
                px.copy_from_slice(&src.data[s..s + ch]);
                idx += step_x;
            }
        }
    }
    Plane {
        width: v.w,
        height: v.h,
        channels: ch,
        data: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3x2 single-channel image:
    /// 1 2 3
    /// 4 5 6
    fn sample() -> Plane {
        Plane {
            width: 3,
            height: 2,
            channels: 1,
            data: vec![1, 2, 3, 4, 5, 6],
        }
    }

    fn run(ops: &[Op]) -> (u32, u32, Vec<u8>) {
        let p = apply_ops(sample(), ops).unwrap();
        (p.width, p.height, p.data)
    }

    #[test]
    fn identity_does_not_copy() {
        let p = sample();
        let ptr = p.data.as_ptr();
        let q = apply_ops(p, &[Op::Rotate(0)]).unwrap();
        assert_eq!(q.data.as_ptr(), ptr);
    }

    #[test]
    fn rotations_are_anticlockwise() {
        // 90 deg anticlockwise: right edge becomes the top row.
        assert_eq!(run(&[Op::Rotate(1)]), (2, 3, vec![3, 6, 2, 5, 1, 4]));
        assert_eq!(run(&[Op::Rotate(2)]), (3, 2, vec![6, 5, 4, 3, 2, 1]));
        // 270 deg anticlockwise = 90 deg clockwise.
        assert_eq!(run(&[Op::Rotate(3)]), (2, 3, vec![4, 1, 5, 2, 6, 3]));
        assert_eq!(run(&[Op::Rotate(4)]), (3, 2, vec![1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn mirrors() {
        // axis 0: top and bottom exchanged
        assert_eq!(run(&[Op::Mirror(0)]), (3, 2, vec![4, 5, 6, 1, 2, 3]));
        // axis 1: left and right exchanged
        assert_eq!(run(&[Op::Mirror(1)]), (3, 2, vec![3, 2, 1, 6, 5, 4]));
    }

    #[test]
    fn compositions_follow_the_listed_order() {
        // rotate then mirror(left/right) differs from mirror then rotate
        let a = run(&[Op::Rotate(1), Op::Mirror(1)]);
        let b = run(&[Op::Mirror(1), Op::Rotate(1)]);
        assert_eq!(a, (2, 3, vec![6, 3, 5, 2, 4, 1]));
        assert_eq!(b, (2, 3, vec![1, 4, 2, 5, 3, 6]));
        assert_eq!(
            run(&[Op::Rotate(1), Op::Rotate(3)]),
            (3, 2, vec![1, 2, 3, 4, 5, 6])
        );
        assert_eq!(
            run(&[Op::Mirror(0), Op::Mirror(0)]),
            (3, 2, vec![1, 2, 3, 4, 5, 6])
        );
    }

    fn crop(w: u32, h: u32, x: i32, y: i32) -> Op {
        Op::Clap(Clap {
            width: (w, 1),
            height: (h, 1),
            horiz_off: (x, 1),
            vert_off: (y, 1),
        })
    }

    #[test]
    fn clap_crops_before_and_after_rotation() {
        // 2x2 at the top-left of the 3x2 image: centre offset -0.5 horizontally is not
        // integral, so use width 2 with offset (-1/2) -> left = (3-2)/2 - 1/2 = 0.
        let c = Op::Clap(Clap {
            width: (2, 1),
            height: (2, 1),
            horiz_off: (-1, 2),
            vert_off: (0, 1),
        });
        assert_eq!(run(&[c]), (2, 2, vec![1, 2, 4, 5]));
        // single column on the right, then rotated: 1x2 column [3,6] -> 2x1 row
        let right = crop(1, 2, 1, 0);
        assert_eq!(run(&[right]), (1, 2, vec![3, 6]));
        assert_eq!(run(&[right, Op::Rotate(1)]), (2, 1, vec![3, 6]));
        assert_eq!(run(&[right, Op::Rotate(3)]), (2, 1, vec![6, 3]));
        // rotate first, then crop in the rotated frame (2x3 image: [3,6],[2,5],[1,4])
        assert_eq!(run(&[Op::Rotate(1), crop(2, 1, 0, -1)]), (2, 1, vec![3, 6]));
        assert_eq!(run(&[Op::Rotate(1), crop(2, 1, 0, 1)]), (2, 1, vec![1, 4]));
    }

    #[test]
    fn clap_outside_image_is_rejected() {
        assert!(apply_ops(sample(), &[crop(4, 2, 0, 0)]).is_err());
        assert!(output_dims(3, 2, &[crop(3, 3, 0, 0)]).is_err());
        assert_eq!(output_dims(3, 2, &[Op::Rotate(1)]).unwrap(), (2, 3));
    }

    #[test]
    fn multichannel_pixels_move_whole() {
        let p = Plane {
            width: 2,
            height: 1,
            channels: 4,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let q = apply_ops(p, &[Op::Mirror(1)]).unwrap();
        assert_eq!(q.data, vec![5, 6, 7, 8, 1, 2, 3, 4]);
        let p = Plane {
            width: 2,
            height: 1,
            channels: 4,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let q = apply_ops(p, &[Op::Rotate(1)]).unwrap();
        assert_eq!((q.width, q.height), (1, 2));
        assert_eq!(q.data, vec![5, 6, 7, 8, 1, 2, 3, 4]);
    }
}
