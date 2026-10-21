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
/// password and recovery mode, and prints one JSON line.
pub(crate) fn run(
    source: &Source,
    operation: Bench,
    count: u32,
    options: &RenderOptions,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let bytes = source.load()?;
    let open = || source.open_bytes(Arc::clone(&bytes), source.recovery);
    let warm = pagura::render_context(Arc::new(open()?));
    let started = Instant::now();
    for _ in 0..count {
        match operation {
            Bench::OpenPages => {
                black_box(open()?.page_count()?);
            }
            Bench::RenderCold => {
                let document = open()?;
                black_box(pagura::render_page(&document, document.page(0)?, options)?);
            }
            Bench::RenderWarm => {
                black_box(warm.render_page(warm.document().page(0)?, options)?);
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
