# open-heic

A permissively licensed (MIT OR Apache-2.0), pure-Rust, `#![forbid(unsafe_code)]` reader for
HEIC/HEIF still images, built for untrusted input.

It exists so that applications that only want to *show* a HEIC photo (file-manager
thumbnails, indexers, upload previews) do not need Microsoft's Store codecs, a C/C++
toolchain, or a copyleft library: `cargo add open-heic` and decode.

```rust
use open_heic::{decode, decode_thumbnail, info, Limits};

let bytes = std::fs::read("IMG_0001.HEIC")?;
let limits = Limits::default();

let i = info(&bytes)?;                                   // container only, nothing decoded
println!("{}x{} {:?} alpha={} thumb={}", i.width, i.height, i.codec, i.has_alpha, i.has_thumbnail);

let thumb = decode_thumbnail(&bytes, 256, &limits)?;     // embedded thumbnail: a few milliseconds
let full = decode(&bytes, &limits)?;                     // primary image
// thumb.rgba / full.rgba: 8-bit RGBA, straight alpha, upright (irot/imir/clap applied)
```

Pixels are returned as the file's own colour space (typically sRGB or Display P3): no ICC or
gamut conversion is applied.

## API

| item | |
|---|---|
| `info(&[u8]) -> Result<Info, Error>` | size after orientation, bit depth, codec, alpha / thumbnail presence |
| `decode(&[u8], &Limits) -> Result<Image, Error>` | the primary image |
| `decode_thumbnail(&[u8], max_edge, &Limits) -> Result<Image, Error>` | smallest embedded thumbnail whose long edge is `>= max_edge`, else the largest thumbnail, else the primary image; never upscales |
| `Limits` | `max_width`, `max_height`, `max_pixels`, `max_memory_bytes` (defaults 16384, 16384, 256 000 000, 1 GiB) |
| `Image` | `width`, `height`, `rgba: Vec<u8>` |
| `Error` (`#[non_exhaustive]`) | `NotHeif`, `Unsupported(&'static str)`, `LimitExceeded(&'static str)`, `Malformed(&'static str)`, `Hevc(String)` |

`Error::Unsupported` and `Error::NotHeif` mean "use another decoder"; the other variants
mean the file is damaged, too large for your limits, or rejected by the HEVC decoder.

## What is supported

| area | status |
|---|---|
| Brands | `heic`, `heix`, `heim`, `heis`, `hevc`, `hevx`, `hevm`, `hevs`, `mif1`, `mif2`, `msf1`, `miaf`, `avif`, `avis` |
| Item storage | `iloc` v0/v1/v2, construction method 0 (file) and 1 (`idat`), multi-extent items; method 2 and external data: `Unsupported` |
| Items | `iinf`/`infe` v2/v3, `pitm`, `iref` v0/v1 (`thmb`, `dimg`, `auxl`, `cdsc`) |
| Properties | `hvcC`, `ispe`, `irot`, `imir`, `clap`, `colr` (nclx used; ICC recognised, not applied), `pixi`, `auxC`; unknown *essential* property: `Unsupported`, unknown non-essential: ignored |
| Derived images | `grid` (descriptor in the file or in `idat`), `iden`; `iovl` (overlay): `Unsupported` |
| Codec | HEVC Main / Main 10, 4:2:0 (what `rust_h265` decodes). 10-bit is scaled to 8-bit |
| Colour | BT.601 (default, limited range), BT.709, BT.2020 NCL, SMPTE 240M, FCC; limited and full range, from the `colr` `nclx` box |
| Transforms | `clap`, `irot`, `imir` applied in `ipma` order |
| Alpha | `auxl` + alpha `auxC` image, decoded as 4:2:0 HEVC luma and applied as straight alpha |
| AV1 (AVIF), JPEG-coded and uncompressed items | detected (`Info::codec`), decode returns `Unsupported` |
| Anything that is not an ISO base media file | `NotHeif` (for example a JPEG renamed `.heic`) |

### Known limitations

* **Apple alpha is not applied.** Apple stores alpha as a *monochrome* (4:0:0) HEVC image and
  `rust_h265` cannot decode 4:0:0. `info()` reports `has_alpha: true`, but `decode()` returns
  the picture fully opaque. A 4:0:0 *primary* image is `Unsupported`.
* HEVC profiles beyond Main / Main 10 4:2:0 (4:2:2, 4:4:4, 12-bit, RExt tools) are
  `Unsupported`, whatever `rust_h265` reports.
* No colour management and no Exif/XMP reading. The orientation comes from `irot`/`imir`
  only, as HEIF specifies (the Exif orientation tag in the file is ignored).
* Single-threaded. A 12-megapixel iPhone grid decodes in roughly one second in release
  builds; an embedded 320x240 thumbnail in about 6 ms.
* The HEVC decoder is young. This crate contains any panic it raises, which needs
  `panic = "unwind"` (the default); with `panic = "abort"` such a panic aborts the process.
  In our 10 000-mutation smoke test a handful of corrupted files trigger such panics inside
  `rust_h265`.

## Safety for untrusted input

* `#![forbid(unsafe_code)]`; no C/C++ build, no `-sys` crates.
* All box parsing is bounds-checked; sizes, counts and offsets from the file use checked
  arithmetic; item, property and reference counts are capped.
* `Limits` are enforced from the `ispe`, grid descriptor and SPS values **before** the
  matching allocation. The memory estimate includes the output canvas, a decoded tile or
  picture with the HEVC decoder's working copies, and the transform buffer.
* Grid tiles must cover the output exactly, which also bounds total decode work to a small
  multiple of the output area.
* Fuzz targets live in `fuzz/` (`cargo +nightly fuzz run decode`); a mutation smoke test
  (10 000 mutated files per run, fixed seed) runs with `cargo test`.

See [SECURITY.md](SECURITY.md).

## Patents

HEVC (H.265) may be covered by patents held by third parties (for example through the
Access Advance and Via LA pools). **This crate grants copyright licences only; no patent
licence is granted or implied for HEVC.** Whether you need a patent licence to decode HEVC
depends on where and how you distribute software or devices, and is your responsibility.

## Licence

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work, as defined in the Apache-2.0 licence, shall be dual
licensed as above, without any additional terms or conditions.

The implementation was written from the published specifications; see
[docs/SOURCES.md](docs/SOURCES.md). No code from libheif (LGPL) or `imazen/heic` (AGPL) was
read or used.

## Development

```text
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
OPEN_HEIC_LOCAL_DIR=/path/to/photos cargo test --release -- --ignored   # owner's local files
cargo run --release --example heic2png -- out_dir a.heic b.heic        # PNG + timings
```

Minimum supported Rust version: 1.85 (edition 2024). Tested with the stable toolchains
1.98.1 and 1.99.1.

The example uses the `png` crate as a **dev-dependency only**; the library has a single
dependency, [`rust_h265`](https://crates.io/crates/rust_h265) (MIT OR Apache-2.0).
