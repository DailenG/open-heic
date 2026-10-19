//! HEIF container parsing: `ftyp`, `meta` and everything inside it (ISO/IEC 14496-12
//! clauses 4.3, 8.11; ISO/IEC 23008-12 clauses 6 and 9).
//!
//! The parser only builds an index of items, properties and references over the input
//! slice. Pixel data stays in place and is fetched through [`Heif::item_data`].

use std::borrow::Cow;
use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::geom::Op;
use crate::props::{Colr, Hvcc, Nclx, Property, parse_property};
use crate::reader::{BmffBox, FourCc, Reader, boxes};

/// Maximum number of items accepted in `iinf` / `iloc`.
const MAX_ITEMS: usize = 1 << 16;
/// Maximum number of properties accepted in `ipco` (`ipma` indices are 15 bits).
const MAX_PROPERTIES: usize = 1 << 15;
/// Maximum number of property associations over the whole file.
const MAX_ASSOCIATIONS: usize = 1 << 20;
/// Maximum number of reference targets over the whole file.
const MAX_REFERENCE_TARGETS: usize = 1 << 20;

/// Brands that identify a HEIF/AVIF still-image file (HEIF clause 10, MIAF).
const BRANDS: &[&[u8; 4]] = &[
    b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"hevm", b"hevs", b"mif1", b"mif2",
    b"msf1", b"avif", b"avis", b"miaf",
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct Extent {
    /// Absolute file offset (construction method 0) or offset inside `idat` (method 1).
    pub offset: u64,
    /// Zero means "to the end of the containing resource".
    pub length: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Location {
    pub method: u8,
    pub external: bool,
    pub extents: Vec<Extent>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Assoc {
    /// Zero-based index into the property table.
    pub index: u16,
    pub essential: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub id: u32,
    pub kind: FourCc,
    pub protected: bool,
    pub location: Option<Location>,
    pub assoc: Vec<Assoc>,
}

#[derive(Clone, Debug)]
struct Reference {
    kind: FourCc,
    from: u32,
    to: Vec<u32>,
}

/// The properties of one item, resolved from `ipma`/`ipco`.
#[derive(Debug, Default)]
pub(crate) struct ItemProps<'h> {
    pub ispe: Option<(u32, u32)>,
    pub hvcc: Option<&'h Hvcc>,
    pub nclx: Option<Nclx>,
    pub has_icc: bool,
    pub pixi_depth: Option<u8>,
    pub alpha_aux: bool,
    /// Transformative properties in `ipma` order.
    pub ops: Vec<Op>,
}

pub(crate) struct Heif<'a> {
    file: &'a [u8],
    pub primary: u32,
    items: Vec<Item>,
    index: HashMap<u32, usize>,
    props: Vec<Property>,
    refs: Vec<Reference>,
    idat: &'a [u8],
}

impl<'a> Heif<'a> {
    pub fn parse(file: &'a [u8]) -> Result<Self> {
        if file.len() < 12 || &file[4..8] != b"ftyp" {
            return Err(Error::NotHeif);
        }
        let mut meta = None;
        for (n, b) in boxes(file).enumerate() {
            let b = b?;
            if n == 0 {
                check_brands(b.body)?;
            } else if &b.kind == b"meta" {
                meta = Some(b);
                break;
            }
        }
        let meta = meta.ok_or(Error::Malformed("no meta box"))?;
        Self::parse_meta(file, meta)
    }

    fn parse_meta(file: &'a [u8], meta: BmffBox<'a>) -> Result<Self> {
        let mut r = Reader::new(meta.body);
        r.full_box()?;
        let (mut hdlr, mut pitm, mut iinf, mut iloc, mut iref, mut iprp, mut idat) =
            (None, None, None, None, None, None, None);
        for b in boxes(r.rest()) {
            let b = b?;
            let slot = match &b.kind {
                b"hdlr" => &mut hdlr,
                b"pitm" => &mut pitm,
                b"iinf" => &mut iinf,
                b"iloc" => &mut iloc,
                b"iref" => &mut iref,
                b"iprp" => &mut iprp,
                b"idat" => &mut idat,
                _ => continue,
            };
            slot.get_or_insert(b.body);
        }

        if let Some(h) = hdlr {
            let mut r = Reader::new(h);
            r.full_box()?;
            r.skip(4)?;
            if &r.fourcc()? != b"pict" {
                return Err(Error::Unsupported("meta handler is not 'pict'"));
            }
        }

        let primary = {
            let mut r = Reader::new(pitm.ok_or(Error::Malformed("no pitm box"))?);
            let (v, _) = r.full_box()?;
            if v == 0 {
                u32::from(r.u16()?)
            } else {
                r.u32()?
            }
        };

        let (items, index) = parse_iinf(iinf.ok_or(Error::Malformed("no iinf box"))?)?;
        let mut heif = Heif {
            file,
            primary,
            items,
            index,
            props: Vec::new(),
            refs: Vec::new(),
            idat: idat.unwrap_or(&[]),
        };
        parse_iloc(iloc.ok_or(Error::Malformed("no iloc box"))?, &mut heif)?;
        if let Some(b) = iref {
            heif.refs = parse_iref(b)?;
        }
        if let Some(b) = iprp {
            parse_iprp(b, &mut heif)?;
        }
        if !heif.index.contains_key(&heif.primary) {
            return Err(Error::Malformed("primary item is not in iinf"));
        }
        Ok(heif)
    }

    pub fn item(&self, id: u32) -> Result<&Item> {
        self.index
            .get(&id)
            .map(|&i| &self.items[i])
            .ok_or(Error::Malformed("reference to a missing item"))
    }

    pub fn primary_item(&self) -> Result<&Item> {
        self.item(self.primary)
    }

    /// Target item ids of the first `dimg` reference whose source is `id`.
    pub fn dimg_inputs(&self, id: u32) -> Option<&[u32]> {
        self.refs
            .iter()
            .find(|r| r.from == id && &r.kind == b"dimg")
            .map(|r| r.to.as_slice())
    }

    /// Items that reference `target` with reference type `kind` (for example the thumbnails
    /// of an image are the sources of `thmb` references to it).
    pub fn sources_referencing<'s>(
        &'s self,
        target: u32,
        kind: &'static [u8; 4],
    ) -> impl Iterator<Item = &'s Item> + 's {
        self.refs
            .iter()
            .filter(move |r| &r.kind == kind && r.to.contains(&target))
            .filter_map(|r| self.index.get(&r.from).map(|&i| &self.items[i]))
    }

    /// The alpha auxiliary item of `id`, if any (`auxl` reference + alpha `auxC`).
    pub fn alpha_item(&self, id: u32) -> Option<&Item> {
        self.sources_referencing(id, b"auxl").find(|it| {
            it.assoc.iter().any(|a| {
                matches!(
                    self.props.get(usize::from(a.index)),
                    Some(Property::AuxC { alpha: true })
                )
            })
        })
    }

    /// Resolve the properties of `item`.
    ///
    /// An essential property this crate does not understand makes the item unusable
    /// ([`Error::Unsupported`], HEIF clause 6.5.1); an unparsable known property is
    /// [`Error::Malformed`].
    pub fn item_props(&self, item: &Item) -> Result<ItemProps<'_>> {
        let mut out = ItemProps::default();
        for a in &item.assoc {
            let p = self
                .props
                .get(usize::from(a.index))
                .ok_or(Error::Malformed("property index out of range"))?;
            match p {
                Property::Unknown if a.essential => {
                    return Err(Error::Unsupported("unknown essential item property"));
                }
                Property::Unknown => {}
                Property::Invalid => return Err(Error::Malformed("invalid item property")),
                Property::Hvcc(h) => {
                    out.hvcc.get_or_insert(h);
                }
                Property::Ispe { width, height } => {
                    out.ispe.get_or_insert((*width, *height));
                }
                Property::Irot(a) => out.ops.push(Op::Rotate(*a)),
                Property::Imir(a) => out.ops.push(Op::Mirror(*a)),
                Property::Clap(c) => out.ops.push(Op::Clap(*c)),
                Property::Colr(Colr::Nclx(n)) => {
                    out.nclx.get_or_insert(*n);
                }
                Property::Colr(Colr::Icc) => out.has_icc = true,
                Property::Pixi { first_depth } => {
                    out.pixi_depth.get_or_insert(*first_depth);
                }
                Property::AuxC { alpha } => out.alpha_aux |= *alpha,
            }
        }
        Ok(out)
    }

    /// The bytes of an item (HEIF clause 8.11.3 `iloc`), borrowed when stored contiguously.
    pub fn item_data(&self, item: &Item) -> Result<Cow<'a, [u8]>> {
        if item.protected {
            return Err(Error::Unsupported("protected item"));
        }
        let loc = item
            .location
            .as_ref()
            .ok_or(Error::Malformed("item has no location"))?;
        if loc.external {
            return Err(Error::Unsupported("item data in an external file"));
        }
        let resource: &'a [u8] = match loc.method {
            0 => self.file,
            1 => self.idat,
            _ => return Err(Error::Unsupported("iloc construction method")),
        };
        let slice_of = |e: &Extent| -> Result<&'a [u8]> {
            let start = usize::try_from(e.offset)
                .ok()
                .filter(|&s| s <= resource.len())
                .ok_or(Error::Malformed("item extent outside the file"))?;
            let end = if e.length == 0 {
                resource.len()
            } else {
                usize::try_from(e.length)
                    .ok()
                    .and_then(|l| start.checked_add(l))
                    .filter(|&end| end <= resource.len())
                    .ok_or(Error::Malformed("item extent outside the file"))?
            };
            Ok(&resource[start..end])
        };
        match loc.extents.as_slice() {
            [] => Err(Error::Malformed("item has no extents")),
            [one] => Ok(Cow::Borrowed(slice_of(one)?)),
            many => {
                let mut total = 0usize;
                let mut parts = Vec::with_capacity(many.len());
                for e in many {
                    let s = slice_of(e)?;
                    total = total
                        .checked_add(s.len())
                        .ok_or(Error::Malformed("item too large"))?;
                    // Overlapping extents could multiply a small file; a real item never
                    // exceeds the file that contains it.
                    if total > self.file.len() {
                        return Err(Error::Malformed("item extents exceed the file size"));
                    }
                    parts.push(s);
                }
                let mut out = Vec::with_capacity(total);
                for s in parts {
                    out.extend_from_slice(s);
                }
                Ok(Cow::Owned(out))
            }
        }
    }
}

fn check_brands(ftyp: &[u8]) -> Result<()> {
    let mut r = Reader::new(ftyp);
    let major = r.fourcc()?;
    r.skip(4)?; // minor_version
    let mut ok = BRANDS.contains(&&major);
    while r.remaining() >= 4 && !ok {
        ok = BRANDS.contains(&&r.fourcc()?);
    }
    if ok { Ok(()) } else { Err(Error::NotHeif) }
}

type ItemTable = (Vec<Item>, HashMap<u32, usize>);

fn parse_iinf(body: &[u8]) -> Result<ItemTable> {
    let mut r = Reader::new(body);
    let (v, _) = r.full_box()?;
    // The declared entry count is not trusted; the child boxes are the truth.
    if v == 0 {
        r.u16()?;
    } else {
        r.u32()?;
    }
    let mut items = Vec::new();
    let mut index = HashMap::new();
    for b in boxes(r.rest()) {
        let b = b?;
        if &b.kind != b"infe" {
            continue;
        }
        let mut r = Reader::new(b.body);
        // Flag bit 0 marks hidden items; visibility does not matter for decoding.
        let (v, _) = r.full_box()?;
        // Versions 0 and 1 carry no item_type; such items cannot be image items here.
        if !(2..=3).contains(&v) {
            continue;
        }
        let id = if v == 2 {
            u32::from(r.u16()?)
        } else {
            r.u32()?
        };
        let protection = r.u16()?;
        let kind = r.fourcc()?;
        if id == 0 {
            return Err(Error::Malformed("item id 0"));
        }
        if items.len() >= MAX_ITEMS {
            return Err(Error::LimitExceeded("too many items"));
        }
        if index.insert(id, items.len()).is_some() {
            return Err(Error::Malformed("duplicate item id"));
        }
        items.push(Item {
            id,
            kind,
            protected: protection != 0,
            location: None,
            assoc: Vec::new(),
        });
    }
    Ok((items, index))
}

fn parse_iloc(body: &[u8], heif: &mut Heif<'_>) -> Result<()> {
    let mut r = Reader::new(body);
    let (v, _) = r.full_box()?;
    if v > 2 {
        return Err(Error::Unsupported("iloc version"));
    }
    let sizes = r.u8()?;
    let (offset_size, length_size) = (usize::from(sizes >> 4), usize::from(sizes & 15));
    let sizes2 = r.u8()?;
    let base_size = usize::from(sizes2 >> 4);
    let index_size = if v == 1 || v == 2 {
        usize::from(sizes2 & 15)
    } else {
        0
    };
    for s in [offset_size, length_size, base_size, index_size] {
        if !matches!(s, 0 | 4 | 8) {
            return Err(Error::Malformed("iloc field size"));
        }
    }
    let count = if v < 2 { u32::from(r.u16()?) } else { r.u32()? };
    let mut seen = std::collections::HashSet::new();
    for _ in 0..count {
        let id = if v < 2 { u32::from(r.u16()?) } else { r.u32()? };
        let method = if v == 1 || v == 2 {
            (r.u16()? & 15) as u8
        } else {
            0
        };
        let data_reference_index = r.u16()?;
        let base = r.uint(base_size)?;
        let extent_count = r.u16()?;
        let mut extents = Vec::new();
        for _ in 0..extent_count {
            r.skip(index_size)?;
            let off = r.uint(offset_size)?;
            let length = r.uint(length_size)?;
            let offset = base
                .checked_add(off)
                .ok_or(Error::Malformed("iloc offset overflow"))?;
            extents.push(Extent { offset, length });
        }
        if seen.len() >= MAX_ITEMS {
            return Err(Error::LimitExceeded("too many iloc entries"));
        }
        if !seen.insert(id) {
            return Err(Error::Malformed("duplicate iloc entry"));
        }
        if let Some(&i) = heif.index.get(&id) {
            heif.items[i].location = Some(Location {
                method,
                external: data_reference_index != 0,
                extents,
            });
        }
    }
    Ok(())
}

fn parse_iref(body: &[u8]) -> Result<Vec<Reference>> {
    let mut r = Reader::new(body);
    let (v, _) = r.full_box()?;
    let width = match v {
        0 => 2,
        1 => 4,
        _ => return Err(Error::Unsupported("iref version")),
    };
    let mut refs = Vec::new();
    let mut targets = 0usize;
    for b in boxes(r.rest()) {
        let b = b?;
        let mut r = Reader::new(b.body);
        let from = r.uint(width)? as u32;
        let n = usize::from(r.u16()?);
        targets += n;
        if targets > MAX_REFERENCE_TARGETS {
            return Err(Error::LimitExceeded("too many item references"));
        }
        let mut to = Vec::with_capacity(n);
        for _ in 0..n {
            to.push(r.uint(width)? as u32);
        }
        refs.push(Reference {
            kind: b.kind,
            from,
            to,
        });
    }
    Ok(refs)
}

fn parse_iprp(body: &[u8], heif: &mut Heif<'_>) -> Result<()> {
    let mut ipma_boxes = Vec::new();
    for b in boxes(body) {
        let b = b?;
        match &b.kind {
            b"ipco" => {
                for p in boxes(b.body) {
                    if heif.props.len() >= MAX_PROPERTIES {
                        return Err(Error::LimitExceeded("too many item properties"));
                    }
                    heif.props.push(parse_property(p?));
                }
            }
            b"ipma" => ipma_boxes.push(b.body),
            _ => {}
        }
    }
    let mut total = 0usize;
    for body in ipma_boxes {
        let mut r = Reader::new(body);
        let (v, flags) = r.full_box()?;
        let entries = r.u32()?;
        for _ in 0..entries {
            let id = if v < 1 { u32::from(r.u16()?) } else { r.u32()? };
            let n = r.u8()?;
            let item = heif.index.get(&id).copied();
            for _ in 0..n {
                let (essential, idx) = if flags & 1 != 0 {
                    let x = r.u16()?;
                    (x & 0x8000 != 0, x & 0x7FFF)
                } else {
                    let x = r.u8()?;
                    (x & 0x80 != 0, u16::from(x & 0x7F))
                };
                total += 1;
                if total > MAX_ASSOCIATIONS {
                    return Err(Error::LimitExceeded("too many property associations"));
                }
                // Index 0 means "no property".
                if let (Some(i), true) = (item, idx > 0) {
                    heif.items[i].assoc.push(Assoc {
                        index: idx - 1,
                        essential,
                    });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((8 + body.len()) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn full(kind: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
        bx(kind, &[&[version, 0, 0, 0][..], body].concat())
    }

    /// Payload of a hand-built box (the bytes after its 8-byte header).
    fn payload(b: &[u8]) -> &[u8] {
        &b[8..]
    }

    fn file_with_meta(children: &[Vec<u8>]) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1");
        let meta = full(b"meta", 0, &children.concat());
        [ftyp, meta].concat()
    }

    fn minimal_children() -> Vec<Vec<u8>> {
        let infe = full(b"infe", 2, &[0, 1, 0, 0, b'h', b'v', b'c', b'1', 0]);
        vec![
            full(b"hdlr", 0, &[&[0u8; 4][..], b"pict", &[0; 13]].concat()),
            full(b"pitm", 0, &[0, 1]),
            full(b"iinf", 0, &[&[0u8, 1][..], &infe].concat()),
            full(
                b"iloc",
                0,
                &[0x44, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0x20, 0, 0, 0, 4],
            ),
        ]
    }

    #[test]
    fn brand_detection() {
        let heif = |ftyp: &[u8]| Heif::parse(&[bx(b"ftyp", ftyp), vec![0; 16]].concat()).err();
        // an accepted compatible brand is enough
        assert!(!matches!(
            heif(b"isom\0\0\0\0isomheic"),
            Some(Error::NotHeif)
        ));
        assert!(!matches!(heif(b"avif\0\0\0\0"), Some(Error::NotHeif)));
        assert_eq!(heif(b"isom\0\0\0\0mp42"), Some(Error::NotHeif));
        assert_eq!(heif(b"qt  \0\0\0\0"), Some(Error::NotHeif));
        // ftyp must come first and be present
        assert_eq!(
            Heif::parse(b"\0\0\0\x10meta\0\0\0\0\0\0\0\0").err(),
            Some(Error::NotHeif)
        );
        assert_eq!(Heif::parse(&[]).err(), Some(Error::NotHeif));
        // a truncated ftyp is malformed rather than "not heif"
        let mut f = bx(b"ftyp", b"heic\0\0\0\0");
        f[3] = 0x40;
        f.extend_from_slice(&[0; 8]);
        assert!(matches!(Heif::parse(&f), Err(Error::Malformed(_))));
    }

    #[test]
    fn minimal_file_parses() {
        let file = file_with_meta(&minimal_children());
        let h = Heif::parse(&file).unwrap();
        assert_eq!(h.primary, 1);
        let item = h.primary_item().unwrap();
        assert_eq!(&item.kind, b"hvc1");
        assert!(h.item(7).is_err());
    }

    #[test]
    fn meta_requirements() {
        let mut c = minimal_children();
        c.remove(1); // pitm
        assert!(matches!(
            Heif::parse(&file_with_meta(&c)),
            Err(Error::Malformed(_))
        ));
        let mut c = minimal_children();
        c.remove(2); // iinf
        assert!(matches!(
            Heif::parse(&file_with_meta(&c)),
            Err(Error::Malformed(_))
        ));
        let mut c = minimal_children();
        c.remove(3); // iloc
        assert!(matches!(
            Heif::parse(&file_with_meta(&c)),
            Err(Error::Malformed(_))
        ));
        // a non-pict handler is unsupported; a missing hdlr is tolerated
        let mut c = minimal_children();
        c[0] = full(b"hdlr", 0, &[&[0u8; 4][..], b"mdir", &[0; 13]].concat());
        assert!(matches!(
            Heif::parse(&file_with_meta(&c)),
            Err(Error::Unsupported(_))
        ));
        let mut c = minimal_children();
        c.remove(0);
        assert!(Heif::parse(&file_with_meta(&c)).is_ok());
        // no meta at all
        assert!(matches!(
            Heif::parse(&bx(b"ftyp", b"heic\0\0\0\0")),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn pitm_versions() {
        let mut c = minimal_children();
        c[1] = full(b"pitm", 1, &[0, 0, 0, 1]);
        assert_eq!(Heif::parse(&file_with_meta(&c)).unwrap().primary, 1);
        let mut c = minimal_children();
        c[1] = full(b"pitm", 0, &[0, 9]);
        assert!(matches!(
            Heif::parse(&file_with_meta(&c)),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn iinf_versions_flags_and_errors() {
        let v2 = full(b"infe", 2, &[0, 5, 0, 0, b'g', b'r', b'i', b'd', 0]);
        let v3 = full(b"infe", 3, &[0, 0, 1, 0, 0, 0, b'h', b'v', b'c', b'1', 0]);
        let v0 = full(b"infe", 0, &[0, 7, 0, 0, b'n', 0, b'x', 0]);
        let protected = full(b"infe", 2, &[0, 8, 0, 1, b'h', b'v', b'c', b'1', 0]);
        let iinf = full(
            b"iinf",
            1,
            &[&[0u8, 0, 0, 4][..], &v2, &v3, &v0, &protected].concat(),
        );
        let (items, index) = parse_iinf(payload(&iinf)).unwrap();
        assert_eq!(items.len(), 3, "version 0 entries are skipped");
        assert_eq!(&items[index[&5]].kind, b"grid");
        assert_eq!(&items[index[&256]].kind, b"hvc1");
        assert!(items[index[&8]].protected);

        let dup = full(b"iinf", 0, &[&[0u8, 2][..], &v2, &v2].concat());
        assert!(matches!(
            parse_iinf(payload(&dup)),
            Err(Error::Malformed(_))
        ));
        let zero = full(b"infe", 2, &[0, 0, 0, 0, b'h', b'v', b'c', b'1', 0]);
        let zero = full(b"iinf", 0, &[&[0u8, 1][..], &zero].concat());
        assert!(matches!(
            parse_iinf(payload(&zero)),
            Err(Error::Malformed(_))
        ));
        let cut = full(
            b"iinf",
            0,
            &[&[0u8, 1][..], &bx(b"infe", &[2, 0, 0, 0, 0, 5])].concat(),
        );
        assert!(matches!(
            parse_iinf(payload(&cut)),
            Err(Error::Malformed(_))
        ));
    }

    fn heif_with_items(ids: &[u32]) -> Heif<'static> {
        let items = ids
            .iter()
            .map(|&id| Item {
                id,
                kind: *b"hvc1",
                protected: false,
                location: None,
                assoc: Vec::new(),
            })
            .collect::<Vec<_>>();
        let index = items.iter().enumerate().map(|(i, it)| (it.id, i)).collect();
        Heif {
            file: &[],
            primary: ids[0],
            items,
            index,
            props: Vec::new(),
            refs: Vec::new(),
            idat: &[],
        }
    }

    fn loc(h: &Heif<'_>, id: u32) -> Location {
        h.item(id).unwrap().location.clone().unwrap()
    }

    #[test]
    fn iloc_v0_with_base_offset() {
        // offset_size 4, length_size 4, base_offset_size 4; one item, id 1, base 100,
        // two extents (10, 20) and (30, 40)
        let mut b = vec![0x44, 0x40, 0, 1, 0, 1, 0, 0, 0, 0, 0, 100, 0, 2];
        for v in [10u32, 20, 30, 40] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        let mut h = heif_with_items(&[1]);
        parse_iloc(payload(&full(b"iloc", 0, &b)), &mut h).unwrap();
        let l = loc(&h, 1);
        assert_eq!((l.method, l.external), (0, false));
        let e: Vec<_> = l.extents.iter().map(|e| (e.offset, e.length)).collect();
        assert_eq!(e, vec![(110, 20), (130, 40)]);
    }

    #[test]
    fn iloc_v1_and_v2_construction_methods_and_wide_fields() {
        // v1: sizes 8/8/0/0; item 3 uses idat (method 1)
        let mut b = vec![0x88, 0x00, 0, 1, 0, 3, 0, 1, 0, 0, 0, 1];
        b.extend_from_slice(&5u64.to_be_bytes());
        b.extend_from_slice(&6u64.to_be_bytes());
        let mut h = heif_with_items(&[3]);
        parse_iloc(payload(&full(b"iloc", 1, &b)), &mut h).unwrap();
        let l = loc(&h, 3);
        assert_eq!(l.method, 1);
        assert_eq!((l.extents[0].offset, l.extents[0].length), (5, 6));

        // v2: 32-bit item ids, extent_index present (index_size 4), item 70000
        let mut b = vec![0x44, 0x04];
        b.extend_from_slice(&1u32.to_be_bytes());
        b.extend_from_slice(&70000u32.to_be_bytes());
        b.extend_from_slice(&[0, 2]); // construction_method 2 (item offset)
        b.extend_from_slice(&[0, 0, 0, 1]); // data_reference_index 0, extent_count 1
        b.extend_from_slice(&9u32.to_be_bytes()); // extent_index
        b.extend_from_slice(&7u32.to_be_bytes());
        b.extend_from_slice(&8u32.to_be_bytes());
        let mut h = heif_with_items(&[70000]);
        parse_iloc(payload(&full(b"iloc", 2, &b)), &mut h).unwrap();
        let l = loc(&h, 70000);
        assert_eq!(l.method, 2);
        assert_eq!((l.extents[0].offset, l.extents[0].length), (7, 8));
        // method 2 is not implemented
        assert!(matches!(
            h.item_data(h.item(70000).unwrap()),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn iloc_rejects_bad_input() {
        let mut h = heif_with_items(&[1]);
        // field size 3 is not allowed
        let b = full(b"iloc", 0, &[0x34, 0x00, 0, 0]);
        assert!(matches!(
            parse_iloc(payload(&b), &mut h),
            Err(Error::Malformed(_))
        ));
        // unknown version
        let b = full(b"iloc", 3, &[0x44, 0x00, 0, 0]);
        assert!(matches!(
            parse_iloc(payload(&b), &mut h),
            Err(Error::Unsupported(_))
        ));
        // claims two items, holds none
        let b = full(b"iloc", 0, &[0x44, 0x00, 0, 2]);
        assert!(matches!(
            parse_iloc(payload(&b), &mut h),
            Err(Error::Malformed(_))
        ));
        // duplicate entries
        let entry = [0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 2];
        let b = full(b"iloc", 0, &[&[0x44, 0, 0, 2][..], &entry, &entry].concat());
        assert!(matches!(
            parse_iloc(payload(&b), &mut h),
            Err(Error::Malformed(_))
        ));
        // offset overflow
        let mut b = vec![0x88, 0x80, 0, 1, 0, 1, 0, 0];
        b.extend_from_slice(&u64::MAX.to_be_bytes()); // base
        b.extend_from_slice(&[0, 1]);
        b.extend_from_slice(&1u64.to_be_bytes());
        b.extend_from_slice(&1u64.to_be_bytes());
        let b = full(b"iloc", 0, &b);
        assert!(matches!(
            parse_iloc(payload(&b), &mut h),
            Err(Error::Malformed(_))
        ));
        // entries for items that are not in iinf are ignored
        let mut h = heif_with_items(&[1]);
        let entry = [0, 9, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 2];
        let b = full(b"iloc", 0, &[&[0x44, 0, 0, 1][..], &entry].concat());
        parse_iloc(payload(&b), &mut h).unwrap();
        assert!(h.item(1).unwrap().location.is_none());
    }

    #[test]
    fn item_data_extents() {
        let file: &'static [u8] = b"0123456789abcdef";
        let mut h = heif_with_items(&[1, 2, 3, 4, 5]);
        h.file = file;
        h.idat = b"IDAT";
        let set = |h: &mut Heif<'_>, id, method, external, extents: &[(u64, u64)]| {
            h.items[h.index[&id]].location = Some(Location {
                method,
                external,
                extents: extents
                    .iter()
                    .map(|&(offset, length)| Extent { offset, length })
                    .collect(),
            });
        };
        set(&mut h, 1, 0, false, &[(2, 3)]);
        set(&mut h, 2, 0, false, &[(0, 2), (10, 3)]);
        set(&mut h, 3, 1, false, &[(1, 2)]);
        set(&mut h, 4, 0, false, &[(12, 0)]); // length 0: to the end
        fn data<'a>(h: &Heif<'a>, id: u32) -> Result<Cow<'a, [u8]>> {
            h.item_data(h.item(id).unwrap())
        }
        let one = data(&h, 1).unwrap();
        assert_eq!(&*one, b"234");
        assert!(matches!(one, Cow::Borrowed(_)));
        let two = data(&h, 2).unwrap();
        assert_eq!(&*two, b"01abc");
        assert!(matches!(two, Cow::Owned(_)));
        assert_eq!(&*data(&h, 3).unwrap(), b"DA");
        assert_eq!(&*data(&h, 4).unwrap(), b"cdef");
        // errors
        assert!(matches!(data(&h, 5), Err(Error::Malformed(_)))); // no location
        set(&mut h, 5, 0, false, &[(10, 10)]);
        assert!(matches!(data(&h, 5), Err(Error::Malformed(_)))); // outside the file
        set(&mut h, 5, 0, false, &[(99, 0)]);
        assert!(matches!(data(&h, 5), Err(Error::Malformed(_))));
        set(&mut h, 5, 0, true, &[(0, 1)]);
        assert!(matches!(data(&h, 5), Err(Error::Unsupported(_)))); // external file
        set(&mut h, 5, 0, false, &[]);
        assert!(matches!(data(&h, 5), Err(Error::Malformed(_)))); // no extents
        set(&mut h, 5, 0, false, &[(0, 16), (0, 16)]);
        assert!(matches!(data(&h, 5), Err(Error::Malformed(_)))); // overlapping extents blow past the file size
        h.items[0].protected = true;
        assert!(matches!(data(&h, 1), Err(Error::Unsupported(_))));
    }

    #[test]
    fn iref_versions() {
        let r0 = bx(b"dimg", &[0, 1, 0, 2, 0, 2, 0, 3]);
        let b = full(
            b"iref",
            0,
            &[&r0[..], &bx(b"thmb", &[0, 9, 0, 1, 0, 1])[..]].concat(),
        );
        let refs = parse_iref(payload(&b)).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(
            (&refs[0].kind, refs[0].from, refs[0].to.clone()),
            (b"dimg", 1, vec![2, 3])
        );
        assert_eq!(
            (&refs[1].kind, refs[1].from, refs[1].to.clone()),
            (b"thmb", 9, vec![1])
        );
        let r1 = bx(b"auxl", &[0, 1, 0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF]);
        let b = full(b"iref", 1, &r1);
        let refs = parse_iref(payload(&b)).unwrap();
        assert_eq!(
            (refs[0].from, refs[0].to.clone()),
            (65536, vec![0xFFFF_FFFF])
        );
        // truncated target list
        let bad = full(b"iref", 0, &bx(b"dimg", &[0, 1, 0, 5, 0, 2]));
        assert!(matches!(
            parse_iref(payload(&bad)),
            Err(Error::Malformed(_))
        ));
        assert!(matches!(
            parse_iref(payload(&full(b"iref", 2, &[]))),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn ipma_and_item_props() {
        let ispe = full(b"ispe", 0, &[&[0, 0, 0, 8][..], &[0, 0, 0, 6]].concat());
        let irot = bx(b"irot", &[1]);
        let imir = bx(b"imir", &[0]);
        let weird = bx(b"zzzz", &[1]);
        let ipco = bx(b"ipco", &[ispe, irot, imir, weird].concat());
        // v0, 8-bit associations: item 1 -> ispe, irot (essential), imir, zzzz (non-essential),
        // and index 0 (ignored); item 2 -> zzzz essential
        let ipma = full(
            b"ipma",
            0,
            &[
                &[0, 0, 0, 2][..],
                &[0, 1, 5, 1, 0x82, 3, 4, 0],
                &[0, 2, 1, 0x84],
            ]
            .concat(),
        );
        let iprp = bx(b"iprp", &[ipco.clone(), ipma].concat());
        let mut h = heif_with_items(&[1, 2]);
        parse_iprp(payload(&iprp), &mut h).unwrap();
        let p = h.item_props(h.item(1).unwrap()).unwrap();
        assert_eq!(p.ispe, Some((8, 6)));
        assert_eq!(p.ops, vec![Op::Rotate(1), Op::Mirror(0)]);
        assert!(matches!(
            h.item_props(h.item(2).unwrap()),
            Err(Error::Unsupported(_))
        ));

        // v1 with 16-bit associations: item 70000, essential bit 15
        let ipma = full(
            b"ipma",
            1,
            &[
                &[0, 0, 0, 1][..],
                &70000u32.to_be_bytes(),
                &[1],
                &[0x80, 0x02],
            ]
            .concat(),
        );
        let iprp = bx(b"iprp", &[ipco, ipma].concat());
        let mut h = heif_with_items(&[70000]);
        // flags bit 0 selects the 16-bit form
        let mut raw = iprp.clone();
        let pos = raw.windows(4).position(|w| w == b"ipma").unwrap();
        raw[pos + 4 + 3] = 1;
        parse_iprp(payload(&raw), &mut h).unwrap();
        let item = h.item(70000).unwrap();
        assert!(item.assoc[0].essential);
        assert_eq!(item.assoc[0].index, 1);
        assert_eq!(h.item_props(item).unwrap().ops, vec![Op::Rotate(1)]);
    }

    #[test]
    fn property_index_out_of_range_is_malformed_when_used() {
        let ipco = bx(b"ipco", &bx(b"irot", &[1]));
        let ipma = full(b"ipma", 0, &[&[0, 0, 0, 1][..], &[0, 1, 1, 5]].concat());
        let iprp = bx(b"iprp", &[ipco, ipma].concat());
        let mut h = heif_with_items(&[1]);
        parse_iprp(payload(&iprp), &mut h).unwrap();
        assert!(matches!(
            h.item_props(h.item(1).unwrap()),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn invalid_known_property_is_malformed_when_used() {
        let ipco = bx(b"ipco", &bx(b"ispe", &[0, 0, 0, 0, 1]));
        let ipma = full(b"ipma", 0, &[&[0, 0, 0, 1][..], &[0, 1, 1, 1]].concat());
        let iprp = bx(b"iprp", &[ipco, ipma].concat());
        let mut h = heif_with_items(&[1]);
        parse_iprp(payload(&iprp), &mut h).unwrap();
        assert!(matches!(
            h.item_props(h.item(1).unwrap()),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn reference_queries() {
        let mut h = heif_with_items(&[1, 2, 3, 4]);
        let mk = |kind: &[u8; 4], from, to: &[u32]| Reference {
            kind: *kind,
            from,
            to: to.to_vec(),
        };
        h.refs = vec![
            mk(b"thmb", 2, &[1]),
            mk(b"dimg", 1, &[3, 4]),
            mk(b"auxl", 4, &[1]),
            mk(b"thmb", 3, &[9]),
        ];
        let thumbs: Vec<u32> = h.sources_referencing(1, b"thmb").map(|i| i.id).collect();
        assert_eq!(thumbs, vec![2]);
        assert_eq!(h.dimg_inputs(1), Some(&[3u32, 4][..]));
        assert_eq!(h.dimg_inputs(2), None);
        // item 4 is an auxiliary image of 1, but only an alpha auxC makes it an alpha plane
        assert!(h.alpha_item(1).is_none());
        h.props.push(Property::AuxC { alpha: true });
        h.items[3].assoc.push(Assoc {
            index: 0,
            essential: true,
        });
        assert_eq!(h.alpha_item(1).map(|i| i.id), Some(4));
    }

    #[test]
    fn limits_on_counts() {
        // more infe entries than MAX_ITEMS is refused rather than allocated
        let infe = |id: u32| {
            full(
                b"infe",
                3,
                &[&id.to_be_bytes()[..], &[0, 0, b'h', b'v', b'c', b'1', 0]].concat(),
            )
        };
        let mut body = vec![0, 0, 0, 0];
        for id in 1..=(MAX_ITEMS as u32 + 1) {
            body.extend(infe(id));
        }
        let iinf = bx(b"iinf", &[&[1u8, 0, 0, 0][..], &body].concat());
        assert!(matches!(
            parse_iinf(payload(&iinf)),
            Err(Error::LimitExceeded(_))
        ));
    }
}
