//! Encoder round trips, determinism and resource boundaries. The independent
//! decoders of `tools/fixturegen/codec.py` check the encoded bytes through
//! `tools/verify_encoders.py`, because cargo tests start no interpreter.
use super::options;
use pagura_codec::{
    Filter, Predictor,
    flate::{self, Level},
    png::{self, ColorType, Options},
};
use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};
use pagura_testkit::{fixture, fixture_names};

#[test]
fn every_codec_vector_round_trips_through_compress_and_flate_decode() {
    let mut types = [false; 3];
    for name in fixture_names("codec", "params") {
        let params = options(&name);
        for suffix in ["in", "out"] {
            let plain = fixture("codec", &format!("{name}.{suffix}"));
            for level in [Level::Fast, Level::Default] {
                let encoded = flate::compress(&plain, level, &Limits::default()).unwrap();
                let decoded = Filter::Flate {
                    predictor: Predictor::default(),
                }
                .decode(
                    &encoded,
                    &Limits {
                        decoded_bytes: plain.len(),
                        ..Limits::default()
                    },
                    &mut Diagnostics::new(Recovery::Strict),
                )
                .unwrap();
                assert_eq!(decoded, plain, "{name}.{suffix} {level:?}");
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
}

#[test]
fn png_encoding_is_deterministic_and_fits_its_exact_budget() {
    let mut count = 0;
    for name in fixture_names("codec", "params") {
        let params = options(&name);
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
        for level in [Level::Fast, Level::Default] {
            let options = Options::default()
                .with_color_type(color_type)
                .with_level(level);
            let encoded = png::encode(&pixels, width, height, &options).unwrap();
            assert_eq!(
                encoded,
                png::encode(&pixels, width, height, &options).unwrap()
            );
            let exact = options.with_limits(Limits {
                stream_bytes: encoded.len(),
                ..options.limits
            });
            assert_eq!(
                encoded,
                png::encode(&pixels, width, height, &exact).unwrap()
            );
            let short = options.with_limits(Limits {
                stream_bytes: encoded.len() - 1,
                ..options.limits
            });
            assert_eq!(
                png::encode(&pixels, width, height, &short)
                    .unwrap_err()
                    .kind,
                ErrorKind::Limit
            );
        }
    }
    assert!(count >= 20);
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
            assert_eq!(
                (error.kind, error.position),
                (ErrorKind::Limit, Position::Unknown)
            );
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
            png::encode(&pixels, 1, 1, &options.with_limits(limits))
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
