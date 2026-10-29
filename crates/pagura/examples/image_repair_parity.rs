#![forbid(unsafe_code)]
//! Hosted native/WASI oracle control for narrowly reported image repairs.
use pagura::{Document, OpenOptions, Recovery, RenderOptions, Rgba8};
use std::{error::Error, fs, sync::Arc};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("input required")?;
    let output = args.next().ok_or("output required")?;
    let recovery = match args.next().as_deref() {
        Some("repair") => Recovery::Repair,
        Some("strict") => Recovery::Strict,
        _ => return Err("expected strict or repair".into()),
    };
    let document = Arc::new(Document::open(
        fs::read(input)?,
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )?);
    let context = pagura::render_context(Arc::clone(&document));
    let options = RenderOptions {
        background: Rgba8::WHITE,
        ..RenderOptions::default()
    };
    let pixels = context.render_page(document.page(0)?, &options)?;
    let mut bytes = format!("P6\n{} {}\n255\n", pixels.width(), pixels.height()).into_bytes();
    bytes.extend(pixels.to_rgb_over(Rgba8::WHITE));
    // These are fresh hosted scratch paths, never document-selected paths.
    std::io::Write::write_all(
        &mut fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?,
        &bytes,
    )?;
    for diagnostic in document.diagnostics() {
        println!("{}", diagnostic.tag());
    }
    Ok(())
}
