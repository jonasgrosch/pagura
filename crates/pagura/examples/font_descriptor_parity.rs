#![forbid(unsafe_code)]
//! Source-owned native/WASI descriptor recovery contracts.
use pagura::{Document, OpenOptions, Recovery, RenderOptions, Rgba8};
use std::{error::Error, fs, io::Write, sync::Arc};

const REVISION: &str = match option_env!("PAGURA_QUAL_SOURCE_REVISION") {
    Some(value) => value,
    None => "local-unqualified",
};

struct Arguments {
    input: String,
    output: String,
    recovery: Recovery,
    dpi: f64,
    page: usize,
    text: bool,
}

impl Arguments {
    fn read() -> Result<Self, Box<dyn Error>> {
        let mut args = std::env::args().skip(1);
        let input = args.next().ok_or("input required")?;
        let output = args.next().ok_or("output required")?;
        let recovery = match args.next().as_deref() {
            Some("strict") => Recovery::Strict,
            Some("repair") => Recovery::Repair,
            _ => return Err("strict or repair required".into()),
        };
        let dpi = args.next().ok_or("dpi required")?.parse()?;
        let page = args.next().ok_or("page required")?.parse()?;
        let text = match args.next().as_deref() {
            Some("text") => true,
            Some("paint") => false,
            _ => return Err("text or paint required".into()),
        };
        if args.next().is_some() {
            return Err("unexpected argument".into());
        }
        Ok(Self {
            input,
            output,
            recovery,
            dpi,
            page,
            text,
        })
    }
}

fn write_frame(path: &str, pixmap: &pagura::Pixmap) -> Result<(), Box<dyn Error>> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    write!(file, "P6\n{} {}\n255\n", pixmap.width(), pixmap.height())?;
    file.write_all(&pixmap.to_rgb_over(Rgba8::WHITE))?;
    Ok(())
}

fn write_bytes(path: &str, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)?;
    Ok(())
}

fn text_contract(context: &pagura::RenderContext, page: &pagura::Page) {
    match context.text_page(page) {
        Ok(text) => {
            println!("text-glyph-count={}", text.glyphs().len());
            for glyph in text.glyphs() {
                let p = glyph.quad.0;
                println!(
                    "text-quad=[{},{},{},{},{},{},{},{}]",
                    p[0].x, p[0].y, p[1].x, p[1].y, p[2].x, p[2].y, p[3].x, p[3].y
                );
            }
        }
        Err(error) => println!("text-error={error:?}"),
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Arguments::read()?;
    println!("qualification-source={REVISION}");
    println!("qualification-page={}", args.page);
    let original = fs::read(&args.input)?;
    let document = Arc::new(Document::open(
        original.clone(),
        &OpenOptions {
            recovery: args.recovery,
            ..OpenOptions::default()
        },
    )?);
    let context = pagura::render_context(Arc::clone(&document));
    let page = document.page(args.page)?;
    let options = RenderOptions::from_dpi(args.dpi);
    let white = context.render_page(page, &options);
    if document.original_bytes() != original {
        return Err("source bytes changed after rendering".into());
    }
    println!("source-bytes-preserved=pass");
    for diagnostic in document.diagnostics() {
        println!("{}", diagnostic.tag());
    }
    let white = white?;
    let transparent = context.render_page(
        page,
        &RenderOptions {
            background: Rgba8::TRANSPARENT,
            ..options
        },
    )?;
    if args.text {
        text_contract(&context, page);
    }
    if document.original_bytes() != original {
        return Err("source bytes changed after additional consumers".into());
    }
    // Target output is created only after both render operations succeed.
    write_frame(&args.output, &white)?;
    write_bytes(&format!("{}.rgba", args.output), white.data())?;
    write_bytes(
        &format!("{}.transparent.rgba", args.output),
        transparent.data(),
    )?;
    Ok(())
}
