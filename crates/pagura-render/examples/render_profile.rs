#![forbid(unsafe_code)]
//! Exploratory, single-process render timings; not a competitor benchmark.
//!
//! `render_profile INPUT PAGE DPI COUNT [OUTPUT.rgba]` records the page once
//! and prints one line per timed render: the sample, its seconds and a hash
//! of the pixels.

use pagura_document::{Document, OpenOptions};
use pagura_render::{RenderContext, RenderOptions};
use std::sync::Arc;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let path = args.get(1).ok_or("expected PDF path")?;
    let page: usize = args.get(2).ok_or("expected page index")?.parse()?;
    let dpi: f64 = args.get(3).ok_or("expected dpi")?.parse()?;
    let count: usize = args.get(4).ok_or("expected sample count")?.parse()?;
    let document = Document::open(std::fs::read(path)?, &OpenOptions::default())?;
    let context = RenderContext::new(Arc::new(document));
    let scene = context.scene(context.document().page(page)?)?;
    let options = RenderOptions::from_dpi(dpi);
    for sample in 0..=count {
        let start = Instant::now();
        let image = context.render_scene(&scene, &options, None)?;
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
