# Specification sources

`open-heic` is a clean-room implementation: it was written from the published standards
and public documentation listed here. **No source code from libheif (LGPL) or from
`imazen/heic` (AGPL) was read, copied or adapted.** The only prior code consulted is the
project owner's own spike (`heicspike`, a container parser written from the same standards).

Clause numbers are quoted where they are certain; elsewhere the clause *title* is given so
the reference can be found in any edition (2017 / 2019 / 2022 of ISO/IEC 23008-12).

## ISO/IEC 14496-12 (ISO base media file format)

| used for | clause |
|---|---|
| Box header, 32-bit size, `size == 1` (64-bit `largesize`), `size == 0` (to end of file), FullBox `version`/`flags` | 4.2 Object structure |
| `ftyp` major / compatible brands | 4.3 File type box |
| `hdlr` handler type `pict` | 8.4.3 Handler reference box |
| `meta` container | 8.11.1 Meta box |
| `iloc` v0/v1/v2: offset/length/base/index sizes, `construction_method` (0 file, 1 `idat`, 2 item offset), extents, `extent_length == 0` | 8.11.3 Item location box |
| `pitm` v0/v1 | 8.11.4 Primary item box |
| `iinf` / `infe` v2/v3 (`item_ID`, `item_protection_index`, `item_type`) | 8.11.6 Item information box |
| `idat` | 8.11.11 Item data box |
| `iref` v0/v1 (`from_item_ID`, `reference_count`, `to_item_ID`) | 8.11.12 Item reference box |
| `clap` clean aperture: fractions `cleanApertureWidth/Height`, `horizOff`, `vertOff`; crop `left = (W - cw)/2 + horizOff` | 12.1.4 Clean aperture |

## ISO/IEC 23008-12 (HEIF)

| used for | clause |
|---|---|
| Brands `heic`, `heix`, `heim`, `heis`, `hevc*`, `mif1`, `msf1` | Brands (clause 10) |
| Items, `pitm`, hidden items, `iloc`/`iinf` constraints for image items | 6.2 / 9 (storage of image items) |
| Item properties in `ipco`, association and the `essential` bit in `ipma` v0/v1, flags bit 0 (15-bit indices); unknown essential property makes the item unusable | 6.5 Item properties (6.5.1) |
| `ispe` image spatial extents | 6.5 (image spatial extents) |
| `irot`: `angle * 90` degrees anti-clockwise | 6.5 (image rotation) |
| `imir`: field `axis`/`mode` bit 0 | 6.5.12 Image mirror |
| `pixi` pixel information, `colr` (`nclx`, `rICC`, `prof`), `auxC` auxiliary type URN | 6.5 (pixel information, colour information, auxiliary type) |
| Transformative properties applied in `ipma` order | 6.5.1 |
| Derived image items: `iden` identity, `iovl` overlay (detected, unsupported), `grid` (`ImageGrid`: version, flags bit 0 selects 16/32-bit sizes, `rows_minus_one`, `columns_minus_one`, output size) | 6.6.2 Derived image items |
| Item reference types `thmb` (thumbnail to master), `dimg` (derived to inputs), `auxl` (auxiliary to master), `cdsc` | 6.6 / 7 Item references |
| Alpha as auxiliary image: `auxC` with alpha URN | auxiliary image items (alpha plane) |
| Default colour handling when no `nclx` is present | HEIF colour information; defaults per ITU-T H.273 |

Alpha auxiliary URNs recognised: `urn:mpeg:mpegB:cicp:systems:auxiliary:alpha` and
`urn:mpeg:hevc:2015:auxid:1`. These are public identifiers.

### `imir` axis meaning

The two editions of the standard word the axis semantics differently. The implementation
follows the 2022 wording as quoted in the public header of libavif (`avif.h`, BSD-2-Clause,
comment on `avifImageMirror`): axis 0 exchanges the top and bottom parts of the image, axis 1
exchanges left and right. Only that comment was consulted; no code was taken.

## ISO/IEC 14496-15 (carriage of NAL unit structured video)

* 8.3.3 HEVC decoder configuration record (`hvcC`): `lengthSizeMinusOne`, `numOfArrays`,
  parameter-set arrays, `chroma_format_idc`, `bit_depth_luma_minus8`.
* NAL-unit length-prefixed sample format (the item payload) and its conversion to an Annex B
  byte stream.

## ITU-T H.265 (HEVC)

* Annex B: byte stream format, start codes, emulation prevention.
* 7.3.1.2: NAL unit header (`nuh_layer_id`; non-zero layers are ignored).
* 7.3.2.2 / 7.4.3.2: sequence parameter set syntax and semantics (up to the bit depths):
  used only to read picture size, conformance window, chroma format and bit depth *before*
  decoding, to enforce limits.
* 7.3.3: `profile_tier_level` syntax (skipped to reach the fields above).
* 9.2: Exp-Golomb `ue(v)` coding.
* Conformance window offsets are in chroma sample units (`SubWidthC`, `SubHeightC`).

Pixel decoding itself is done by the `rust_h265` crate.

## Colour

* ITU-T H.273 (CICP): `MatrixCoefficients` code points 0, 1, 2, 4, 5, 6, 7, 9, 10 (Table 4);
  `full_range_flag`.
* ITU-R BT.601-7, BT.709-6, BT.2020-2, SMPTE ST 240: luma coefficients `Kr`, `Kb`
  (BT.601 0.299/0.114, BT.709 0.2126/0.0722, BT.2020 0.2627/0.0593, SMPTE 240M 0.212/0.087,
  FCC 0.30/0.11) and the limited-range code ranges (219 / 224 steps at 8 bit, scaled by
  `2^(bit_depth-8)`).
* Chroma is upsampled with a 3:1 triangle filter assuming chroma samples centred between
  luma samples.

## Test material

See `tests/fixtures/README.md`. Everything committed was made by the project owner.
