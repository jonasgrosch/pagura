//! Encoder fixtures, independent Python decoding, and resource boundaries.
use crate::{
    Filter, Predictor,
    flate::{self, Level},
    png::{self, ColorType, Options},
};
use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};
use pagura_testkit::{fixture, fixture_path};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn names() -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(fixture_path("codec", ""))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "params"))
        .map(|p| p.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

fn frame(batch: &mut Vec<u8>, kind: u8, encoded: &[u8], plain: &[u8], dimensions: [u32; 3]) {
    batch.push(kind);
    for value in [
        u32::try_from(encoded.len()).unwrap(),
        u32::try_from(plain.len()).unwrap(),
    ]
    .into_iter()
    .chain(dimensions)
    {
        batch.extend_from_slice(&value.to_be_bytes());
    }
    batch.extend_from_slice(encoded);
    batch.extend_from_slice(plain);
}

fn verify(batch: &[u8]) {
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools");
    let mut process = Command::new("python3")
        .args(["-m", "fixturegen.codec", "verify"])
        .env("PYTHONPATH", tools)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    process.stdin.take().unwrap().write_all(batch).unwrap();
    let result = process.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
}

#[test]
fn every_codec_vector_round_trips_and_python_checks_the_compressed_bytes() {
    let mut batch = Vec::new();
    let mut types = [false; 3];
    for name in names() {
        let params = crate::vectors::options(&name);
        for suffix in ["in", "out"] {
            let plain = fixture("codec", &format!("{name}.{suffix}"));
            for level in [Level::Fast, Level::Default] {
                let encoded = flate::compress(&plain, level, &Limits::default()).unwrap();
                let decoded = Filter::Flate {
                    predictor: Predictor::default(),
                }
                .decode(
                    &encoded,
                    plain.len(),
                    &mut Diagnostics::new(Recovery::Strict),
                )
                .unwrap();
                assert_eq!(decoded, plain, "{name}.{suffix} {level:?}");
                frame(&mut batch, 0, &encoded, &plain, [0; 3]);
                if params["filter"] == "encode-flate" {
                    assert_eq!(
                        encoded,
                        flate::compress(&plain, level, &Limits::default()).unwrap()
                    );
                    if let Some(expected) = params.get("block_type") {
                        let actual = (encoded[2] >> 1) & 3;
                        assert_eq!(actual, expected.parse::<u8>().unwrap(), "{name} {level:?}");
                        types[usize::from(actual)] = true;
                    }
                }
            }
        }
    }
    assert!(types.into_iter().all(|seen| seen));
    verify(&batch);
}

#[test]
fn independent_png_decode_checks_pixels_crc_chunks_filters_and_straight_alpha() {
    let mut batch = Vec::new();
    let mut count = 0;
    for name in names() {
        let params = crate::vectors::options(&name);
        if params["filter"] != "encode-png" {
            continue;
        }
        count += 1;
        let width = params["width"].parse().unwrap();
        let height = params["height"].parse().unwrap();
        let channels = params["channels"].parse().unwrap();
        let color_type = match channels {
            1 => ColorType::Gray,
            3 => ColorType::Rgb,
            4 => ColorType::Rgba,
            _ => panic!("fixture channels"),
        };
        let pixels = fixture("codec", &format!("{name}.in"));
        let expected = fixture("codec", &format!("{name}.out"));
        for level in [Level::Fast, Level::Default] {
            let options = Options {
                color_type,
                level,
                ..Options::default()
            };
            let encoded = png::encode(&pixels, width, height, &options).unwrap();
            assert_eq!(
                encoded,
                png::encode(&pixels, width, height, &options).unwrap()
            );
            frame(
                &mut batch,
                1,
                &encoded,
                &expected,
                [width, height, channels],
            );
            let exact = Options {
                limits: Limits {
                    stream_bytes: encoded.len(),
                    ..options.limits
                },
                ..options
            };
            assert_eq!(
                encoded,
                png::encode(&pixels, width, height, &exact).unwrap()
            );
            let short = Options {
                limits: Limits {
                    stream_bytes: encoded.len() - 1,
                    ..options.limits
                },
                ..options
            };
            assert_eq!(
                png::encode(&pixels, width, height, &short)
                    .unwrap_err()
                    .kind,
                ErrorKind::Limit
            );
        }
    }
    assert!(count >= 20);
    verify(&batch);
}

#[test]
fn encoding_limits_dimensions_and_pixel_counts_are_explicit() {
    for level in [Level::Fast, Level::Default] {
        for plain in [b"".as_slice(), b"ABCABCABC"] {
            let encoded = flate::compress(plain, level, &Limits::default()).unwrap();
            let exact = Limits {
                decoded_bytes: plain.len(),
                stream_bytes: encoded.len(),
                ..Limits::default()
            };
            assert_eq!(flate::compress(plain, level, &exact).unwrap(), encoded);
            let short = Limits {
                stream_bytes: encoded.len() - 1,
                ..exact
            };
            let error = flate::compress(plain, level, &short).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            assert!(matches!(error.position, Position::Buffer(_)));
        }
        assert_eq!(
            flate::compress(
                b"x",
                level,
                &Limits {
                    decoded_bytes: 0,
                    ..Limits::default()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit
        );
    }
    let options = Options::default();
    for (pixels, w, h) in [
        (b"".as_slice(), 0, 1),
        (b"", 1, 0),
        (b"", 0x8000_0000, 1),
        (b"abc", 1, 1),
        (b"abcde", 1, 1),
    ] {
        assert_eq!(
            png::encode(pixels, w, h, &options).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
    }
    let pixels = [255, 128, 64, 0];
    for limits in [
        Limits {
            pixels: 0,
            ..options.limits
        },
        Limits {
            decoded_bytes: 3,
            ..options.limits
        },
        Limits {
            stream_bytes: 0,
            ..options.limits
        },
    ] {
        assert_eq!(
            png::encode(&pixels, 1, 1, &Options { limits, ..options })
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
    assert_eq!(
        png::encode(&pixels, 0x7fff_ffff, 0x7fff_ffff, &options)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}
