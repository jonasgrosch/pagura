#![forbid(unsafe_code)]
//! Conversion adapter built against the same facade/features for native and WASI.
//! Boundaries match `pagura convert`: open/enumerate, load+compile+paint, emit.

use pagura::{Document, OpenOptions, Recovery, RenderOptions, Rgba8};
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let mut password = Vec::new();
    if let Some(index) = args.iter().position(|arg| arg == "--password") {
        password = args
            .get(index + 1)
            .ok_or("missing password")?
            .as_bytes()
            .to_vec();
        args.drain(index..=index + 1);
    }
    let geometry = args.first().is_some_and(|arg| arg == "--geometry");
    if geometry {
        args.remove(0);
    }
    if args.len() != if geometry { 4 } else { 5 } {
        return Err(
            "expected INPUT OUTDIR DPI FIRST LAST, or --geometry INPUT DPI FIRST LAST".into(),
        );
    }
    let offset = usize::from(!geometry);
    let dpi: f64 = args[1 + offset].parse()?;
    let first: usize = args[2 + offset].parse()?;
    let last: usize = args[3 + offset].parse()?;
    if !dpi.is_finite() || dpi <= 0.0 || first > last {
        return Err("invalid DPI or page range".into());
    }
    let started = Instant::now();
    let options = OpenOptions {
        recovery: Recovery::Repair,
        password,
        ..OpenOptions::default()
    };
    let mut bytes = Vec::new();
    std::fs::File::open(&args[0])?
        .take(options.limits.file_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > options.limits.file_bytes {
        return Err("input exceeds file-byte budget".into());
    }
    let document = Arc::new(Document::open(bytes, &options)?);
    let context = pagura::render_context(document.clone());
    let count = document.page_count()?;
    if last >= count {
        return Err(format!("page range outside the document's {count} pages").into());
    }
    let opened = started.elapsed().as_nanos();
    let settings = RenderOptions::from_dpi(dpi);
    let mut pages = Vec::new();
    for page in first..=last {
        if geometry {
            let (width, height) = pagura::device_size(document.page(page)?, &settings)?;
            pages.push(format!(
                "{{\"page\":{page},\"width\":{width},\"height\":{height}}}"
            ));
            continue;
        }
        let render = Instant::now();
        let image = context.render_page(document.page(page)?, &settings)?;
        let render = render.elapsed().as_nanos();
        let emit = Instant::now();
        let target = std::path::Path::new(&args[1]).join(format!("page-{:04}.ppm", page + 1));
        let mut file = std::fs::File::create_new(target)?;
        write!(file, "P6\n{} {}\n255\n", image.width(), image.height())?;
        file.write_all(&image.to_rgb_over(Rgba8::WHITE))?;
        drop(file);
        pages.push(format!(
            "{{\"page\":{page},\"render_ns\":{render},\"emit_ns\":{}}}",
            emit.elapsed().as_nanos()
        ));
    }
    if geometry {
        println!("{{\"pages\":[{}]}}", pages.join(","));
    } else {
        println!(
            "{{\"schema\":1,\"open_ns\":{opened},\"pages\":[{}],\"total_ns\":{}}}",
            pages.join(","),
            started.elapsed().as_nanos()
        );
    }
    Ok(())
}
