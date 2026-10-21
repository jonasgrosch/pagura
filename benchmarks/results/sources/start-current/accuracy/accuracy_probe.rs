#![forbid(unsafe_code)]
//! Untimed public-API accuracy adapter shared verbatim by both source revisions.

use pagura_core::Recovery;
use pagura_document::{Document, OpenOptions};
use pagura_raster::Rgba8;
use pagura_render::{Accuracy, RenderOptions, Tile, compile_page};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 13 {
        return Err("usage: accuracy_probe INPUT exact|competitive full|tile OUTPUT R G B A X Y W H".into());
    }
    let accuracy = match args[2].as_str() {
        "exact" => Accuracy::Exact,
        "competitive" => Accuracy::Competitive,
        _ => return Err("unknown accuracy".into()),
    };
    let background = Rgba8 {
        r: args[5].parse()?,
        g: args[6].parse()?,
        b: args[7].parse()?,
        a: args[8].parse()?,
    };
    let tile = Tile {
        x: args[9].parse()?,
        y: args[10].parse()?,
        width: args[11].parse()?,
        height: args[12].parse()?,
    };
    let options = RenderOptions {
        accuracy,
        scale: 1.0,
        background,
        ..RenderOptions::default()
    };
    let document = Document::open(
        std::fs::read(&args[1])?,
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )?;
    let list = compile_page(&document, document.page(0)?)?;
    let image = match args[3].as_str() {
        "full" if tile == (Tile { x: 0, y: 0, width: 0, height: 0 }) => list.render(&options)?,
        "tile" => list.render_tile(&options, tile)?,
        _ => return Err("unknown operation or nonzero full-render sentinel".into()),
    };
    let diagnostics = document.diagnostics().len();
    let suppressed = document.suppressed_diagnostics();
    let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&args[4])?;
    writeln!(output, "RGBA8 {} {} premultiplied", image.width(), image.height())?;
    output.write_all(image.data())?;
    println!(
        "{{\"schema\":1,\"accuracy\":\"{}\",\"operation\":\"{}\",\"page\":0,\"dpi\":72,\"background\":[{},{},{},{}],\"tile\":[{},{},{},{}],\"width\":{},\"height\":{},\"diagnostics\":{},\"suppressed_diagnostics\":{}}}",
        args[2], args[3], background.r, background.g, background.b, background.a,
        tile.x, tile.y, tile.width, tile.height, image.width(), image.height(), diagnostics, suppressed,
    );
    if diagnostics != 0 || suppressed != 0 {
        return Err("unexpected document diagnostics".into());
    }
    Ok(())
}
