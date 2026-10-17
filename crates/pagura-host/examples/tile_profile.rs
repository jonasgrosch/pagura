#![forbid(unsafe_code)]
//! Separates compilation and tile painting costs for an exploratory profile.

use pagura::{Document, OpenOptions, RenderOptions, Tile, compile_page};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("expected a PDF path")?;
    let document = Document::open(std::fs::read(path)?, &OpenOptions::default())?;
    let page = document.page(0)?;
    let start = Instant::now();
    for _ in 0..64 {
        std::hint::black_box(compile_page(&document, page)?);
    }
    let compile_seconds = start.elapsed().as_secs_f64();
    let list = compile_page(&document, page)?;
    let start = Instant::now();
    for y in (0..512).step_by(64) {
        for x in (0..512).step_by(64) {
            std::hint::black_box(list.render_tile(
                &RenderOptions::default(),
                Tile {
                    x,
                    y,
                    width: 64,
                    height: 64,
                },
            )?);
        }
    }
    println!(
        "{{\"compilations\":64,\"compile_seconds\":{compile_seconds},\"paint_seconds\":{}}}",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}
