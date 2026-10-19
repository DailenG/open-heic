//! Test support: a small, deterministic HEIF file assembler.
//!
//! The HEVC tile used by every synthetic file is lifted out of the committed `quad.heic`
//! fixture (160x120, four flat quadrants: red top-left, green top-right, blue bottom-left,
//! yellow bottom-right), so the expected pixels are known without any external encoder.

#![allow(dead_code)]

use open_heic::Image;

pub const QUAD: &[u8] = include_bytes!("../fixtures/quad.heic");

pub fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((8 + body.len()) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

pub fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

/// Children `(type, body)` of a box body (or of a whole file).
pub fn children(mut data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    while data.len() >= 8 {
        let size = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[4..8].try_into().unwrap();
        out.push((kind, &data[8..size]));
        data = &data[size..];
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    children(data)
        .into_iter()
        .find(|(k, _)| k == kind)
        .expect("box present")
        .1
}

/// One HEVC-coded picture as stored in a HEIF file.
#[derive(Clone)]
pub struct Hevc {
    /// Body of the `hvcC` box.
    pub hvcc: Vec<u8>,
    /// Length-prefixed NAL units (the item data).
    pub payload: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The HEVC picture of `quad.heic`.
pub fn quad_hevc() -> Hevc {
    let top = children(QUAD);
    let meta = top.iter().find(|(k, _)| k == b"meta").unwrap().1;
    let iprp = child(&meta[4..], b"iprp");
    let ipco = child(iprp, b"ipco");
    Hevc {
        hvcc: child(ipco, b"hvcC").to_vec(),
        payload: top.iter().find(|(k, _)| k == b"mdat").unwrap().1.to_vec(),
        width: 160,
        height: 120,
    }
}

fn unescape(nal: &[u8]) -> Vec<u8> {
    let (mut out, mut zeros) = (Vec::new(), 0);
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Split an Annex B stream into NAL units (without start codes or trailing zero padding).
fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i..i + 3] == [0, 0, 1] {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (n, &s) in starts.iter().enumerate() {
        let mut e = if n + 1 < starts.len() {
            starts[n + 1] - 3
        } else {
            data.len()
        };
        while e > s && data[e - 1] == 0 && n + 1 < starts.len() {
            e -= 1;
        }
        out.push(&data[s..e]);
    }
    out
}

/// Wrap a single-picture HEVC Annex B stream (as written by `ffmpeg -f hevc`) for use as a
/// HEIF item: parameter sets go into an `hvcC`, slices become length-prefixed NAL units.
/// SEI and other non-VCL units are dropped.
pub fn hevc_from_annex_b(
    stream: &[u8],
    width: u32,
    height: u32,
    chroma_format: u8,
    bit_depth: u8,
) -> Hevc {
    let nals = split_annex_b(stream);
    let of_type = |t: u8| {
        nals.iter()
            .copied()
            .find(|n| (n[0] >> 1) & 0x3F == t)
            .expect("parameter set present")
    };
    let (vps, sps, pps) = (of_type(32), of_type(33), of_type(34));
    let ptl = &unescape(sps)[3..15];
    let mut hvcc = vec![1, ptl[0]];
    hvcc.extend_from_slice(&ptl[1..5]); // general_profile_compatibility_flags
    hvcc.extend_from_slice(&ptl[5..11]); // constraint indicator flags
    hvcc.push(ptl[11]); // general_level_idc
    hvcc.extend_from_slice(&[0xF0, 0x00]); // min_spatial_segmentation_idc
    hvcc.push(0xFC); // parallelismType
    hvcc.push(0xFC | chroma_format);
    hvcc.push(0xF8 | (bit_depth - 8));
    hvcc.push(0xF8 | (bit_depth - 8));
    hvcc.extend_from_slice(&[0, 0]); // avgFrameRate
    hvcc.push(0x03); // lengthSizeMinusOne = 3
    hvcc.push(3); // numOfArrays
    for (t, nal) in [(32u8, vps), (33, sps), (34, pps)] {
        hvcc.push(0x80 | t);
        hvcc.extend_from_slice(&1u16.to_be_bytes());
        hvcc.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        hvcc.extend_from_slice(nal);
    }
    let mut payload = Vec::new();
    for n in nals.iter().filter(|n| (n[0] >> 1) & 0x3F < 32) {
        payload.extend_from_slice(&(n.len() as u32).to_be_bytes());
        payload.extend_from_slice(n);
    }
    Hevc {
        hvcc,
        payload,
        width,
        height,
    }
}

/// One of the committed 64x48 test streams (`tests/fixtures/*.hevc`, see the README there).
pub fn stream(name: &str, chroma_format: u8, bit_depth: u8) -> Hevc {
    let path = format!("{}/tests/fixtures/{name}.hevc", env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    hevc_from_annex_b(&data, 64, 48, chroma_format, bit_depth)
}

// ---- property boxes -------------------------------------------------------------------

pub fn p_hvcc(h: &Hevc) -> Vec<u8> {
    bx(b"hvcC", &h.hvcc)
}

pub fn p_ispe(w: u32, h: u32) -> Vec<u8> {
    full(b"ispe", 0, 0, &[w.to_be_bytes(), h.to_be_bytes()].concat())
}

pub fn p_irot(angle: u8) -> Vec<u8> {
    bx(b"irot", &[angle])
}

pub fn p_imir(axis: u8) -> Vec<u8> {
    bx(b"imir", &[axis])
}

#[allow(clippy::too_many_arguments)]
pub fn p_clap(wn: u32, wd: u32, hn: u32, hd: u32, xn: i32, xd: u32, yn: i32, yd: u32) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [wn, wd, hn, hd] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.extend_from_slice(&xn.to_be_bytes());
    b.extend_from_slice(&xd.to_be_bytes());
    b.extend_from_slice(&yn.to_be_bytes());
    b.extend_from_slice(&yd.to_be_bytes());
    bx(b"clap", &b)
}

pub fn p_colr_nclx(primaries: u16, transfer: u16, matrix: u16, full_range: bool) -> Vec<u8> {
    let mut b = b"nclx".to_vec();
    for v in [primaries, transfer, matrix] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.push(if full_range { 0x80 } else { 0 });
    bx(b"colr", &b)
}

pub fn p_colr_icc() -> Vec<u8> {
    bx(b"colr", &[b"prof".as_slice(), &[1, 2, 3, 4]].concat())
}

pub fn p_pixi(depth: u8) -> Vec<u8> {
    full(b"pixi", 0, 0, &[3, depth, depth, depth])
}

pub const ALPHA_URN: &str = "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha";

pub fn p_auxc(urn: &str) -> Vec<u8> {
    let mut b = urn.as_bytes().to_vec();
    b.push(0);
    full(b"auxC", 0, 0, &b)
}

// ---- items ------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Store {
    File,
    Idat,
}

#[derive(Clone)]
pub struct Item {
    pub id: u32,
    pub kind: [u8; 4],
    pub data: Vec<u8>,
    pub store: Store,
    /// Number of `iloc` extents the data is split over (separated by junk in the file).
    pub extents: usize,
    /// `(property box, essential)`.
    pub props: Vec<(Vec<u8>, bool)>,
    /// Omit the item from `iloc`.
    pub no_location: bool,
}

impl Item {
    pub fn new(id: u32, kind: &[u8; 4]) -> Self {
        Item {
            id,
            kind: *kind,
            data: Vec::new(),
            store: Store::File,
            extents: 1,
            props: Vec::new(),
            no_location: false,
        }
    }

    pub fn hevc(id: u32, h: &Hevc) -> Self {
        let mut it = Item::new(id, b"hvc1");
        it.data = h.payload.clone();
        it.props = vec![(p_hvcc(h), true), (p_ispe(h.width, h.height), false)];
        it
    }

    pub fn with(mut self, prop: Vec<u8>, essential: bool) -> Self {
        self.props.push((prop, essential));
        self
    }

    pub fn idat(mut self) -> Self {
        self.store = Store::Idat;
        self
    }

    pub fn split(mut self, n: usize) -> Self {
        self.extents = n;
        self
    }
}

/// An `ImageGrid` descriptor.
pub fn grid_descriptor(rows: u8, cols: u8, w: u32, h: u32, wide: bool) -> Vec<u8> {
    let mut d = vec![0, u8::from(wide), rows - 1, cols - 1];
    if wide {
        d.extend_from_slice(&w.to_be_bytes());
        d.extend_from_slice(&h.to_be_bytes());
    } else {
        d.extend_from_slice(&(w as u16).to_be_bytes());
        d.extend_from_slice(&(h as u16).to_be_bytes());
    }
    d
}

#[derive(Clone)]
pub struct Opts {
    pub brand: [u8; 4],
    pub iloc_version: u8,
    pub infe_version: u8,
    pub ipma_version: u8,
    pub ipma_wide_assoc: bool,
    pub iref_version: u8,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            brand: *b"heic",
            iloc_version: 1,
            infe_version: 2,
            ipma_version: 0,
            ipma_wide_assoc: false,
            iref_version: 0,
        }
    }
}

#[derive(Clone)]
pub struct Spec {
    pub primary: u32,
    pub items: Vec<Item>,
    /// `(reference type, from, to...)`.
    pub refs: Vec<([u8; 4], u32, Vec<u32>)>,
    pub opts: Opts,
}

impl Spec {
    pub fn new(primary: u32, items: Vec<Item>) -> Self {
        Spec {
            primary,
            items,
            refs: Vec::new(),
            opts: Opts::default(),
        }
    }

    pub fn refer(mut self, kind: &[u8; 4], from: u32, to: &[u32]) -> Self {
        self.refs.push((*kind, from, to.to_vec()));
        self
    }

    pub fn build(&self) -> Vec<u8> {
        build(self)
    }
}

fn id_bytes(id: u32, wide: bool) -> Vec<u8> {
    if wide {
        id.to_be_bytes().to_vec()
    } else {
        (id as u16).to_be_bytes().to_vec()
    }
}

/// Item data layout: the `mdat` body, the `idat` body and, per item, its extents as
/// (store-relative offset, length).
type Layout = (Vec<u8>, Vec<u8>, Vec<Vec<(u32, u32)>>);

fn layout(spec: &Spec) -> Layout {
    let (mut mdat, mut idat) = (Vec::new(), Vec::new());
    let mut all = Vec::new();
    for it in &spec.items {
        let mut exts = Vec::new();
        let n = it.extents.max(1);
        let part = it.data.len().div_ceil(n).max(1);
        let target = if it.store == Store::File {
            &mut mdat
        } else {
            &mut idat
        };
        let mut chunks: Vec<&[u8]> = it.data.chunks(part).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }
        for c in chunks {
            exts.push((target.len() as u32, c.len() as u32));
            target.extend_from_slice(c);
            if it.store == Store::File {
                target.extend_from_slice(&[0xEE, 0xEE, 0xEE]); // junk between extents
            }
        }
        all.push(exts);
    }
    (mdat, idat, all)
}

fn build(spec: &Spec) -> Vec<u8> {
    let o = &spec.opts;
    let (mdat_body, idat_body, extents) = layout(spec);

    // property table (deduplicated by content) and associations
    let mut table: Vec<Vec<u8>> = Vec::new();
    let mut assoc: Vec<Vec<(usize, bool)>> = Vec::new();
    for it in &spec.items {
        let mut a = Vec::new();
        for (prop, essential) in &it.props {
            let idx = table.iter().position(|p| p == prop).unwrap_or_else(|| {
                table.push(prop.clone());
                table.len() - 1
            });
            a.push((idx + 1, *essential));
        }
        assoc.push(a);
    }

    let meta_with = |mdat_start: u32| -> Vec<u8> {
        let wide_ids = o.infe_version == 3;
        let hdlr = full(
            b"hdlr",
            0,
            0,
            &[&[0u8; 4][..], b"pict", &[0u8; 12], &[0]].concat(),
        );
        let pitm = if spec.primary > 0xFFFF {
            full(b"pitm", 1, 0, &spec.primary.to_be_bytes())
        } else {
            full(b"pitm", 0, 0, &(spec.primary as u16).to_be_bytes())
        };
        // iinf
        let mut infes = Vec::new();
        for it in &spec.items {
            let mut b = id_bytes(it.id, wide_ids);
            b.extend_from_slice(&[0, 0]);
            b.extend_from_slice(&it.kind);
            b.push(0);
            infes.extend(full(b"infe", o.infe_version, 0, &b));
        }
        let iinf = full(
            b"iinf",
            0,
            0,
            &[(spec.items.len() as u16).to_be_bytes().to_vec(), infes].concat(),
        );
        // iloc
        let v = o.iloc_version;
        // offset_size 4, length_size 4; base_offset_size 0, index_size 0
        let mut body = vec![0x44, 0x00];
        let located: Vec<usize> = (0..spec.items.len())
            .filter(|&i| !spec.items[i].no_location)
            .collect();
        if v < 2 {
            body.extend_from_slice(&(located.len() as u16).to_be_bytes());
        } else {
            body.extend_from_slice(&(located.len() as u32).to_be_bytes());
        }
        for &i in &located {
            let it = &spec.items[i];
            body.extend(id_bytes(it.id, v == 2));
            if v >= 1 {
                body.extend_from_slice(
                    &(if it.store == Store::Idat { 1u16 } else { 0 }).to_be_bytes(),
                );
            } else {
                assert!(it.store == Store::File, "iloc v0 cannot address idat");
            }
            body.extend_from_slice(&0u16.to_be_bytes()); // data_reference_index
            body.extend_from_slice(&(extents[i].len() as u16).to_be_bytes());
            for &(off, len) in &extents[i] {
                let base = if it.store == Store::File {
                    mdat_start
                } else {
                    0
                };
                body.extend_from_slice(&(base + off).to_be_bytes());
                body.extend_from_slice(&len.to_be_bytes());
            }
        }
        let iloc = full(b"iloc", v, 0, &body);
        // iref
        let mut refs = Vec::new();
        let wide_ref = o.iref_version == 1;
        for (kind, from, to) in &spec.refs {
            let mut b = id_bytes(*from, wide_ref);
            b.extend_from_slice(&(to.len() as u16).to_be_bytes());
            for t in to {
                b.extend(id_bytes(*t, wide_ref));
            }
            refs.extend(bx(kind, &b));
        }
        let iref = if spec.refs.is_empty() {
            Vec::new()
        } else {
            full(b"iref", o.iref_version, 0, &refs)
        };
        // iprp
        let ipco = bx(b"ipco", &table.concat());
        let mut ipma_body = (spec.items.len() as u32).to_be_bytes().to_vec();
        for (it, a) in spec.items.iter().zip(&assoc) {
            ipma_body.extend(id_bytes(it.id, o.ipma_version >= 1));
            ipma_body.push(a.len() as u8);
            for &(idx, essential) in a {
                if o.ipma_wide_assoc {
                    ipma_body.extend_from_slice(
                        &(((essential as u16) << 15) | idx as u16).to_be_bytes(),
                    );
                } else {
                    ipma_body.push(((essential as u8) << 7) | idx as u8);
                }
            }
        }
        let ipma = full(
            b"ipma",
            o.ipma_version,
            u32::from(o.ipma_wide_assoc),
            &ipma_body,
        );
        let iprp = bx(b"iprp", &[ipco, ipma].concat());
        let idat = if idat_body.is_empty() {
            Vec::new()
        } else {
            bx(b"idat", &idat_body)
        };
        full(
            b"meta",
            0,
            0,
            &[hdlr, pitm, iinf, iloc, iref, iprp, idat].concat(),
        )
    };

    let ftyp = bx(
        b"ftyp",
        &[&o.brand[..], &[0, 0, 0, 0], b"mif1", &o.brand[..]].concat(),
    );
    let meta_len = meta_with(0).len();
    let mdat_start = (ftyp.len() + meta_len + 8) as u32;
    let meta = meta_with(mdat_start);
    assert_eq!(meta.len(), meta_len);
    [ftyp, meta, bx(b"mdat", &mdat_body)].concat()
}

// ---- pixel helpers ------------------------------------------------------------------------

pub fn px(img: &Image, x: u32, y: u32) -> [u8; 4] {
    assert!(
        x < img.width && y < img.height,
        "pixel ({x},{y}) outside {}x{}",
        img.width,
        img.height
    );
    let i = ((y * img.width + x) * 4) as usize;
    [
        img.rgba[i],
        img.rgba[i + 1],
        img.rgba[i + 2],
        img.rgba[i + 3],
    ]
}

pub const RED: [u8; 3] = [255, 0, 0];
pub const GREEN: [u8; 3] = [0, 255, 0];
pub const BLUE: [u8; 3] = [0, 0, 255];
pub const YELLOW: [u8; 3] = [255, 255, 0];

/// Assert that the RGB part of `p` is within `tol` of `want` per channel.
#[track_caller]
pub fn assert_near(p: [u8; 4], want: [u8; 3], tol: i32) {
    for c in 0..3 {
        assert!(
            (i32::from(p[c]) - i32::from(want[c])).abs() <= tol,
            "pixel {p:?} is not within {tol} of {want:?}"
        );
    }
}

/// Centre of each quadrant of a `w` x `h` image: `[top-left, top-right, bottom-left, bottom-right]`.
pub fn quadrant_centres(img: &Image) -> [[u8; 4]; 4] {
    let (w, h) = (img.width, img.height);
    [
        px(img, w / 4, h / 4),
        px(img, 3 * w / 4, h / 4),
        px(img, w / 4, 3 * h / 4),
        px(img, 3 * w / 4, 3 * h / 4),
    ]
}

#[track_caller]
pub fn assert_quadrants(img: &Image, want: [[u8; 3]; 4]) {
    let got = quadrant_centres(img);
    for (g, w) in got.iter().zip(want) {
        assert_near(*g, w, 12);
    }
}

/// Single-tile file: the quad picture as a plain `hvc1` primary item with `extra` properties.
pub fn single(extra: &[(Vec<u8>, bool)]) -> Vec<u8> {
    let h = quad_hevc();
    let mut item = Item::hevc(1, &h);
    for (p, e) in extra {
        item = item.with(p.clone(), *e);
    }
    Spec::new(1, vec![item]).build()
}

/// Grid file: `rows` x `cols` copies of the quad picture, output `w` x `h`, descriptor in
/// `idat` when `idat_descriptor`.
pub fn grid(rows: u8, cols: u8, w: u32, h: u32, idat_descriptor: bool) -> Spec {
    let q = quad_hevc();
    let mut g = Item::new(1, b"grid");
    g.data = grid_descriptor(rows, cols, w, h, false);
    g.store = if idat_descriptor {
        Store::Idat
    } else {
        Store::File
    };
    g.props = vec![(p_ispe(w, h), false)];
    let n = u32::from(rows) * u32::from(cols);
    let mut items = vec![g];
    let mut ids = Vec::new();
    for i in 0..n {
        items.push(Item::hevc(2 + i, &q));
        ids.push(2 + i);
    }
    Spec::new(1, items).refer(b"dimg", 1, &ids)
}

/// Like [`stream`] for a stream of another size.
pub fn stream_sized(name: &str, width: u32, height: u32) -> Hevc {
    let path = format!("{}/tests/fixtures/{name}.hevc", env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    hevc_from_annex_b(&data, width, height, 1, 8)
}
