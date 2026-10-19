# Test fixtures

Everything here was made by the project owner with free tools; none of it is third-party
content, so it is covered by the crate's own licence (MIT OR Apache-2.0).

## `quad.heic` (513 bytes)

A 160x120 HEIF still image: four flat quadrants (red top-left, green top-right, blue
bottom-left, yellow bottom-right), one HEVC Main-profile picture. Copied unchanged from the
Thumbwright repository (`crates/tw-core/tests/fixtures/heic/quad.heic`, same owner), where
its provenance is recorded: made with ffmpeg 9.0.1 (libx265, `-crf 14`) from a generated
PNG (`make_quad_png.py out.png 160 120`) and wrapped into a single-item HEIF container by
Thumbwright's `scripts/fixtures/hevc_to_heic.py`.

The synthetic files used by `tests/pipeline.rs` (grids, `irot`, `imir`, `clap`, alpha,
thumbnails, `idat`, multi-extent items, ...) are not stored: `tests/common/mod.rs` assembles
them at test time around this picture's HEVC data.

## `*.hevc` (about 2.5 kB each)

Single-picture HEVC Annex B streams, 64x48 (`quad_50x34` is 50x34), four flat quadrants of
deliberately unsaturated colours (`#c83c28`, `#3cb45a`, `#3250c8`, `#dcc83c`) so a wrong
matrix or range shows up instead of clamping. `tests/common/mod.rs` wraps them into HEIF
items (the `hvcC` is built from the stream's own parameter sets). They were produced by
`make-streams.sh` with ffmpeg 9.0.1 (gyan.dev full build, libx265):

| file | pixel format | RGB to YCbCr conversion |
|---|---|---|
| `quad601_8.hevc` | yuv420p | BT.601, limited range |
| `quad709_8.hevc` | yuv420p | BT.709, limited range |
| `quad601full_8.hevc` | yuv420p | BT.601, full range |
| `quad2020_10.hevc` | yuv420p10le | BT.2020 non-constant, limited range |
| `quad601_gray.hevc` | gray (4:0:0) | luma only, full range |
| `quad_50x34.hevc` | yuv420p | BT.601 limited; 56x40 coded, 50x34 conformance window |

The container's `colr` box, not the stream's VUI, selects the matrix when decoding, exactly
as for real HEIC files, so the tests attach the matching (or a deliberately wrong) `colr`.

## Local-only files

`tests/local.rs` reads the owner's iPhone photographs from the directory named by
`OPEN_HEIC_LOCAL_DIR`. Those files are never committed or published.
