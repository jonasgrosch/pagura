#![forbid(unsafe_code)]
//! Exploratory release-mode compression measurement, not a release benchmark.
//!
//! Build with `cargo build --release -p pagura-codec --example encode_measure`.
//! Supply repetitions and samples as positive integers (default 1 and 7).
//! Stdin is a sequence of big-endian u32 lengths followed by input bytes. Inputs
//! are read once; timings include encoding, allocation and dropping each result,
//! and exclude loading, verification and stdout. Three batches warm each level.
//! Stderr is CSV: level,sample,repetitions,input_bytes,output_bytes,elapsed_ns.
//! Stdout returns one length-prefixed zlib stream per input, Fast then Default,
//! for independent verification. No corpus data is written to disk.
use pagura_codec::{
    Filter, Predictor,
    flate::{self, Level},
};
use pagura_core::{Diagnostics, Limits, Recovery};
use std::{
    error::Error,
    hint::black_box,
    io::{self, Read, Write},
    time::Instant,
};

fn read_inputs() -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(256 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 * 1024 {
        return Err("measurement input exceeds 256 MiB".into());
    }
    let mut inputs = Vec::new();
    let mut cursor = bytes.as_slice();
    while !cursor.is_empty() {
        if inputs.len() >= 4096 {
            return Err("measurement input exceeds 4096 streams".into());
        }
        let length = u32::from_be_bytes(cursor.get(..4).ok_or("short input frame")?.try_into()?);
        cursor = &cursor[4..];
        let length = usize::try_from(length)?;
        inputs.push(cursor.get(..length).ok_or("short input data")?.to_vec());
        cursor = &cursor[length..];
    }
    Ok(inputs)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let repeats: u32 = arguments.next().map_or(Ok(1), |v| v.parse())?;
    let samples: u32 = arguments.next().map_or(Ok(7), |v| v.parse())?;
    if arguments.next().is_some() || repeats == 0 || samples == 0 {
        return Err("expected positive repetitions and samples".into());
    }
    let inputs = read_inputs()?;
    let input_bytes: usize = inputs.iter().map(Vec::len).sum();
    let limits = Limits::default();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    eprintln!("level,sample,repetitions,input_bytes,output_bytes,elapsed_ns");
    for level in [Level::Fast, Level::Default] {
        let mut output_bytes = 0;
        for input in &inputs {
            let encoded = flate::compress(input, level, &limits)?;
            let decoded = Filter::Flate {
                predictor: Predictor::default(),
            }
            .decode(
                &encoded,
                input.len(),
                &mut Diagnostics::new(Recovery::Strict),
            )?;
            if decoded != *input {
                return Err("compression round trip mismatch".into());
            }
            output_bytes += encoded.len();
            stdout.write_all(&u32::try_from(encoded.len())?.to_be_bytes())?;
            stdout.write_all(&encoded)?;
        }
        let batch = || -> pagura_core::Result<()> {
            for input in &inputs {
                black_box(flate::compress(black_box(input), level, &limits)?);
            }
            Ok(())
        };
        for _ in 0..3 {
            batch()?;
        }
        for sample in 0..samples {
            let start = Instant::now();
            for _ in 0..repeats {
                batch()?;
            }
            let elapsed = start.elapsed().as_nanos();
            eprintln!("{level:?},{sample},{repeats},{input_bytes},{output_bytes},{elapsed}");
        }
    }
    stdout.flush()?;
    Ok(())
}
