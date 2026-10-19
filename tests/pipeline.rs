//! End-to-end tests on the committed quad fixture and on files assembled from its tile.

mod common;

use common::*;
use open_heic::{Codec, Error, Limits, decode, decode_thumbnail, info};

fn lim() -> Limits {
    Limits::default()
}

#[test]
fn quad_decodes_with_the_right_colours() {
    let img = decode(QUAD, &lim()).unwrap();
    assert_eq!((img.width, img.height), (160, 120));
    assert_eq!(img.rgba.len(), 160 * 120 * 4);
    assert_quadrants(&img, [RED, GREEN, BLUE, YELLOW]);
    assert!(img.rgba.chunks_exact(4).all(|p| p[3] == 255));
}

#[test]
fn quad_info() {
    let i = info(QUAD).unwrap();
    assert_eq!((i.width, i.height), (160, 120));
    assert_eq!(i.codec, Codec::Hevc);
    assert_eq!(i.bit_depth, 8);
    assert!(!i.has_alpha);
    assert!(!i.has_thumbnail);
}

#[test]
fn quad_without_thumbnail_falls_back_to_the_primary() {
    let t = decode_thumbnail(QUAD, 64, &lim()).unwrap();
    assert_eq!((t.width, t.height), (160, 120));
}

// ---- container variants -------------------------------------------------------------------

#[test]
fn container_variants_all_decode() {
    for iloc in [0u8, 1, 2] {
        for infe in [2u8, 3] {
            for (ipma, wide) in [(0u8, false), (1, false), (0, true), (1, true)] {
                for iref in [0u8, 1] {
                    let h = quad_hevc();
                    let mut spec = Spec::new(1, vec![Item::hevc(1, &h)]);
                    spec.opts = Opts {
                        brand: *b"heic",
                        iloc_version: iloc,
                        infe_version: infe,
                        ipma_version: ipma,
                        ipma_wide_assoc: wide,
                        iref_version: iref,
                    };
                    let img = decode(&spec.build(), &lim()).unwrap_or_else(|e| {
                        panic!(
                            "iloc v{iloc} infe v{infe} ipma v{ipma} wide={wide} iref v{iref}: {e}"
                        )
                    });
                    assert_quadrants(&img, [RED, GREEN, BLUE, YELLOW]);
                }
            }
        }
    }
}

#[test]
fn item_data_in_idat_and_in_many_extents() {
    let h = quad_hevc();
    let idat = Spec::new(1, vec![Item::hevc(1, &h).idat()]).build();
    assert_quadrants(&decode(&idat, &lim()).unwrap(), [RED, GREEN, BLUE, YELLOW]);
    let split = Spec::new(1, vec![Item::hevc(1, &h).split(5)]).build();
    assert_quadrants(&decode(&split, &lim()).unwrap(), [RED, GREEN, BLUE, YELLOW]);
    let idat_split = Spec::new(1, vec![Item::hevc(1, &h).idat().split(3)]).build();
    assert_quadrants(
        &decode(&idat_split, &lim()).unwrap(),
        [RED, GREEN, BLUE, YELLOW],
    );
}

#[test]
fn every_accepted_brand_is_heif() {
    for brand in [
        b"heic", b"heix", b"heim", b"heis", b"mif1", b"msf1", b"hevc",
    ] {
        let h = quad_hevc();
        let mut spec = Spec::new(1, vec![Item::hevc(1, &h)]);
        spec.opts.brand = *brand;
        decode(&spec.build(), &lim())
            .unwrap_or_else(|e| panic!("{}: {e}", String::from_utf8_lossy(brand)));
    }
}

#[test]
fn unknown_brand_is_not_heif() {
    let h = quad_hevc();
    let mut spec = Spec::new(1, vec![Item::hevc(1, &h)]);
    spec.opts.brand = *b"isom";
    let mut bytes = spec.build();
    // overwrite the compatible brand `mif1` too
    let pos = bytes.windows(4).position(|w| w == b"mif1").unwrap();
    bytes[pos..pos + 4].copy_from_slice(b"iso2");
    assert_eq!(info(&bytes), Err(Error::NotHeif));
}

#[test]
fn unknown_non_essential_property_is_ignored_but_essential_is_unsupported() {
    let weird = bx(b"zzzz", &[1, 2, 3]);
    let ok = single(&[(weird.clone(), false)]);
    assert_quadrants(&decode(&ok, &lim()).unwrap(), [RED, GREEN, BLUE, YELLOW]);
    let bad = single(&[(weird, true)]);
    assert!(matches!(decode(&bad, &lim()), Err(Error::Unsupported(_))));
    // info() reads the same properties
    assert!(matches!(info(&bad), Err(Error::Unsupported(_))));
}

#[test]
fn icc_profile_and_pixi_are_accepted() {
    let f = single(&[(p_colr_icc(), false), (p_pixi(8), false)]);
    assert_quadrants(&decode(&f, &lim()).unwrap(), [RED, GREEN, BLUE, YELLOW]);
}

#[test]
fn not_heif_inputs() {
    assert_eq!(info(&[]), Err(Error::NotHeif));
    assert_eq!(
        info(b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01\x01"),
        Err(Error::NotHeif)
    );
    assert_eq!(
        info(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"),
        Err(Error::NotHeif)
    );
    assert_eq!(
        decode(b"GIF89a....................", &lim()),
        Err(Error::NotHeif)
    );
    assert_eq!(
        decode_thumbnail(&[0u8; 100], 64, &lim()),
        Err(Error::NotHeif)
    );
    // a plain mp4-style file
    let mut mp4 = bx(b"ftyp", b"isom\0\0\0\0isomiso2mp41");
    mp4.extend(bx(b"moov", &[0; 8]));
    assert_eq!(info(&mp4), Err(Error::NotHeif));
}

#[test]
fn truncated_inputs_are_errors_never_panics() {
    for len in 0..QUAD.len() {
        let _ = info(&QUAD[..len]);
        let _ = decode(&QUAD[..len], &lim());
    }
}

#[test]
fn truncated_mdat_is_malformed() {
    // cut inside the picture data
    let cut = &QUAD[..QUAD.len() - 20];
    assert!(matches!(decode(cut, &lim()), Err(Error::Malformed(_))));
}

// ---- unsupported codecs ----------------------------------------------------------------------

fn coded_only(kind: &[u8; 4]) -> Vec<u8> {
    let mut it = Item::new(1, kind);
    it.data = vec![0; 32];
    it.props = vec![(p_ispe(64, 48), false), (p_pixi(10), false)];
    Spec::new(1, vec![it]).build()
}

#[test]
fn av1_jpeg_and_uncompressed_are_detected_and_unsupported() {
    for (kind, codec) in [
        (b"av01", Codec::Av1),
        (b"jpeg", Codec::Jpeg),
        (b"unci", Codec::Uncompressed),
    ] {
        let f = coded_only(kind);
        let i = info(&f).unwrap();
        assert_eq!(i.codec, codec);
        assert_eq!((i.width, i.height, i.bit_depth), (64, 48, 10));
        assert!(
            matches!(decode(&f, &lim()), Err(Error::Unsupported(_))),
            "{kind:?}"
        );
        assert!(
            matches!(decode_thumbnail(&f, 32, &lim()), Err(Error::Unsupported(_))),
            "{kind:?}"
        );
    }
    assert_eq!(
        info(&coded_only(b"abcd")).unwrap().codec,
        Codec::Other(*b"abcd")
    );
}

#[test]
fn avif_brand_file_reports_av1() {
    let mut it = Item::new(1, b"av01");
    it.data = vec![0; 8];
    it.props = vec![(p_ispe(32, 32), false)];
    let mut spec = Spec::new(1, vec![it]);
    spec.opts.brand = *b"avif";
    let f = spec.build();
    assert_eq!(info(&f).unwrap().codec, Codec::Av1);
    assert!(matches!(decode(&f, &lim()), Err(Error::Unsupported(m)) if m.contains("AV1")));
}

#[test]
fn overlay_is_unsupported_and_grid_of_jpeg_tiles_reports_jpeg() {
    let q = quad_hevc();
    let mut ov = Item::new(1, b"iovl");
    ov.data = vec![0; 16];
    ov.props = vec![(p_ispe(160, 120), false)];
    let f = Spec::new(1, vec![ov, Item::hevc(2, &q)])
        .refer(b"dimg", 1, &[2])
        .build();
    assert_eq!(info(&f).unwrap().codec, Codec::Hevc);
    assert!(matches!(decode(&f, &lim()), Err(Error::Unsupported(m)) if m.contains("overlay")));

    let mut spec = grid(1, 1, 64, 48, true);
    spec.items[1].kind = *b"jpeg";
    spec.items[1].props = vec![(p_ispe(64, 48), false)];
    let f = spec.build();
    assert_eq!(info(&f).unwrap().codec, Codec::Jpeg);
    assert!(matches!(decode(&f, &lim()), Err(Error::Unsupported(_))));
}

// ---- grid ------------------------------------------------------------------------------------

#[test]
fn grid_2x2_with_idat_descriptor() {
    let f = grid(2, 2, 320, 240, true).build();
    let i = info(&f).unwrap();
    assert_eq!(
        (i.width, i.height, i.codec, i.bit_depth),
        (320, 240, Codec::Hevc, 8)
    );
    let img = decode(&f, &lim()).unwrap();
    assert_eq!((img.width, img.height), (320, 240));
    // every tile shows the same four quadrants
    for (x0, y0) in [(0, 0), (160, 0), (0, 120), (160, 120)] {
        for (dx, dy, want) in [
            (40, 30, RED),
            (120, 30, GREEN),
            (40, 90, BLUE),
            (120, 90, YELLOW),
        ] {
            assert_near(px(&img, x0 + dx, y0 + dy), want, 12);
        }
    }
}

#[test]
fn grid_with_file_descriptor_wide_fields_and_clipping() {
    // descriptor in the file, output clipped to 250x200 (tiles 160x120 cover it)
    let mut spec = grid(2, 2, 250, 200, false);
    spec.items[0].data = grid_descriptor(2, 2, 250, 200, true);
    let f = spec.build();
    let img = decode(&f, &lim()).unwrap();
    assert_eq!((img.width, img.height), (250, 200));
    assert_near(px(&img, 40, 30), RED, 12);
    assert_near(px(&img, 200, 30), RED, 12); // second tile: x 160..250 is its left part
    assert_near(px(&img, 249, 199), YELLOW, 12); // tile (1,1), local (89, 79): bottom-right quadrant
    assert_near(px(&img, 245, 90), YELLOW, 12); // tile (1,0), local (85, 90)
}

#[test]
fn grid_irot_applies_to_the_whole_grid() {
    let spec = grid(1, 2, 320, 120, true);
    let mut spec = spec;
    spec.items[0].props.push((p_irot(1), true));
    let img = decode(&spec.build(), &lim()).unwrap();
    assert_eq!((img.width, img.height), (120, 320));
    let i = info(&spec.build()).unwrap();
    assert_eq!((i.width, i.height), (120, 320));
}

#[test]
fn grid_rejects_inconsistent_descriptors() {
    // wrong tile count
    let mut spec = grid(2, 2, 320, 240, true);
    spec.refs[0].2.pop();
    assert!(matches!(
        decode(&spec.build(), &lim()),
        Err(Error::Malformed(_))
    ));
    // tiles that cannot cover the output
    let spec = grid(1, 1, 400, 120, true);
    assert!(matches!(
        decode(&spec.build(), &lim()),
        Err(Error::Malformed(_))
    ));
    // a tile that lies completely outside the output
    let spec = grid(2, 1, 160, 120, true);
    assert!(matches!(
        decode(&spec.build(), &lim()),
        Err(Error::Malformed(_))
    ));
    // no dimg reference at all
    let mut spec = grid(1, 1, 160, 120, true);
    spec.refs.clear();
    assert!(matches!(
        decode(&spec.build(), &lim()),
        Err(Error::Malformed(_))
    ));
    // tiles of different sizes
    let mut spec = grid(1, 2, 300, 120, true);
    spec.items[2].props[1] = (p_ispe(140, 120), false);
    assert!(matches!(
        decode(&spec.build(), &lim()),
        Err(Error::Malformed(_))
    ));
    // unsupported descriptor version
    let mut spec = grid(1, 1, 160, 120, true);
    spec.items[0].data[0] = 1;
    assert!(matches!(
        decode(&spec.build(), &lim()),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn identity_derivation_applies_its_own_transforms() {
    let q = quad_hevc();
    let mut iden = Item::new(1, b"iden");
    iden.props = vec![(p_irot(2), true)];
    iden.no_location = true;
    let f = Spec::new(1, vec![iden, Item::hevc(2, &q)])
        .refer(b"dimg", 1, &[2])
        .build();
    let i = info(&f).unwrap();
    assert_eq!((i.width, i.height, i.codec), (160, 120, Codec::Hevc));
    let img = decode(&f, &lim()).unwrap();
    assert_quadrants(&img, [YELLOW, BLUE, GREEN, RED]);
}

#[test]
fn identity_loops_are_rejected() {
    let mut a = Item::new(1, b"iden");
    a.no_location = true;
    let mut b = Item::new(2, b"iden");
    b.no_location = true;
    let f = Spec::new(1, vec![a, b])
        .refer(b"dimg", 1, &[2])
        .refer(b"dimg", 2, &[1])
        .build();
    assert!(matches!(decode(&f, &lim()), Err(Error::Malformed(_))));
    assert!(matches!(info(&f), Err(Error::Malformed(_))));
}

// ---- orientation and crop ----------------------------------------------------------------

fn with_props(props: &[(Vec<u8>, bool)]) -> open_heic::Image {
    decode(&single(props), &lim()).unwrap()
}

#[test]
fn irot_rotates_anticlockwise() {
    // original: TL red, TR green, BL blue, BR yellow
    let r1 = with_props(&[(p_irot(1), true)]);
    assert_eq!((r1.width, r1.height), (120, 160));
    assert_quadrants(&r1, [GREEN, YELLOW, RED, BLUE]);
    let r2 = with_props(&[(p_irot(2), true)]);
    assert_eq!((r2.width, r2.height), (160, 120));
    assert_quadrants(&r2, [YELLOW, BLUE, GREEN, RED]);
    let r3 = with_props(&[(p_irot(3), true)]);
    assert_eq!((r3.width, r3.height), (120, 160));
    assert_quadrants(&r3, [BLUE, RED, YELLOW, GREEN]);
    let r0 = with_props(&[(p_irot(0), true)]);
    assert_quadrants(&r0, [RED, GREEN, BLUE, YELLOW]);
}

#[test]
fn imir_mirrors() {
    let top_bottom = with_props(&[(p_imir(0), true)]);
    assert_quadrants(&top_bottom, [BLUE, YELLOW, RED, GREEN]);
    let left_right = with_props(&[(p_imir(1), true)]);
    assert_quadrants(&left_right, [GREEN, RED, YELLOW, BLUE]);
}

#[test]
fn clap_crops() {
    // left half: width 80, centred offset -40
    let left = with_props(&[(p_clap(80, 1, 120, 1, -40, 1, 0, 1), true)]);
    assert_eq!((left.width, left.height), (80, 120));
    assert_near(px(&left, 20, 30), RED, 12);
    assert_near(px(&left, 60, 30), RED, 12);
    assert_near(px(&left, 20, 90), BLUE, 12);
    // bottom-right 40x30 corner
    let corner = with_props(&[(p_clap(40, 1, 30, 1, 60, 1, 45, 1), true)]);
    assert_eq!((corner.width, corner.height), (40, 30));
    assert_near(px(&corner, 20, 15), YELLOW, 12);
    // aperture outside the image
    let bad = single(&[(p_clap(200, 1, 120, 1, 0, 1, 0, 1), true)]);
    assert!(matches!(decode(&bad, &lim()), Err(Error::Malformed(_))));
}

#[test]
fn transforms_apply_in_ipma_order() {
    // clap (left half 80x120) then irot 1 -> 120x80, content: rotated left half
    let img = with_props(&[
        (p_clap(80, 1, 120, 1, -40, 1, 0, 1), true),
        (p_irot(1), true),
    ]);
    assert_eq!((img.width, img.height), (120, 80));
    // left half is red over blue; rotated anticlockwise: red moves to the left, blue to the right
    assert_near(px(&img, 30, 40), RED, 12);
    assert_near(px(&img, 90, 40), BLUE, 12);
    // irot then imir: rotate 90 (GREEN YELLOW / RED BLUE) then mirror left/right
    let img = with_props(&[(p_irot(1), true), (p_imir(1), true)]);
    assert_quadrants(&img, [YELLOW, GREEN, BLUE, RED]);
    // and the other order gives a different picture
    let img = with_props(&[(p_imir(1), true), (p_irot(1), true)]);
    assert_quadrants(&img, [RED, BLUE, GREEN, YELLOW]);
    let i = info(&single(&[
        (p_clap(80, 1, 120, 1, -40, 1, 0, 1), true),
        (p_irot(1), true),
    ]))
    .unwrap();
    assert_eq!((i.width, i.height), (120, 80));
}

// ---- colour ------------------------------------------------------------------------------------

#[test]
fn colr_matrix_changes_the_result_and_default_is_bt601() {
    let default = decode(QUAD, &lim()).unwrap();
    let explicit601 = with_props(&[(p_colr_nclx(1, 13, 6, false), false)]);
    assert_eq!(default.rgba, explicit601.rgba);
    let as709 = with_props(&[(p_colr_nclx(1, 13, 1, false), false)]);
    assert_ne!(default.rgba, as709.rgba);
    let full = with_props(&[(p_colr_nclx(1, 13, 6, true), false)]);
    assert_ne!(default.rgba, full.rgba);
    let unsupported = single(&[(p_colr_nclx(1, 13, 8, false), false)]);
    assert!(matches!(
        decode(&unsupported, &lim()),
        Err(Error::Unsupported(_))
    ));
}

// ---- alpha ------------------------------------------------------------------------------------

fn with_alpha() -> Vec<u8> {
    let q = quad_hevc();
    let mut alpha = Item::hevc(2, &q);
    alpha.props.push((p_auxc(ALPHA_URN), true));
    let primary = Item::hevc(1, &q);
    Spec::new(1, vec![primary, alpha])
        .refer(b"auxl", 2, &[1])
        .build()
}

#[test]
fn alpha_plane_becomes_straight_alpha() {
    let f = with_alpha();
    let i = info(&f).unwrap();
    assert!(i.has_alpha);
    let img = decode(&f, &lim()).unwrap();
    // the stand-in alpha plane is the luma of the quad picture (BT.601 limited luma of
    // red, green, blue, yellow = 81, 145, 41, 210), and colours are NOT premultiplied
    let want = [81i32, 145, 41, 210];
    for (p, a) in quadrant_centres(&img).iter().zip(want) {
        assert!((i32::from(p[3]) - a).abs() <= 6, "alpha {} vs {a}", p[3]);
    }
    assert_quadrants(&img, [RED, GREEN, BLUE, YELLOW]);
}

#[test]
fn alpha_follows_the_orientation_of_the_image() {
    let q = quad_hevc();
    let mut alpha = Item::hevc(2, &q);
    alpha.props.push((p_auxc(ALPHA_URN), true));
    let primary = Item::hevc(1, &q).with(p_irot(1), true);
    let f = Spec::new(1, vec![primary, alpha])
        .refer(b"auxl", 2, &[1])
        .build();
    let img = decode(&f, &lim()).unwrap();
    assert_eq!((img.width, img.height), (120, 160));
    // rotated: TL green, TR yellow, BL red, BR blue; alpha rotates with them
    let want = [145i32, 210, 81, 41];
    for (p, a) in quadrant_centres(&img).iter().zip(want) {
        assert!((i32::from(p[3]) - a).abs() <= 6, "alpha {} vs {a}", p[3]);
    }
}

#[test]
fn alpha_of_a_different_size_is_resampled() {
    let q = quad_hevc();
    let mut alpha = Item::hevc(2, &q);
    alpha.props = vec![
        (p_hvcc(&q), true),
        (p_ispe(80, 60), false),
        (p_auxc(ALPHA_URN), true),
    ];
    let f = Spec::new(1, vec![Item::hevc(1, &q), alpha])
        .refer(b"auxl", 2, &[1])
        .build();
    let img = decode(&f, &lim()).unwrap();
    assert_eq!((img.width, img.height), (160, 120));
    // 80x60 crop of the alpha picture is its top-left (red) quadrant only: constant luma 81
    for p in quadrant_centres(&img) {
        assert!((i32::from(p[3]) - 81).abs() <= 6);
    }
}

#[test]
fn a_non_alpha_auxiliary_image_is_not_alpha() {
    let q = quad_hevc();
    let mut depth = Item::hevc(2, &q);
    depth
        .props
        .push((p_auxc("urn:mpeg:hevc:2015:auxid:2"), true));
    let f = Spec::new(1, vec![Item::hevc(1, &q), depth])
        .refer(b"auxl", 2, &[1])
        .build();
    assert!(!info(&f).unwrap().has_alpha);
    assert!(
        decode(&f, &lim())
            .unwrap()
            .rgba
            .chunks_exact(4)
            .all(|p| p[3] == 255)
    );
}

#[test]
fn undecodable_alpha_leaves_the_image_opaque() {
    let q = quad_hevc();
    let mut alpha = Item::new(2, b"av01"); // an alpha plane in a codec we cannot decode
    alpha.data = vec![0; 8];
    alpha.props = vec![(p_ispe(160, 120), false), (p_auxc(ALPHA_URN), true)];
    let f = Spec::new(1, vec![Item::hevc(1, &q), alpha])
        .refer(b"auxl", 2, &[1])
        .build();
    assert!(info(&f).unwrap().has_alpha);
    let img = decode(&f, &lim()).unwrap();
    assert!(img.rgba.chunks_exact(4).all(|p| p[3] == 255));
    assert_quadrants(&img, [RED, GREEN, BLUE, YELLOW]);
}

// ---- thumbnails --------------------------------------------------------------------------------

fn with_thumbs(thumbs: &[(u32, u32, u32)], primary_props: &[(Vec<u8>, bool)]) -> Vec<u8> {
    let q = quad_hevc();
    let mut primary = Item::hevc(1, &q);
    for (p, e) in primary_props {
        primary = primary.with(p.clone(), *e);
    }
    let mut items = vec![primary];
    let mut ids = Vec::new();
    for &(id, w, h) in thumbs {
        let mut t = Item::hevc(id, &q);
        t.props[1] = (p_ispe(w, h), false);
        items.push(t);
        ids.push(id);
    }
    let mut spec = Spec::new(1, items);
    for id in ids {
        spec = spec.refer(b"thmb", id, &[1]);
    }
    spec.build()
}

#[test]
fn thumbnail_selection() {
    let f = with_thumbs(&[(2, 120, 90), (3, 40, 30), (4, 80, 60)], &[]);
    assert!(info(&f).unwrap().has_thumbnail);
    let t = |edge| {
        let i = decode_thumbnail(&f, edge, &lim()).unwrap();
        (i.width, i.height)
    };
    assert_eq!(t(1), (40, 30)); // smallest that is >= 1
    assert_eq!(t(40), (40, 30)); // exactly equal counts as large enough
    assert_eq!(t(41), (80, 60));
    assert_eq!(t(80), (80, 60));
    assert_eq!(t(100), (120, 90));
    assert_eq!(t(120), (120, 90));
    assert_eq!(t(121), (120, 90)); // none large enough: the largest, never upscaled
    assert_eq!(t(4096), (120, 90));
}

#[test]
fn thumbnail_edge_uses_the_long_side() {
    // a portrait thumbnail 60 wide, 100 tall has long edge 100
    let f = with_thumbs(&[(2, 60, 100), (3, 40, 30)], &[]);
    let i = decode_thumbnail(&f, 90, &lim()).unwrap();
    assert_eq!((i.width, i.height), (60, 100));
}

#[test]
fn thumbnail_orientation_comes_from_the_thumbnail_item_only() {
    // iPhone layout: the primary and its thumbnail each carry their own irot.
    let q = quad_hevc();
    let mut thumb = Item::hevc(2, &q).with(p_irot(3), true);
    thumb.props[1] = (p_ispe(80, 60), false);
    let f = Spec::new(1, vec![Item::hevc(1, &q).with(p_irot(3), true), thumb])
        .refer(b"thmb", 2, &[1])
        .build();
    let t = decode_thumbnail(&f, 32, &lim()).unwrap();
    assert_eq!((t.width, t.height), (60, 80));
    // properties are per item: a thumbnail without irot is not rotated by the primary's
    let f = with_thumbs(&[(2, 80, 60)], &[(p_irot(1), true)]);
    let t = decode_thumbnail(&f, 32, &lim()).unwrap();
    assert_eq!((t.width, t.height), (80, 60));
    // and the primary itself is rotated
    assert_eq!(
        decode(&f, &lim()).map(|i| (i.width, i.height)),
        Ok((120, 160))
    );
}

#[test]
fn unusable_thumbnail_is_skipped() {
    let q = quad_hevc();
    let mut jpeg = Item::new(2, b"jpeg");
    jpeg.data = vec![0; 8];
    jpeg.props = vec![(p_ispe(200, 150), false)];
    let mut small = Item::hevc(3, &q);
    small.props[1] = (p_ispe(80, 60), false);
    let f = Spec::new(1, vec![Item::hevc(1, &q), jpeg, small])
        .refer(b"thmb", 2, &[1])
        .refer(b"thmb", 3, &[1])
        .build();
    // the 200px JPEG thumbnail would be chosen for 150, but cannot be decoded
    let t = decode_thumbnail(&f, 150, &lim()).unwrap();
    assert_eq!((t.width, t.height), (80, 60));
    // only an unusable thumbnail: the primary
    let f = Spec::new(
        1,
        vec![
            Item::hevc(1, &q),
            Item::new(2, b"jpeg").with(p_ispe(64, 64), false),
        ],
    )
    .refer(b"thmb", 2, &[1])
    .build();
    let t = decode_thumbnail(&f, 32, &lim()).unwrap();
    assert_eq!((t.width, t.height), (160, 120));
}

// ---- limits ------------------------------------------------------------------------------------

fn claim(w: u32, h: u32) -> Vec<u8> {
    let q = quad_hevc();
    let mut it = Item::hevc(1, &q);
    it.props[1] = (p_ispe(w, h), false);
    Spec::new(1, vec![it]).build()
}

#[test]
fn dimension_limits_apply_before_decoding() {
    let l = Limits {
        max_width: 100,
        ..lim()
    };
    assert!(matches!(decode(QUAD, &l), Err(Error::LimitExceeded(_))));
    let l = Limits {
        max_height: 100,
        ..lim()
    };
    assert!(matches!(decode(QUAD, &l), Err(Error::LimitExceeded(_))));
    let l = Limits {
        max_pixels: 160 * 120 - 1,
        ..lim()
    };
    assert!(matches!(decode(QUAD, &l), Err(Error::LimitExceeded(_))));
    let l = Limits {
        max_pixels: 160 * 120,
        ..lim()
    };
    assert!(decode(QUAD, &l).is_ok());
    let l = Limits {
        max_memory_bytes: 1000,
        ..lim()
    };
    assert!(matches!(decode(QUAD, &l), Err(Error::LimitExceeded(_))));
}

#[test]
fn rotation_is_checked_against_the_output_dimensions() {
    let f = single(&[(p_irot(1), true)]); // output 120 x 160
    let l = Limits {
        max_width: 160,
        max_height: 130,
        ..lim()
    };
    assert!(matches!(decode(&f, &l), Err(Error::LimitExceeded(_))));
    let l = Limits {
        max_width: 160,
        max_height: 160,
        ..lim()
    };
    assert!(decode(&f, &l).is_ok());
}

#[test]
fn huge_claimed_sizes_are_rejected_without_allocating() {
    // 4 gigapixel claim: far beyond max_pixels
    assert!(matches!(
        decode(&claim(65535, 65535), &lim()),
        Err(Error::LimitExceeded(_))
    ));
    assert!(matches!(
        decode(&claim(u32::MAX, u32::MAX), &lim()),
        Err(Error::LimitExceeded(_))
    ));
    // within the pixel limit but over a small memory budget: 4000 x 4000 RGBA is 64 MB
    let l = Limits {
        max_memory_bytes: 50_000_000,
        ..lim()
    };
    assert!(matches!(
        decode(&claim(4000, 4000), &l),
        Err(Error::LimitExceeded(_))
    ));
    // zero sized claim
    assert!(matches!(
        decode(&claim(0, 10), &lim()),
        Err(Error::Malformed(_))
    ));
    // info() never allocates by size and still reports the claim
    let i = info(&claim(65535, 65535)).unwrap();
    assert_eq!((i.width, i.height), (65535, 65535));
}

#[test]
fn sps_larger_than_ispe_is_checked_against_limits() {
    // ispe says 16x16 (cheap) but the bitstream is a 160x120 picture
    let f = claim(16, 16);
    let l = Limits {
        max_pixels: 1000,
        ..lim()
    };
    assert!(matches!(decode(&f, &l), Err(Error::LimitExceeded(_))));
    // with room it decodes and is cropped to the declared extents
    let img = decode(&f, &lim()).unwrap();
    assert_eq!((img.width, img.height), (16, 16));
}

#[test]
fn decoded_picture_smaller_than_ispe_is_malformed() {
    assert!(matches!(
        decode(&claim(320, 240), &lim()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn grid_limits_use_the_descriptor() {
    // 255 x 255 tiles claiming a 40800 x 30600 canvas
    let spec = grid(1, 1, 160, 120, true);
    let mut spec = spec;
    spec.items[0].data = grid_descriptor(1, 1, 60000, 60000, true);
    assert!(matches!(
        decode(&spec.build(), &lim()),
        Err(Error::LimitExceeded(_))
    ));
    // canvas within limits but memory budget too small
    let spec = grid(2, 2, 320, 240, true);
    let l = Limits {
        max_memory_bytes: 320 * 240 * 4,
        ..lim()
    };
    assert!(matches!(
        decode(&spec.build(), &l),
        Err(Error::LimitExceeded(_))
    ));
}

// ---- structural errors ---------------------------------------------------------------------------

#[test]
fn structural_errors_are_malformed() {
    let h = quad_hevc();
    // primary item not in iinf
    let f = Spec::new(9, vec![Item::hevc(1, &h)]).build();
    assert!(matches!(decode(&f, &lim()), Err(Error::Malformed(_))));
    // duplicate item id
    let f = Spec::new(1, vec![Item::hevc(1, &h), Item::hevc(1, &h)]).build();
    assert!(matches!(decode(&f, &lim()), Err(Error::Malformed(_))));
    // item without location
    let mut it = Item::hevc(1, &h);
    it.no_location = true;
    assert!(matches!(
        decode(&Spec::new(1, vec![it]).build(), &lim()),
        Err(Error::Malformed(_))
    ));
    // missing hvcC
    let mut it = Item::hevc(1, &h);
    it.props.remove(0);
    assert!(matches!(
        decode(&Spec::new(1, vec![it]).build(), &lim()),
        Err(Error::Malformed(_))
    ));
    // missing ispe
    let mut it = Item::hevc(1, &h);
    it.props.remove(1);
    assert!(matches!(
        decode(&Spec::new(1, vec![it]).build(), &lim()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn corrupt_picture_data_is_an_error() {
    let h = quad_hevc();
    let mut it = Item::hevc(1, &h);
    for b in it.data.iter_mut().skip(20) {
        *b = !*b;
    }
    let r = decode(&Spec::new(1, vec![it]).build(), &lim());
    assert!(r.is_err() || r.is_ok()); // must not panic; either outcome is acceptable
    let mut it = Item::hevc(1, &h);
    it.data.truncate(40);
    assert!(decode(&Spec::new(1, vec![it]).build(), &lim()).is_err());
}

#[test]
fn error_display_is_informative() {
    for e in [
        Error::NotHeif,
        Error::Unsupported("x"),
        Error::LimitExceeded("y"),
        Error::Malformed("z"),
        Error::Hevc("w".into()),
    ] {
        assert!(!e.to_string().is_empty());
    }
    let e: Box<dyn std::error::Error> = Box::new(Error::NotHeif);
    assert!(e.to_string().contains("HEIF"));
}
