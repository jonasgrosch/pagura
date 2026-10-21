//! `microbench`: Pagura-only stage timings for profiling; never a
//! cross-engine claim (ADR 0002).

use crate::args::{Bench, Source};
use crate::failure::Failure;
use crate::json;
use pagura::RenderOptions;
use std::hint::black_box;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

/// Times `count` repetitions of `operation` on `source`, opened with its
/// password and recovery mode, and prints one JSON line. The document is
/// opened once outside the timing, except that `open-pages` opens it again
/// every time. `render-cold` records and renders page 0 with a new context
/// every time, so nothing is reused; `record-warm` records it with one
/// context, which reuses fonts, glyphs and images but keeps no scene;
/// `render-warm` renders it from one context, which records the page once
/// and keeps its scene.
pub(crate) fn run(
    source: &Source,
    operation: Bench,
    count: u32,
    options: &RenderOptions,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let bytes = source.load()?;
    let open = || source.open_bytes(Arc::clone(&bytes), source.recovery);
    let document = Arc::new(open()?);
    let warm = pagura::render_context(Arc::clone(&document));
    let page = document.page(0)?;
    let started = Instant::now();
    for _ in 0..count {
        match operation {
            Bench::OpenPages => {
                black_box(open()?.page_count()?);
            }
            Bench::RenderCold => {
                let cold = pagura::render_context(Arc::clone(&document));
                black_box(cold.render_page(page, options)?);
            }
            Bench::RecordWarm => {
                black_box(warm.record(page)?);
            }
            Bench::RenderWarm => {
                black_box(warm.render_page(page, options)?);
            }
        }
    }
    writeln!(
        out,
        "{{\"schema\":{},\"operation\":\"{}\",\"iterations\":{count},\"elapsed_ns\":{}}}",
        json::MICROBENCH_SCHEMA,
        operation.name(),
        started.elapsed().as_nanos()
    )?;
    Ok(())
}
