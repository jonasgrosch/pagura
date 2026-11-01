#![forbid(unsafe_code)]
//! Hosted native/WASI independent oracle controls for compatibility gaps.
use pagura::{Document, Limits, OpenOptions, Recovery, RenderOptions, Rgba8};
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
    let dpi: f64 = args.next().ok_or("dpi required")?.parse()?;
    let stream_bytes: usize = args
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or_else(|| Limits::default().stream_bytes);
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let original = fs::read(input)?;
    let document = Arc::new(Document::open(
        original.clone(),
        &OpenOptions {
            recovery,
            limits: Limits {
                stream_bytes,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )?);
    let context = pagura::render_context(Arc::clone(&document));
    let options = RenderOptions {
        background: Rgba8::WHITE,
        ..RenderOptions::from_dpi(dpi)
    };
    let pixels = context.render_page(document.page(0)?, &options)?;
    if document.original_bytes() != original {
        return Err("source bytes changed".into());
    }
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
