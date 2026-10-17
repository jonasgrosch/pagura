#![forbid(unsafe_code)]
//! Local release-mode Flate allocation/copy measurement, not a release claim.
//!
//! Usage: `flate_measure <fixture-prefix> <repetitions> <samples>`.
//! The prefix identifies independently generated `.in` and `.out` files.
//! Verification happens outside the timed loops; allocations and drops are timed.

use pagura_codec::{Filter, Predictor};
use pagura_core::{Diagnostics, Recovery};
use std::{error::Error, hint::black_box, path::Path, time::Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let prefix = arguments.next().ok_or("missing fixture prefix")?;
    let repeats: u32 = arguments
        .next()
        .ok_or("missing repetition count")?
        .parse()?;
    let samples: u32 = arguments.next().ok_or("missing sample count")?.parse()?;
    if arguments.next().is_some() || repeats == 0 || samples == 0 {
        return Err("expected: fixture-prefix positive-repetitions positive-samples".into());
    }
    let input = std::fs::read(Path::new(&prefix).with_extension("in"))?;
    let expected = std::fs::read(Path::new(&prefix).with_extension("out"))?;
    let filter = Filter::Flate {
        predictor: Predictor::default(),
    };
    let decode = || {
        filter.decode(
            black_box(&input),
            expected.len(),
            &mut Diagnostics::new(Recovery::Strict),
        )
    };
    let verify = || -> Result<(), Box<dyn Error>> {
        if decode()? != expected {
            return Err("independent output mismatch".into());
        }
        Ok(())
    };
    verify()?;
    for _ in 0..100 {
        black_box(decode()?);
    }
    println!("sample,repetitions,decoded_bytes,elapsed_ns");
    for sample in 0..samples {
        let start = Instant::now();
        for _ in 0..repeats {
            black_box(decode()?);
        }
        let elapsed = start.elapsed();
        verify()?;
        println!(
            "{sample},{repeats},{},{elapsed}",
            expected.len(),
            elapsed = elapsed.as_nanos()
        );
    }
    Ok(())
}
