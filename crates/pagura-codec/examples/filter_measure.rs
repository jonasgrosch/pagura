#![forbid(unsafe_code)]
//! Local release-mode Flate and LZW decoding measurement, not a release
//! claim.
//!
//! Usage: `filter_measure <fixture-prefix> <repetitions> <samples>`.
//! The prefix names a generated codec vector (`corpus/generated/codec/…`):
//! `.in` is the encoded input, `.out` the independently expected output, and
//! `.params` selects the decoder: `filter=FlateDecode` (with `raw=1` for raw
//! DEFLATE) or `filter=LZWDecode` (with `early=`). Vectors with predictors are
//! not supported. Verification happens outside the timed loops; allocations
//! and drops are timed.

use pagura_codec::{Filter, Predictor, decode_raw_deflate};
use pagura_core::{Diagnostics, Dictionary, Limits, Name, Recovery, Value};
use std::{collections::BTreeMap, error::Error, hint::black_box, path::Path, time::Instant};

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
    let text = std::fs::read_to_string(Path::new(&prefix).with_extension("params"))?;
    let params: BTreeMap<&str, &str> = text
        .lines()
        .map(|line| line.split_once('=').ok_or("malformed .params line"))
        .collect::<Result<_, _>>()?;
    if ["predictor", "colors", "bits", "columns"]
        .iter()
        .any(|key| params.contains_key(key))
    {
        return Err("predictor vectors are not supported".into());
    }
    let raw = params.get("raw") == Some(&"1");
    let mut parameters = Dictionary::new();
    if let Some(early) = params.get("early") {
        parameters.insert(Name::new(b"EarlyChange"), Value::Integer(early.parse()?));
    }
    let filter = Filter::from_pdf(
        params.get("filter").ok_or("missing filter")?.as_bytes(),
        Some(&parameters),
    )?;
    if !matches!(filter, Filter::Flate { .. } | Filter::Lzw { .. }) {
        return Err("only FlateDecode and LZWDecode vectors are measured".into());
    }
    // Vectors whose expected result is a repair decode in Repair mode.
    let recovery = if params.get("repair") == Some(&"none") {
        Recovery::Strict
    } else {
        Recovery::Repair
    };
    let limits = Limits {
        decoded_bytes: expected.len(),
        ..Limits::default()
    };
    let decode = || {
        let mut diagnostics = Diagnostics::new(recovery);
        if raw {
            decode_raw_deflate(
                black_box(&input),
                &limits,
                Predictor::default(),
                &mut diagnostics,
            )
        } else {
            filter.decode(black_box(&input), &limits, &mut diagnostics)
        }
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
