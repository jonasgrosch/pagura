#![forbid(unsafe_code)]
//! Hosted PDF and public Function API controls for the bounded Type 4 proof.

use pagura::{Document, OpenOptions, Recovery, RenderOptions, Rgba8};
use pagura_color::{CalculatorFunction, Function};
use std::{error::Error, fs, sync::Arc};

type Arguments = std::iter::Skip<std::env::Args>;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("pdf") => pdf(args),
        Some("function") => function(args),
        _ => Err("expected pdf or function operation".into()),
    }
}

fn function(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let input = args.next().ok_or("input required")?;
    let outputs: usize = args.next().ok_or("output dimension required")?.parse()?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    // The source-owned API vectors all explicitly declare this Domain.
    // These are test callers of the public constructor, not PDF dictionaries.
    Function::calculator(
        CalculatorFunction {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]; outputs],
            program: Arc::from(fs::read(input)?),
        },
        &pagura::Limits::default(),
    )?;
    Err("the explicitly unsupported API vector unexpectedly constructed".into())
}

fn pdf(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let input = args.next().ok_or("input required")?;
    let output = args.next().ok_or("output required")?;
    let recovery = match args.next().as_deref() {
        Some("repair") => Recovery::Repair,
        Some("strict") => Recovery::Strict,
        _ => return Err("expected strict or repair".into()),
    };
    let dpi: f64 = args.next().ok_or("dpi required")?.parse()?;
    let page: usize = args.next().ok_or("page required")?.parse()?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let original = fs::read(input)?;
    let document = Arc::new(Document::open(
        original.clone(),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )?);
    let context = pagura::render_context(Arc::clone(&document));
    let options = RenderOptions {
        background: Rgba8::WHITE,
        ..RenderOptions::from_dpi(dpi)
    };
    if page > 0 {
        drop(context.render_page(document.page(0)?, &options)?);
        println!("prior-page-rendered=0");
    }
    let pixels = context.render_page(document.page(page)?, &options)?;
    let transparent = context.render_page(
        document.page(page)?,
        &RenderOptions {
            background: Rgba8::TRANSPARENT,
            ..options
        },
    )?;
    if document.original_bytes() != original {
        return Err("render changed original source bytes".into());
    }
    let mut ppm = format!("P6\n{} {}\n255\n", pixels.width(), pixels.height()).into_bytes();
    ppm.extend(pixels.to_rgb_over(Rgba8::WHITE));
    // Engine faults happen before any output is created. These fresh scratch
    // paths are selected by the hosted runner, never by an input document.
    for (path, bytes) in [
        (output.clone(), ppm.as_slice()),
        (format!("{output}.rgba"), pixels.data()),
        (format!("{output}.transparent.rgba"), transparent.data()),
    ] {
        std::io::Write::write_all(
            &mut fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?,
            bytes,
        )?;
    }
    for diagnostic in document.diagnostics() {
        println!("{}", diagnostic.tag());
    }
    Ok(())
}
