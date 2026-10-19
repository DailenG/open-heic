#![no_main]

use libfuzzer_sys::fuzz_target;
use open_heic::Limits;

const LIMITS: Limits = Limits {
    max_width: 4096,
    max_height: 4096,
    max_pixels: 4 << 20,
    max_memory_bytes: 128 << 20,
};

fuzz_target!(|data: &[u8]| {
    // First two bytes choose the requested edge, the rest is the file.
    let Some((edge, file)) = data.split_first_chunk::<2>() else {
        return;
    };
    let _ = open_heic::decode_thumbnail(file, u32::from(u16::from_le_bytes(*edge)), &LIMITS);
});
