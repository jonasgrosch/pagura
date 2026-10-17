//! Generated vector tests; all expected bytes originate outside Pagura.
use crate::{Filter, Predictor, PredictorKind};
use pagura_core::{Diagnostics, ErrorKind, Position, Recovery};
use pagura_testkit::{assert_bytes_eq, fixture, fixture_path};
use std::collections::BTreeMap;

fn options(name: &str) -> BTreeMap<String, String> {
    String::from_utf8(fixture("codec", &format!("{name}.params")))
        .unwrap()
        .lines()
        .map(|line| {
            let (key, value) = line.split_once('=').unwrap();
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

fn filter(params: &BTreeMap<String, String>) -> Filter {
    let value = |key: &str, default: u32| params.get(key).map_or(default, |s| s.parse().unwrap());
    let predictor = Predictor {
        kind: match value("predictor", 1) {
            1 => PredictorKind::None,
            2 => PredictorKind::Tiff,
            _ => PredictorKind::Png,
        },
        colors: u8::try_from(value("colors", 1)).unwrap(),
        bits_per_component: u8::try_from(value("bits", 8)).unwrap(),
        columns: value("columns", 1),
    };
    match params["filter"].as_str() {
        "FlateDecode" => Filter::Flate { predictor },
        "LZWDecode" => Filter::Lzw {
            predictor,
            early_change: value("early", 1) == 1,
        },
        "ASCII85Decode" => Filter::Ascii85,
        "ASCIIHexDecode" => Filter::AsciiHex,
        "RunLengthDecode" => Filter::RunLength,
        name => panic!("unknown fixture filter {name}"),
    }
}

#[test]
fn independent_vectors_exact_errors_repairs_and_caps() {
    let mut names: Vec<_> = std::fs::read_dir(fixture_path("codec", ""))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "params")
        })
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert!(names.len() > 200);
    for name in names {
        let params = options(&name);
        let filter = filter(&params);
        let encoded = fixture("codec", &format!("{name}.in"));
        let expected = fixture("codec", &format!("{name}.out"));
        let cap = params["cap"].parse().unwrap();
        let decode = |cap, diagnostics: &mut Diagnostics| {
            if params.get("raw").is_some_and(|s| s == "1") {
                crate::decode_raw_deflate(&encoded, cap, Predictor::default(), diagnostics)
            } else {
                filter.decode(&encoded, cap, diagnostics)
            }
        };
        let mut strict = Diagnostics::new(Recovery::Strict);
        let result = decode(cap, &mut strict);
        match params["outcome"].as_str() {
            "valid" if params["repair"] == "none" => {
                let actual = result.unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_bytes_eq(&actual, &expected, &name);
                assert!(strict.is_empty(), "{name}");
                // Each independent vector also establishes the exact cap boundary.
                if !actual.is_empty() {
                    let error = decode(actual.len() - 1, &mut Diagnostics::default()).unwrap_err();
                    assert_eq!(error.kind, ErrorKind::Limit, "{name}: {error}");
                }
            }
            "valid" => {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
                assert!(matches!(error.position, Position::Buffer(_)), "{name}");
                let mut diagnostics = Diagnostics::default();
                let actual =
                    decode(cap, &mut diagnostics).unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_bytes_eq(&actual, &expected, &name);
                if params["repair"] == "filter.raw-deflate" {
                    assert_eq!(error.position, Position::Buffer(0), "{name}");
                    assert_eq!(
                        diagnostics.as_slice()[0].message,
                        "FlateDecode data lacks the zlib wrapper; decoded as raw DEFLATE",
                        "{name}"
                    );
                    assert_eq!(
                        diagnostics.as_slice()[0].position,
                        Position::Buffer(0),
                        "{name}"
                    );
                }
                if !actual.is_empty() {
                    let error = decode(actual.len() - 1, &mut Diagnostics::default()).unwrap_err();
                    assert_eq!(error.kind, ErrorKind::Limit, "{name}: {error}");
                }
                assert_eq!(diagnostics.as_slice().len(), 1, "{name}");
                assert_eq!(
                    diagnostics.as_slice()[0].repair.as_str(),
                    params["repair"],
                    "{name}"
                );
            }
            outcome => {
                let error = result.unwrap_err();
                assert_eq!(
                    error.kind,
                    if outcome == "limit" {
                        ErrorKind::Limit
                    } else {
                        ErrorKind::Syntax
                    },
                    "{name}: {error}"
                );
                assert!(
                    matches!(error.position, Position::Buffer(_)),
                    "{name}: {error}"
                );
                assert_eq!(
                    decode(cap, &mut Diagnostics::default()).unwrap_err().kind,
                    error.kind,
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn malformed_small_inputs_never_panic_or_exceed_cap() {
    let filters = [
        Filter::AsciiHex,
        Filter::Ascii85,
        Filter::RunLength,
        Filter::Flate {
            predictor: Predictor::default(),
        },
        Filter::Lzw {
            predictor: Predictor::default(),
            early_change: false,
        },
        Filter::Lzw {
            predictor: Predictor::default(),
            early_change: true,
        },
    ];
    // Exhaustive two-byte input exercises invalid characters, headers, codes,
    // missing markers and truncated bit reads. No expected-output self oracle.
    for number in 0..=u16::MAX {
        let input = number.to_le_bytes();
        for filter in &filters {
            let result = filter.decode(&input, 8, &mut Diagnostics::default());
            if let Ok(bytes) = result {
                assert!(bytes.len() <= 8);
            }
        }
    }
}
