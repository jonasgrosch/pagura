#![forbid(unsafe_code)]
//! One exploratory sample using public retained-list APIs; no admission claim.

use pagura_document::{Document, OpenOptions};
use pagura_render::{Accuracy, DisplayList, Pixmap, RenderOptions, Tile, compile_page};
use std::io::Write;
use std::time::Instant;

fn render(
    list: &DisplayList,
    options: &RenderOptions,
    operation: &str,
) -> Result<Pixmap, Box<dyn std::error::Error>> {
    let (width, height) = list.device_size(options)?;
    if operation == "tile" {
        return Ok(list.render_tile(
            options,
            Tile {
                x: 0,
                y: 0,
                width: width.min(256),
                height: height.min(256),
            },
        )?);
    }
    if operation != "tiled" {
        return Ok(list.render(options)?);
    }
    // Full traversal includes assembly; every tile uses global coordinates.
    let mut output = Pixmap::new(
        width,
        height,
        options.background,
        pagura_core::Limits::default().pixels,
    )?;
    let stride = usize::try_from(width)?
        .checked_mul(4)
        .ok_or("row overflow")?;
    for y in (0..height).step_by(256) {
        for x in (0..width).step_by(256) {
            let tile = Tile {
                x,
                y,
                width: (width - x).min(256),
                height: (height - y).min(256),
            };
            let image = list.render_tile(options, tile)?;
            let count = usize::try_from(tile.width)?
                .checked_mul(4)
                .ok_or("tile row overflow")?;
            for row in 0..tile.height {
                let source = usize::try_from(row)?
                    .checked_mul(count)
                    .ok_or("tile offset overflow")?;
                let destination = usize::try_from(y + row)?
                    .checked_mul(stride)
                    .and_then(|value| value.checked_add(usize::try_from(x).ok()?.checked_mul(4)?))
                    .ok_or("output offset overflow")?;
                output.data_mut()[destination..destination + count]
                    .copy_from_slice(&image.data()[source..source + count]);
            }
        }
    }
    Ok(output)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 6 {
        return Err(
            "usage: scene_index_profile INPUT exact|competitive cold|full|tile|tiled OUTPUT DPI"
                .into(),
        );
    }
    let accuracy = match arguments[2].as_str() {
        "exact" => Accuracy::Exact,
        "competitive" => Accuracy::Competitive,
        _ => return Err("unknown accuracy".into()),
    };
    let operation = arguments[3].as_str();
    if !matches!(operation, "cold" | "full" | "tile" | "tiled") {
        return Err("unknown operation".into());
    }
    let dpi: f64 = arguments[5].parse()?;
    if !dpi.is_finite() || dpi <= 0.0 {
        return Err("invalid DPI".into());
    }
    let options = RenderOptions {
        accuracy,
        ..RenderOptions::from_dpi(dpi)
    };
    let bytes = std::fs::read(&arguments[1])?;
    // Storage reads and output emission are excluded; this is not cold I/O.
    let started = Instant::now();
    let document = Document::open(bytes, &OpenOptions::default())?;
    let open_ns = started.elapsed().as_nanos();
    let prepare = Instant::now();
    let list = compile_page(&document, document.page(0)?)?;
    let compile_ns = prepare.elapsed().as_nanos();
    let render_started = Instant::now();
    let image = render(&list, &options, operation)?;
    let render_ns = render_started.elapsed().as_nanos();
    let total_ns = started.elapsed().as_nanos();
    let metric_ns = if operation == "cold" {
        total_ns
    } else {
        render_ns
    };
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&arguments[4])?;
    writeln!(
        output,
        "RGBA8 {} {} premultiplied",
        image.width(),
        image.height()
    )?;
    output.write_all(image.data())?;
    println!(
        "{{\"schema\":1,\"operation\":\"{operation}\",\"accuracy\":\"{}\",\"dpi\":{dpi},\"open_ns\":{open_ns},\"compile_ns\":{compile_ns},\"render_ns\":{render_ns},\"total_ns\":{total_ns},\"metric_ns\":{metric_ns},\"width\":{},\"height\":{}}}",
        arguments[2],
        image.width(),
        image.height()
    );
    std::hint::black_box(image);
    Ok(())
}
