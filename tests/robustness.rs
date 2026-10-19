//! In-process randomized smoke test: mutated files must never make the crate panic.
//!
//! This is a cheap stand-in for the cargo-fuzz targets in `fuzz/`; it runs on every
//! `cargo test`. The generator is a fixed-seed xorshift so failures reproduce exactly.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use open_heic::{Limits, decode, decode_thumbnail, info};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(rng: &mut Rng, src: &[u8]) -> Vec<u8> {
    let mut d = src.to_vec();
    for _ in 0..1 + rng.below(6) {
        match rng.below(8) {
            0..=2 => {
                let i = rng.below(d.len());
                d[i] = rng.next() as u8;
            }
            3 => {
                let i = rng.below(d.len());
                d[i] ^= 1 << rng.below(8);
            }
            4 => {
                let keep = rng.below(d.len());
                d.truncate(keep.max(1));
            }
            5 => {
                // saturate a 4-byte field (sizes, counts, offsets)
                let i = rng.below(d.len().saturating_sub(4).max(1));
                for b in d.iter_mut().skip(i).take(4) {
                    *b = 0xFF;
                }
            }
            6 => {
                let i = rng.below(d.len());
                let n = rng.below(16);
                let junk: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                d.splice(i..i, junk);
            }
            _ => {
                let i = rng.below(d.len());
                let j = (i + rng.below(32)).min(d.len());
                d.drain(i..j);
            }
        }
        if d.is_empty() {
            d.push(0);
        }
    }
    d
}

/// Limits tight enough that a mutated size field cannot turn into a slow or huge decode.
fn small_limits() -> Limits {
    Limits {
        max_width: 2048,
        max_height: 2048,
        max_pixels: 1 << 20,
        max_memory_bytes: 64 << 20,
    }
}

static INNER_PANICS: AtomicUsize = AtomicUsize::new(0);

#[test]
fn mutated_files_never_panic() {
    // Count panics that the HEVC glue catches internally, so regressions in the decoder
    // dependency are visible even though they never reach the caller.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {
        INNER_PANICS.fetch_add(1, Ordering::Relaxed);
    }));

    let mut seeds: Vec<Vec<u8>> = vec![
        QUAD.to_vec(),
        grid(2, 2, 300, 230, true).build(),
        single(&[(p_irot(1), true), (p_clap(80, 1, 60, 1, 0, 1, 0, 1), true)]),
    ];
    let q = quad_hevc();
    let mut alpha = Item::hevc(2, &q);
    alpha.props.push((p_auxc(ALPHA_URN), true));
    let mut thumb = Item::hevc(3, &q);
    thumb.props[1] = (p_ispe(80, 60), false);
    seeds.push(
        Spec::new(
            1,
            vec![
                Item::hevc(1, &q).with(p_colr_nclx(1, 13, 1, true), false),
                alpha,
                thumb,
            ],
        )
        .refer(b"auxl", 2, &[1])
        .refer(b"thmb", 3, &[1])
        .build(),
    );

    let limits = small_limits();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut ok = 0usize;
    for round in 0..10_000 {
        let seed = &seeds[round % seeds.len()];
        let data = mutate(&mut rng, seed);
        let result = std::panic::catch_unwind(|| {
            let _ = info(&data);
            let a = decode(&data, &limits).is_ok();
            let b = decode_thumbnail(&data, 64, &limits).is_ok();
            a || b
        });
        match result {
            Ok(decoded) => ok += usize::from(decoded),
            Err(_) => {
                std::panic::set_hook(default_hook);
                panic!("round {round} panicked for input {data:02x?}");
            }
        }
    }
    std::panic::set_hook(default_hook);
    println!(
        "10000 mutations: {ok} still decoded, {} panics contained inside the HEVC glue",
        INNER_PANICS.load(Ordering::Relaxed)
    );
    assert!(
        ok > 0,
        "the mutator never leaves a decodable file; the test is not exercising the decoder"
    );
}

#[test]
fn random_bytes_and_prefixes_never_panic() {
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    let limits = small_limits();
    for _ in 0..2000 {
        let n = rng.below(600);
        let mut data: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        // make some of them look like HEIF so the parser goes deeper than the brand check
        if n > 24 && rng.below(2) == 0 {
            data[4..8].copy_from_slice(b"ftyp");
            data[8..12].copy_from_slice(b"heic");
            data[0..4].copy_from_slice(&24u32.to_be_bytes());
        }
        let _ = info(&data);
        let _ = decode(&data, &limits);
        let _ = decode_thumbnail(&data, 128, &limits);
    }
}
