# Security policy

`open-heic` parses files from untrusted sources. Crashes, hangs, unbounded memory use and
out-of-limit allocations on crafted input are treated as security bugs.

## Design

* `#![forbid(unsafe_code)]` in this crate; the only dependency, `rust_h265`, is also safe
  Rust but is a young decoder, so its calls are wrapped in `catch_unwind`.
* All parsing goes through a bounds-checked reader; sizes, offsets and counts taken from the
  file use checked or saturating arithmetic.
* Structural caps independent of `Limits`: at most 131 072 boxes per container, 65 536 items,
  32 768 properties, about one million property associations and reference targets, 256 x 256
  grid tiles (the format's own maximum).
* An item's data is never concatenated beyond the size of the file (overlapping `iloc`
  extents cannot multiply a small file).
* Only the HEVC base layer (`nuh_layer_id == 0`) is decoded.

## Limits

`Limits` bounds every decode. The defaults suit desktop thumbnailing; lower them for
services.

| field | default | enforced |
|---|---|---|
| `max_width` | 16384 | on the `ispe` / grid descriptor of every item decoded, on every SPS in the stream, and on the output after rotation |
| `max_height` | 16384 | same |
| `max_pixels` | 256 000 000 | same, as `width * height` |
| `max_memory_bytes` | 1 GiB | estimated peak heap (output canvas + one decoded picture with the decoder's working copies + the orientation/crop buffer), checked before each allocation |

Checks use the descriptor values, never the amount of data present, and happen **before**
the allocation they guard. Exceeding one returns `Error::LimitExceeded`.

Work is bounded too: grid tiles must cover the output exactly (no tile may lie wholly outside
it), so the total number of decoded pixels is below about four times the output area.

Not bounded: wall-clock time of the HEVC decoder on pathological bitstreams that are within
the pixel limits. Run decodes of untrusted files in a worker with a deadline if that matters.

`panic = "abort"` builds lose the panic containment around `rust_h265`: a panic inside the
decoder would abort the process. Use `panic = "unwind"` (the default) for untrusted input.

## Testing

* `cargo test` runs unit tests per box parser, end-to-end tests, and a fixed-seed
  10 000-file mutation smoke test (`tests/robustness.rs`) which asserts that no panic ever
  reaches the caller.
* `fuzz/` contains `cargo-fuzz` targets `info`, `decode` and `decode_thumbnail`
  (`cargo +nightly fuzz run decode`).

## Reporting a vulnerability

Please report privately, not in a public issue: use GitHub's private vulnerability reporting
(<https://github.com/DailenG/open-heic/security/advisories/new>). Include the smallest
input that reproduces the problem (a fuzz artifact is ideal) and the crate version. You
should get an acknowledgement within a few days; fixes are released as patch versions and
announced in `CHANGELOG.md`.
