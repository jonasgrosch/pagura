#![forbid(unsafe_code)]
//! Exploratory, single-process render timings; not a competitor benchmark.

use pagura_document::{Document, OpenOptions};
use pagura_render::{RenderOptions, compile_page};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let path = args.get(1).ok_or("expected PDF path")?;
    let page: usize = args.get(2).ok_or("expected page index")?.parse()?;
    let dpi: f64 = args.get(3).ok_or("expected dpi")?.parse()?;
    let count: usize = args.get(4).ok_or("expected sample count")?.parse()?;
    let document = Document::open(std::fs::read(path)?, &OpenOptions::default())?;
    let list = compile_page(&document, document.page(page)?)?;
    let options = RenderOptions::from_dpi(dpi);
    for sample in 0..=count {
        let start = Instant::now();
        let image = list.render(&options)?;
        let elapsed = start.elapsed().as_secs_f64();
        // Hash and disk emission happen outside the measured render stage.
        let digest = image
            .data()
            .iter()
            .fold(0xcbf2_9ce4_8422_2325_u64, |n, &b| {
                (n ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
            });
        if sample > 0 {
            println!("{sample} {elapsed:.9} {digest:016x}");
        }
        if let Some(output) = args.get(5) {
            std::fs::write(output, image.data())?;
        }
        std::hint::black_box(image);
    }
    Ok(())
}
