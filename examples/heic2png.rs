//! Decode HEIC files to PNG and print timings.
//!
//! ```text
//! cargo run --release --example heic2png -- <out_dir> <file.heic>...
//! ```
//!
//! For every input this writes `<stem>.thumb.png` (the 256 px thumbnail the way a file
//! manager would ask for it) and `<stem>.full.png` (the primary image). The PNG encoder is
//! the `png` dev-dependency; the library itself has no encoder.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::time::Instant;

use open_heic::{Image, Limits};

fn write_png(path: &Path, img: &Image) -> Result<(), Box<dyn std::error::Error>> {
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.write_header()?.write_image_data(&img.rgba)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(out_dir), inputs) = (args.next(), args.collect::<Vec<_>>()) else {
        eprintln!("usage: heic2png <out_dir> <file.heic>...");
        std::process::exit(2);
    };
    if inputs.is_empty() {
        eprintln!("usage: heic2png <out_dir> <file.heic>...");
        std::process::exit(2);
    }
    let out_dir = Path::new(&out_dir);
    std::fs::create_dir_all(out_dir)?;
    let limits = Limits::default();

    for input in inputs {
        let stem = Path::new(&input)
            .file_stem()
            .map_or_else(|| "image".into(), |s| s.to_string_lossy().into_owned());
        let data = std::fs::read(&input)?;
        match open_heic::info(&data) {
            Ok(i) => println!(
                "{stem}: {} bytes, {}x{} {:?} {}-bit, alpha {}, thumbnail {}",
                data.len(),
                i.width,
                i.height,
                i.codec,
                i.bit_depth,
                i.has_alpha,
                i.has_thumbnail
            ),
            Err(e) => {
                println!("{stem}: info failed: {e}");
                continue;
            }
        }
        let t = Instant::now();
        match open_heic::decode_thumbnail(&data, 256, &limits) {
            Ok(img) => {
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                write_png(&out_dir.join(format!("{stem}.thumb.png")), &img)?;
                println!("  thumbnail {}x{} in {ms:.1} ms", img.width, img.height);
            }
            Err(e) => println!("  thumbnail failed: {e}"),
        }
        let t = Instant::now();
        match open_heic::decode(&data, &limits) {
            Ok(img) => {
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                write_png(&out_dir.join(format!("{stem}.full.png")), &img)?;
                println!("  full      {}x{} in {ms:.0} ms", img.width, img.height);
            }
            Err(e) => println!("  full failed: {e}"),
        }
    }
    Ok(())
}
