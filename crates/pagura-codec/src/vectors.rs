//! Generated vector tests; all expected bytes originate outside Pagura.
use crate::{Filter, FilterChain, features};
use pagura_core::{Diagnostics, Dictionary, ErrorKind, Name, Position, Recovery, Value};
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

fn chain(params: &BTreeMap<String, String>) -> pagura_core::Result<FilterChain> {
    let value = |text: &str| match text {
        "null" => Value::Null,
        "true" => Value::Boolean(true),
        _ if text.starts_with('/') => Value::name(text.trim_start_matches('/').as_bytes()),
        _ => Value::Integer(text.parse().unwrap()),
    };
    let dictionary: Dictionary = [
        ("predictor", b"Predictor".as_slice()),
        ("colors", b"Colors"),
        ("bits", b"BitsPerComponent"),
        ("columns", b"Columns"),
        ("early", b"EarlyChange"),
        ("crypt_type", b"Type"),
        ("crypt_name", b"Name"),
    ]
    .into_iter()
    .filter_map(|(key, name)| params.get(key).map(|text| (Name::new(name), value(text))))
    .collect();
    let names: Vec<_> = params["filter"]
        .split(',')
        .map(|name| Value::name(name.as_bytes()))
        .collect();
    let entries = Value::Array(names);
    let decode_params = Value::Array(
        entries
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, _)| {
                if index == 0 {
                    Value::Dictionary(dictionary.clone())
                } else {
                    Value::Null
                }
            })
            .collect(),
    );
    FilterChain::from_entries(Some(&entries), Some(&decode_params))
}

fn error_kind(outcome: &str) -> ErrorKind {
    match outcome {
        "limit" => ErrorKind::Limit,
        "unsupported-dimensions" => ErrorKind::Unsupported(features::PREDICTOR_DIMENSIONS),
        _ => ErrorKind::Syntax,
    }
}

fn assert_error_position(error: &pagura_core::Error, parameter_error: bool, name: &str) {
    if parameter_error {
        assert_eq!(error.position, Position::Unknown, "{name}: {error}");
    } else {
        assert!(
            matches!(error.position, Position::Buffer(_)),
            "{name}: {error}"
        );
    }
}

fn assert_repair_rejection(error: &pagura_core::Error, tag: &str, name: &str) {
    assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
    assert!(matches!(error.position, Position::Buffer(_)), "{name}");
    assert_eq!(
        error.repair.map(pagura_core::Repair::as_str),
        Some(tag),
        "{name}"
    );
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
        let chain = chain(&params);
        let encoded = fixture("codec", &format!("{name}.in"));
        let expected = fixture("codec", &format!("{name}.out"));
        let cap = params["cap"].parse().unwrap();
        let decode = |cap, diagnostics: &mut Diagnostics| -> pagura_core::Result<Vec<u8>> {
            let chain = chain.as_ref().map_err(Clone::clone)?;
            if params.get("raw").is_some_and(|s| s == "1") {
                let Some(Filter::Flate { predictor }) = chain.filters().first() else {
                    panic!("invalid raw fixture");
                };
                crate::decode_raw_deflate(&encoded, cap, *predictor, diagnostics)
            } else if params.get("take_crypt").is_some_and(|s| s == "1") {
                let mut chain = chain.clone();
                assert!(matches!(chain.take_crypt(), Some(None)), "{name}");
                chain
                    .decode(&encoded, cap, diagnostics)
                    .map(std::borrow::Cow::into_owned)
            } else {
                chain
                    .decode(&encoded, cap, diagnostics)
                    .map(std::borrow::Cow::into_owned)
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
                assert_repair_rejection(&error, &params["repair"], &name);
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
                assert_eq!(error.kind, error_kind(outcome), "{name}: {error}");
                assert_error_position(
                    &error,
                    params.get("parameter_error").is_some_and(|s| s == "1"),
                    &name,
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
            predictor: crate::Predictor::default(),
        },
        Filter::Lzw {
            predictor: crate::Predictor::default(),
            early_change: false,
        },
        Filter::Lzw {
            predictor: crate::Predictor::default(),
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
