#![forbid(unsafe_code)]
//! Writes the zlib and PNG encodings of every codec fixture to stdout for
//! `tools/verify_encoders.py`, which decodes them with the independent
//! decoders of `tools/fixturegen/codec.py`. Cargo tests never start an
//! interpreter (CONTRIBUTING.md, Conventions 2), so this program is how the
//! encoders reach that oracle.
//!
//! Every fixture's input and expected output are compressed at both levels,
//! and every `encode-png` fixture is encoded as PNG at both levels. Each
//! frame is big-endian: the kind (0 zlib, 1 PNG), the encoded length, the
//! expected length, the width, height and channels (zero for zlib), then
//! the encoded and the expected bytes.
use pagura_codec::flate::{self, Level};
use pagura_codec::png::{self, ColorType, Options};
use pagura_core::Limits;
use pagura_testkit::{fixture, fixture_names, fixture_text};
use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, Write};

fn frame(
    out: &mut impl Write,
    kind: u8,
    encoded: &[u8],
    expected: &[u8],
    dimensions: [u32; 3],
) -> Result<(), Box<dyn Error>> {
    out.write_all(&[kind])?;
    for value in [
        u32::try_from(encoded.len())?,
        u32::try_from(expected.len())?,
    ]
    .into_iter()
    .chain(dimensions)
    {
        out.write_all(&value.to_be_bytes())?;
    }
    out.write_all(encoded)?;
    out.write_all(expected)?;
    Ok(())
}

fn png_vector(
    out: &mut impl Write,
    name: &str,
    params: &BTreeMap<&str, &str>,
) -> Result<(), Box<dyn Error>> {
    let number = |key: &str| -> Result<u32, Box<dyn Error>> {
        Ok(params.get(key).ok_or("missing PNG parameter")?.parse()?)
    };
    let (width, height, channels) = (number("width")?, number("height")?, number("channels")?);
    let color_type = match channels {
        1 => ColorType::Gray,
        3 => ColorType::Rgb,
        4 => ColorType::Rgba,
        _ => return Err("unsupported PNG fixture channels".into()),
    };
    let pixels = fixture("codec", &format!("{name}.in"));
    let expected = fixture("codec", &format!("{name}.out"));
    for level in [Level::Fast, Level::Default] {
        let options = Options::default()
            .with_color_type(color_type)
            .with_level(level);
        let encoded = png::encode(&pixels, width, height, &options)?;
        frame(out, 1, &encoded, &expected, [width, height, channels])?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    for name in fixture_names("codec", "params") {
        let text = fixture_text("codec", &format!("{name}.params"));
        let params: BTreeMap<&str, &str> = text
            .lines()
            .map(|line| line.split_once('=').ok_or("malformed .params line"))
            .collect::<Result<_, _>>()?;
        if params.get("filter") == Some(&"encode-png") {
            png_vector(&mut out, &name, &params)?;
        }
        for suffix in ["in", "out"] {
            let plain = fixture("codec", &format!("{name}.{suffix}"));
            for level in [Level::Fast, Level::Default] {
                let encoded = flate::compress(&plain, level, &Limits::default())?;
                frame(&mut out, 0, &encoded, &plain, [0; 3])?;
            }
        }
    }
    out.flush()?;
    Ok(())
}
