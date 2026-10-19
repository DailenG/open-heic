//! Tests on real camera files that cannot be committed (personal photographs).
//!
//! Run with:
//!
//! ```text
//! OPEN_HEIC_LOCAL_DIR=C:\wt\heicspike\in cargo test --release -- --ignored
//! ```
//!
//! The directory must contain `classic-car.heic` (iPhone, 4032x3024 grid, `irot` 3,
//! 320x240 thumbnail) and `chef-with-trumpet.heic` (iPhone, 4032x3024 grid, no thumbnail
//! item). `IMG_0074.heic` (a JPEG with a `.heic` name) and `quad.heic` are used when present.

use std::path::PathBuf;

use open_heic::{Codec, Error, Image, Limits, decode, decode_thumbnail, info};

fn local(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("OPEN_HEIC_LOCAL_DIR")
        .expect("set OPEN_HEIC_LOCAL_DIR to the directory with the local HEIC files");
    std::fs::read(PathBuf::from(dir).join(name)).ok()
}

fn required(name: &str) -> Vec<u8> {
    local(name).unwrap_or_else(|| panic!("{name} not found in OPEN_HEIC_LOCAL_DIR"))
}

/// Mean RGB of the whole image, sampled on a coarse grid.
fn mean_rgb(img: &Image) -> [f64; 3] {
    let (mut sum, mut n) = ([0f64; 3], 0f64);
    let step_x = (img.width / 64).max(1) as usize;
    let step_y = (img.height / 64).max(1) as usize;
    for y in (0..img.height as usize).step_by(step_y) {
        for x in (0..img.width as usize).step_by(step_x) {
            let i = (y * img.width as usize + x) * 4;
            for (c, s) in sum.iter_mut().enumerate() {
                *s += f64::from(img.rgba[i + c]);
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n)
}

fn assert_not_flat(img: &Image) {
    let first = &img.rgba[..3];
    assert!(
        img.rgba
            .chunks_exact(4)
            .step_by(997)
            .any(|p| &p[..3] != first),
        "image is a single flat colour"
    );
}

#[test]
#[ignore = "needs OPEN_HEIC_LOCAL_DIR"]
fn classic_car_is_rotated_and_decodes() {
    let data = required("classic-car.heic");
    let i = info(&data).unwrap();
    // irot 3 turns the stored 4032x3024 grid into a portrait picture
    assert_eq!(
        (i.width, i.height, i.codec, i.bit_depth),
        (3024, 4032, Codec::Hevc, 8)
    );
    assert!(i.has_thumbnail);
    assert!(!i.has_alpha);

    let full = decode(&data, &Limits::default()).unwrap();
    assert_eq!((full.width, full.height), (3024, 4032));
    assert_eq!(full.rgba.len(), 3024 * 4032 * 4);
    assert_not_flat(&full);
    assert!(full.rgba.chunks_exact(4).step_by(1009).all(|p| p[3] == 255));

    // the 320x240 thumbnail, rotated by its own irot
    let thumb = decode_thumbnail(&data, 256, &Limits::default()).unwrap();
    assert_eq!((thumb.width, thumb.height), (240, 320));
    assert_not_flat(&thumb);

    // the thumbnail and the full image must show the same picture the same way up
    let (a, b) = (mean_rgb(&thumb), mean_rgb(&full));
    for c in 0..3 {
        assert!(
            (a[c] - b[c]).abs() < 12.0,
            "mean colour differs: thumb {a:?} vs full {b:?}"
        );
    }
    // compare the upper and lower halves: the sky is brighter than the road in this photo,
    // which fails if the thumbnail were rotated differently from the full image
    let half = |img: &Image, top: bool| {
        let mut sum = 0f64;
        let (h0, h1) = if top {
            (0, img.height / 2)
        } else {
            (img.height / 2, img.height)
        };
        let mut n = 0f64;
        for y in (h0..h1).step_by((img.height / 64).max(1) as usize) {
            for x in (0..img.width).step_by((img.width / 64).max(1) as usize) {
                let i = ((y * img.width + x) * 4) as usize;
                sum += f64::from(img.rgba[i])
                    + f64::from(img.rgba[i + 1])
                    + f64::from(img.rgba[i + 2]);
                n += 3.0;
            }
        }
        sum / n
    };
    assert_eq!(
        half(&thumb, true) > half(&thumb, false),
        half(&full, true) > half(&full, false)
    );
}

#[test]
#[ignore = "needs OPEN_HEIC_LOCAL_DIR"]
fn chef_decodes_and_thumbnail_falls_back_to_the_primary() {
    let data = required("chef-with-trumpet.heic");
    let i = info(&data).unwrap();
    assert_eq!((i.width, i.height, i.codec), (4032, 3024, Codec::Hevc));
    assert!(!i.has_thumbnail);

    let full = decode(&data, &Limits::default()).unwrap();
    assert_eq!((full.width, full.height), (4032, 3024));
    assert_not_flat(&full);

    // no thumbnail item: decode_thumbnail returns the primary unchanged
    let thumb = decode_thumbnail(&data, 256, &Limits::default()).unwrap();
    assert_eq!(thumb, full);
}

#[test]
#[ignore = "needs OPEN_HEIC_LOCAL_DIR"]
fn real_files_respect_limits() {
    let data = required("classic-car.heic");
    let tight = Limits {
        max_pixels: 1_000_000,
        ..Limits::default()
    };
    assert!(matches!(
        decode(&data, &tight),
        Err(Error::LimitExceeded(_))
    ));
    // the 320x240 thumbnail fits in the same budget
    assert!(decode_thumbnail(&data, 256, &tight).is_ok());
    let tiny_memory = Limits {
        max_memory_bytes: 32 << 20,
        ..Limits::default()
    };
    assert!(matches!(
        decode(&data, &tiny_memory),
        Err(Error::LimitExceeded(_))
    ));
}

#[test]
#[ignore = "needs OPEN_HEIC_LOCAL_DIR"]
fn jpeg_renamed_heic_is_not_heif() {
    let Some(data) = local("IMG_0074.heic") else {
        return;
    };
    assert_eq!(info(&data), Err(Error::NotHeif));
    assert_eq!(decode(&data, &Limits::default()), Err(Error::NotHeif));
}

#[test]
#[ignore = "needs OPEN_HEIC_LOCAL_DIR"]
fn quad_from_the_local_directory_decodes() {
    let Some(data) = local("quad.heic") else {
        return;
    };
    let img = decode(&data, &Limits::default()).unwrap();
    assert_eq!((img.width, img.height), (160, 120));
}
