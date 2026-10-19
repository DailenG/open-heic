# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-19

First release.

### Added

* `info`, `decode` and `decode_thumbnail` over HEIC/HEIF still images, returning 8-bit
  straight-alpha RGBA with `clap`, `irot` and `imir` applied.
* HEIF container: `ftyp` brand detection, `meta`/`hdlr`/`pitm`/`iinf`/`iloc` (v0-v2, file and
  `idat` storage, multi-extent items)/`iref`/`iprp`, `hvcC`, `ispe`, `irot`, `imir`, `clap`,
  `colr`, `pixi` and `auxC` properties, essential-property handling.
* Derived images: `grid` (descriptor in the file or in `idat`) and `iden`.
* HEVC decoding through `rust_h265` with Annex B reassembly, SPS vetting before decode, and
  panic containment.
* YCbCr to RGB for BT.601 / BT.709 / BT.2020 / SMPTE 240M / FCC matrices, limited and full
  range, 8-bit and 10-bit, with triangle-filtered chroma upsampling.
* Alpha auxiliary images (4:2:0 HEVC luma) as straight alpha.
* `Limits` (dimensions, pixels, memory) enforced from descriptors before allocation.
* AV1, JPEG-coded and uncompressed items are detected and reported as `Unsupported`; files
  that are not ISO base media files are `NotHeif`.
* Unit tests per box parser, end-to-end tests on committed fixtures, a 10 000-mutation smoke
  test, local-only tests for real iPhone files (`#[ignore]`, `OPEN_HEIC_LOCAL_DIR`),
  `cargo-fuzz` targets, and the `heic2png` example.

### Known limitations

* Apple's monochrome (4:0:0) alpha planes are not decodable by `rust_h265`: such images are
  returned opaque while `Info::has_alpha` is `true`.
* HEVC beyond Main / Main 10 4:2:0, image overlays (`iovl`), `iloc` construction method 2 and
  external item data are `Unsupported`.
