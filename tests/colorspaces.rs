//! Colour matrices, ranges and bit depths on real x265 streams (`tests/fixtures/*.hevc`).
//!
//! Each stream is a 64x48 picture of four flat quadrants (terracotta, green, blue, mustard:
//! deliberately not saturated, so a wrong matrix or range cannot hide behind clamping)
//! encoded from RGB with a known matrix/range/bit depth, so decoding with the matching
//! `colr` must return the original colours, and decoding with a different one must not.

mod common;

use common::*;
use open_heic::{Codec, Error, Limits, decode, info};

/// The source colours of the four quadrants (top-left, top-right, bottom-left, bottom-right).
const QUADRANTS: [[u8; 3]; 4] = [
    [0xc8, 0x3c, 0x28],
    [0x3c, 0xb4, 0x5a],
    [0x32, 0x50, 0xc8],
    [0xdc, 0xc8, 0x3c],
];

fn file(name: &str, chroma: u8, depth: u8, colr: Option<Vec<u8>>) -> Vec<u8> {
    let h = stream(name, chroma, depth);
    let mut item = Item::hevc(1, &h);
    if let Some(c) = colr {
        item = item.with(c, false);
    }
    Spec::new(1, vec![item]).build()
}

fn max_error(img: &open_heic::Image) -> i32 {
    let want = QUADRANTS;
    quadrant_centres(img)
        .iter()
        .zip(want)
        .map(|(p, w)| {
            (0..3)
                .map(|c| (i32::from(p[c]) - i32::from(w[c])).abs())
                .max()
                .unwrap()
        })
        .max()
        .unwrap()
}

fn error_of(name: &str, depth: u8, colr: Option<Vec<u8>>) -> i32 {
    let img = decode(&file(name, 1, depth, colr), &Limits::default()).unwrap();
    assert_eq!((img.width, img.height), (64, 48));
    max_error(&img)
}

const TOL: i32 = 8;

#[test]
fn bt601_limited_is_the_default() {
    assert!(error_of("quad601_8", 8, None) <= TOL);
    for matrix in [5, 6] {
        assert!(
            error_of("quad601_8", 8, Some(p_colr_nclx(1, 13, matrix, false))) <= TOL,
            "matrix {matrix}"
        );
    }
    // an ICC profile without nclx keeps the default
    assert!(error_of("quad601_8", 8, Some(p_colr_icc())) <= TOL);
}

#[test]
fn bt709_limited() {
    assert!(error_of("quad709_8", 8, Some(p_colr_nclx(1, 1, 1, false))) <= TOL);
    // decoded with the default BT.601 the colours are visibly off
    assert!(error_of("quad709_8", 8, None) > 15);
}

#[test]
fn bt601_full_range() {
    assert!(error_of("quad601full_8", 8, Some(p_colr_nclx(1, 13, 6, true))) <= TOL);
    assert!(error_of("quad601full_8", 8, Some(p_colr_nclx(1, 13, 6, false))) > 15);
    assert!(error_of("quad601full_8", 8, None) > 15);
}

#[test]
fn bt2020_ten_bit() {
    let f = file("quad2020_10", 1, 10, Some(p_colr_nclx(9, 16, 9, false)));
    let i = info(&f).unwrap();
    assert_eq!(
        (i.codec, i.bit_depth, i.width, i.height),
        (Codec::Hevc, 10, 64, 48)
    );
    let img = decode(&f, &Limits::default()).unwrap();
    assert!(max_error(&img) <= TOL, "error {}", max_error(&img));
    // BT.2020 and BT.601 luma weights differ by less than the other pairs, but 10-bit data
    // read with the wrong matrix is still clearly worse than with the right one
    let wrong = error_of("quad2020_10", 10, Some(p_colr_nclx(9, 16, 6, false)));
    assert!(wrong > max_error(&img) + 4, "wrong matrix error {wrong}");
}

#[test]
fn monochrome_primary_is_unsupported_not_garbage() {
    let f = file("quad601_gray", 0, 8, None);
    assert!(
        matches!(decode(&f, &Limits::default()), Err(Error::Unsupported(m)) if m.contains("monochrome"))
    );
    assert_eq!(info(&f).unwrap().codec, Codec::Hevc);
}

#[test]
fn monochrome_alpha_plane_is_ignored_and_the_image_stays_opaque() {
    // This is how Apple writes alpha (a 4:0:0 HEVC auxiliary image); rust_h265 cannot decode
    // 4:0:0, so the picture is returned fully opaque and has_alpha still reports the plane.
    let color = stream("quad601_8", 1, 8);
    let gray = stream("quad601_gray", 0, 8);
    let mut alpha = Item::hevc(2, &gray);
    alpha.props.push((p_auxc(ALPHA_URN), true));
    let f = Spec::new(1, vec![Item::hevc(1, &color), alpha])
        .refer(b"auxl", 2, &[1])
        .build();
    assert!(info(&f).unwrap().has_alpha);
    let img = decode(&f, &Limits::default()).unwrap();
    assert!(img.rgba.chunks_exact(4).all(|p| p[3] == 255));
    assert!(max_error(&img) <= TOL);
}

#[test]
fn four_two_zero_alpha_plane_is_used() {
    // An alpha plane stored as ordinary 4:2:0 video: its luma is the alpha value. The
    // full-range stream's luma is 0.299 R + 0.587 G + 0.114 B of the four colours.
    let color = stream("quad601_8", 1, 8);
    let a = stream("quad601full_8", 1, 8); // full-range luma: 0.299 R + 0.587 G + 0.114 B
    let mut alpha = Item::hevc(2, &a);
    alpha.props.push((p_auxc(ALPHA_URN), true));
    let f = Spec::new(1, vec![Item::hevc(1, &color), alpha])
        .refer(b"auxl", 2, &[1])
        .build();
    let img = decode(&f, &Limits::default()).unwrap();
    let want = [100i32, 134, 85, 190];
    for (p, w) in quadrant_centres(&img).iter().zip(want) {
        assert!((i32::from(p[3]) - w).abs() <= 6, "alpha {} vs {w}", p[3]);
    }
}

#[test]
fn conformance_window_crops_the_coded_picture() {
    // 50x34 is coded as 56x40 with a conformance window
    let h = stream_sized("quad_50x34", 50, 34);
    let f = Spec::new(1, vec![Item::hevc(1, &h)]).build();
    let img = decode(&f, &Limits::default()).unwrap();
    assert_eq!((img.width, img.height), (50, 34));
    assert!(max_error(&img) <= TOL, "error {}", max_error(&img));
    // the last row and column belong to the picture, not to padding
    let p = px(&img, 49, 33);
    assert!(
        (i32::from(p[0]) - 0xdc).abs() <= TOL && (i32::from(p[2]) - 0x3c).abs() <= TOL,
        "{p:?}"
    );
}
