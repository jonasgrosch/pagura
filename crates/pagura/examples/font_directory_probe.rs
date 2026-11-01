#![forbid(unsafe_code)]
//! Explicit public font/render contracts; no Strict-first scan inference.
use pagura::{
    Diagnostic, Document, Error, Limits, OpenOptions, Recovery, RenderOptions, Result, Rgba8,
};
use pagura_core::Diagnostics;
use pagura_font::{FontFormat, FontProgram, GlyphId};
use std::{env, fs, path::Path, process::ExitCode};

fn read(path: &str) -> Result<Vec<u8>> {
    fs::read(path).map_err(|_| Error::invalid_argument("probe input cannot be read"))
}
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|_| Error::invalid_argument("probe output cannot be written"))
}
fn diagnostics(items: &[Diagnostic]) {
    for item in items {
        println!(
            "DIAGNOSTIC {} {:?} {:?}",
            item.tag(),
            item.position,
            item.object
        );
    }
}
fn limits(name: &str) -> Result<Limits> {
    Ok(match name {
        "default" => Limits::default(),
        "work" => Limits {
            font_work: 1,
            ..Limits::default()
        },
        "entries" => Limits {
            font_entries: 0,
            ..Limits::default()
        },
        "input" => Limits {
            decoded_bytes: 1,
            ..Limits::default()
        },
        _ => return Err(Error::invalid_argument("unknown probe budget")),
    })
}
fn font(bytes: &[u8], format: FontFormat, recovery: Recovery, limits: &Limits) -> Result<()> {
    let mut notes = Diagnostics::new(recovery);
    let parsed = FontProgram::parse_with_diagnostics(bytes.to_owned(), format, limits, &mut notes);
    diagnostics(notes.as_slice());
    let program = parsed?;
    if program.data() != bytes {
        return Err(Error::invalid_argument("probe font source changed"));
    }
    println!("SOURCE_BYTES_VERIFIED");
    println!("MATRIX {:?}", program.font_matrix());
    println!("GLYPHS {}", program.glyph_count());
    for index in 0..program.glyph_count() {
        let glyph = GlyphId(index);
        println!(
            "GLYPH {index} {:?} {:?} {:?}",
            program.glyph_name(glyph),
            program.advance(glyph)?,
            program.outline(glyph)?.segments()
        );
    }
    Ok(())
}
fn render(bytes: &[u8], output: &str, dpi: f64, recovery: Recovery, limits: Limits) -> Result<()> {
    let document = Document::open(
        bytes.to_owned(),
        &OpenOptions {
            recovery,
            limits,
            ..OpenOptions::default()
        },
    )?;
    let result = (|| {
        let page = document.page(0)?;
        let options = RenderOptions::from_dpi(dpi);
        let white = pagura::render_page(&document, page, &options)?;
        let transparent = pagura::render_page(
            &document,
            page,
            &RenderOptions {
                background: Rgba8::TRANSPARENT,
                ..options
            },
        )?;
        let text = pagura::text_page(&document, page)?;
        if document.original_bytes() != bytes {
            return Err(Error::invalid_argument("probe document source changed"));
        }
        println!("SOURCE_BYTES_VERIFIED");
        println!("DIMENSIONS {} {}", white.width(), white.height());
        for glyph in text.glyphs() {
            println!(
                "TEXT {:?} {:?} {:?}",
                glyph.unicode.as_deref(),
                glyph.origin,
                glyph.advance_end
            );
        }
        let mut ppm = format!("P6\n{} {}\n255\n", white.width(), white.height()).into_bytes();
        ppm.extend(white.to_rgb_over(Rgba8::WHITE));
        // No output is created until all public operations and source checks succeeded.
        write(Path::new(&format!("{output}.ppm")), &ppm)?;
        write(Path::new(&format!("{output}.white.rgba")), white.data())?;
        write(
            Path::new(&format!("{output}.transparent.rgba")),
            transparent.data(),
        )?;
        Ok(())
    })();
    diagnostics(&document.diagnostics());
    result
}
fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [operation, input, output, dpi, mode, format, budget] = args.as_slice() else {
        return Err(Error::invalid_argument(
            "expected operation input output DPI mode format budget",
        ));
    };
    let recovery = match mode.as_str() {
        "strict" => Recovery::Strict,
        "repair" => Recovery::Repair,
        _ => return Err(Error::invalid_argument("unknown probe recovery")),
    };
    let format = match format.as_str() {
        "truetype" => FontFormat::TrueType,
        "opentype" => FontFormat::OpenType,
        _ => return Err(Error::invalid_argument("unknown probe format")),
    };
    let dpi = dpi
        .parse::<f64>()
        .map_err(|_| Error::invalid_argument("invalid probe DPI"))?;
    let limits = limits(budget)?;
    let bytes = read(input)?;
    println!(
        "COMPILED_SOURCE {}",
        option_env!("PAGURA_SOURCE_REVISION").unwrap_or("unrecorded")
    );
    match operation.as_str() {
        "font" => font(&bytes, format, recovery, &limits),
        "render" => render(&bytes, output, dpi, recovery, limits),
        _ => Err(Error::invalid_argument("unknown probe operation")),
    }
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:?}");
            ExitCode::FAILURE
        }
    }
}
