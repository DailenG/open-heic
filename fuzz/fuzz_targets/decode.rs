#![no_main]

use libfuzzer_sys::fuzz_target;
use open_heic::Limits;

// Tight limits keep each execution fast and stop a mutated size field from turning into a
// legitimately huge (but valid) decode; the limit checks themselves are part of the target.
const LIMITS: Limits = Limits {
    max_width: 4096,
    max_height: 4096,
    max_pixels: 4 << 20,
    max_memory_bytes: 128 << 20,
};

fuzz_target!(|data: &[u8]| {
    let _ = open_heic::decode(data, &LIMITS);
});
