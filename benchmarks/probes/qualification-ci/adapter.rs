#![forbid(unsafe_code)]
//! Qualification-only facade adapter: cold API, retained-context warm API and pixels.
use std::{error::Error, fs, io::Write, sync::Arc, time::Instant};

const SOURCE_REVISION: &str = env!("PAGURA_QUAL_SOURCE_REVISION");

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let input = arguments.next().ok_or("input required")?;
    let output = arguments.next().ok_or("output required")?;
    let dpi: f64 = arguments.next().ok_or("dpi required")?.parse()?;
    let page: usize = arguments.next().ok_or("page required")?.parse()?;
    let recovery = match arguments.next().as_deref() {
        Some("strict") => pagura::Recovery::Strict,
        Some("repair") => pagura::Recovery::Repair,
        _ => return Err("strict or repair required".into()),
    };
    let warmups: usize = arguments.next().ok_or("warmup count required")?.parse()?;
    let samples: usize = arguments.next().ok_or("sample count required")?.parse()?;
    if warmups > 100 || samples > 1000 || arguments.next().is_some() {
        return Err("invalid qualification arguments".into());
    }
    let start = Instant::now();
    let bytes: Arc<[u8]> = fs::read(input)?.into();
    let document = Arc::new(pagura::Document::open(
        bytes,
        &pagura::OpenOptions { recovery, ..Default::default() },
    )?);
    let context = pagura::render_context(document);
    let count = context.document().page_count()?;
    if page >= count { return Err("page outside document".into()); }
    let open_ns = start.elapsed().as_nanos();
    let options = pagura::RenderOptions {
        background: pagura::Rgba8::WHITE,
        ..pagura::RenderOptions::from_dpi(dpi)
    };
    let render_start = Instant::now();
    let pixels = context.render_page(context.document().page(page)?, &options)?;
    let render_ns = render_start.elapsed().as_nanos();
    let width = pixels.width();
    let height = pixels.height();
    let expected = pixels.to_rgb_over(pagura::Rgba8::WHITE);
    drop(pixels);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(output)?;
    write!(file, "P6\n{width} {height}\n255\n")?;
    file.write_all(&expected)?;
    file.flush()?;
    drop(file);
    let mut warm_ns = Vec::with_capacity(samples);
    for index in 0..warmups.checked_add(samples).ok_or("sample count overflow")? {
        let warm_start = Instant::now();
        let pixels = context.render_page(context.document().page(page)?, &options)?;
        let elapsed = warm_start.elapsed().as_nanos();
        if pixels.width() != width || pixels.height() != height
            || pixels.to_rgb_over(pagura::Rgba8::WHITE) != expected {
            return Err("cold/warm full pixel mismatch".into());
        }
        if index >= warmups { warm_ns.push(elapsed); }
    }
    let diagnostics = context.document().diagnostics().iter()
        .map(|diagnostic| format!("{:?}", diagnostic.tag())).collect::<Vec<_>>().join(",");
    println!("{{\"schema\":1,\"source_revision\":\"{SOURCE_REVISION}\",\"open_ns\":{open_ns},\"render_ns\":{render_ns},\"cold_api_ns\":{},\"warm_ns\":{warm_ns:?},\"width\":{width},\"height\":{height},\"diagnostics\":[{diagnostics}]}}", open_ns + render_ns);
    Ok(())
}
