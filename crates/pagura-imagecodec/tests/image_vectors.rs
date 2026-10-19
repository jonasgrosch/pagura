#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "the vector helpers fail the test on unexpected fixture data"
)]
//! The independent JPEG and fax vectors of `corpus/generated/image`,
//! decoded through the public codec API. Each fixture kind has its own test,
//! and every failing case is listed rather than only the first.
use pagura_core::{Diagnostics, ErrorKind, Limits, Recovery};
use pagura_imagecodec::{ccitt, dct};
use pagura_testkit::{fixture, fixture_names};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

type Params = BTreeMap<String, String>;

fn params(name: &str) -> Params {
    String::from_utf8(fixture("image", &format!("{name}.params")))
        .unwrap()
        .lines()
        .map(|line| {
            let (key, value) = line.split_once('=').unwrap();
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

/// Runs `check` on every case of `kind`, then fails with the names of the
/// cases that failed. Returns the number of cases.
fn each_case(kind: &str, check: fn(&str, &Params)) -> usize {
    let (mut count, mut failures) = (0, Vec::new());
    for name in fixture_names("image", "params") {
        let params = params(&name);
        if params["kind"] != kind {
            continue;
        }
        count += 1;
        if catch_unwind(AssertUnwindSafe(|| check(&name, &params))).is_err() {
            failures.push(name);
        }
    }
    assert_eq!(failures, Vec::<String>::new(), "failing {kind} vectors");
    count
}

fn number(params: &Params, key: &str, default: i64) -> i64 {
    params.get(key).map_or(default, |v| v.parse().unwrap())
}

/// Compares decoded samples with the expected ones within the case's
/// tolerance, or checks the expected failure.
fn check_outcome(name: &str, params: &Params, result: pagura_core::Result<Vec<u8>>) {
    let expected = fixture("image", &format!("{name}.out"));
    match params["outcome"].as_str() {
        "valid" => {
            let actual = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(actual.len(), expected.len(), "{name}");
            let tolerance = u8::try_from(number(params, "tolerance", 0)).unwrap();
            for (i, (&a, &b)) in actual.iter().zip(&expected).enumerate() {
                assert!(a.abs_diff(b) <= tolerance, "{name} byte {i}: {a} != {b}");
            }
        }
        "unsupported" => assert_eq!(
            result.unwrap_err().feature().unwrap().as_str(),
            params["feature"],
            "{name}"
        ),
        outcome => {
            let error = result.unwrap_err();
            let kind = if outcome == "limit" {
                ErrorKind::Limit
            } else {
                ErrorKind::Syntax
            };
            assert_eq!(error.kind, kind, "{name}: {error}");
        }
    }
}

#[test]
fn jpeg_vectors() {
    let count = each_case("dct", |name, params| {
        let encoded = fixture("image", &format!("{name}.in"));
        let transform = match number(params, "transform", -1) {
            -1 => None,
            value => Some(value),
        };
        let result =
            dct::decode(&encoded, transform, &Limits::default()).map(|image| image.samples);
        check_outcome(name, params, result);
    });
    assert!(count >= 80, "{count} JPEG vectors");
}

#[test]
fn fax_vectors() {
    let count = each_case("ccitt", |name, params| {
        let encoded = fixture("image", &format!("{name}.in"));
        let fax = ccitt::CcittParams {
            k: number(params, "k", 0),
            columns: u32::try_from(number(params, "width", 13)).unwrap(),
            rows: u32::try_from(number(params, "height", 6))
                .ok()
                .filter(|&rows| rows != 0),
            end_of_block: number(params, "eob", 1) != 0,
            end_of_line: number(params, "eol", 0) != 0,
            encoded_byte_align: number(params, "align", 0) != 0,
            black_is_1: number(params, "black", 0) != 0,
            damaged_rows_before_error: u32::try_from(number(params, "damaged", 0)).unwrap(),
        };
        let limits = Limits::default();
        if params["outcome"] == "repair" {
            let error = ccitt::decode(&encoded, &fax, &limits).unwrap_err();
            assert_eq!(
                error.repair,
                Some(pagura_imagecodec::repairs::CCITT_DAMAGED_ROW),
                "{name}"
            );
            let mut diagnostics = Diagnostics::new(Recovery::Repair);
            let decoded =
                ccitt::decode_with_diagnostics(&encoded, &fax, &limits, &mut diagnostics).unwrap();
            assert_eq!(decoded, fixture("image", &format!("{name}.out")), "{name}");
            assert_eq!(
                diagnostics.as_slice().len(),
                usize::try_from(fax.damaged_rows_before_error).unwrap(),
                "{name}"
            );
            return;
        }
        check_outcome(name, params, ccitt::decode(&encoded, &fax, &limits));
    });
    assert!(count >= 200, "{count} fax vectors");
}

#[test]
fn image_budgets_precede_allocations() {
    let limits = Limits {
        pixels: 1,
        decoded_bytes: 1024,
        ..Limits::default()
    };
    assert_eq!(
        dct::decode(&fixture("image", "jpeg-baseline-gray.in"), None, &limits)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let params = ccitt::CcittParams {
        columns: 13,
        rows: Some(6),
        ..ccitt::CcittParams::default()
    };
    assert_eq!(
        ccitt::decode(&[], &params, &limits).unwrap_err().kind,
        ErrorKind::Limit
    );
}

#[test]
fn truncated_and_mutated_codecs_never_invent_samples() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 65536,
        decode_bytes: 1 << 20,
        decode_work: 100_000,
        ..Limits::default()
    };
    for name in [
        "jpeg-baseline-gray",
        "jpeg-progressive-band-refinement",
        "jpeg-interleaved-progressive",
    ] {
        let data = fixture("image", &format!("{name}.in"));
        for end in 0..data.len() {
            assert!(
                dct::decode(&data[..end], None, &limits).is_err(),
                "{name} truncation {end}"
            );
        }
        for index in 0..data.len() {
            let mut corrupt = data.clone();
            corrupt[index] ^= 0xa5;
            let _ = dct::decode(&corrupt, None, &limits);
        }
    }
    let data = fixture("image", "fax-complex--1.in");
    let params = ccitt::CcittParams {
        k: -1,
        columns: 73,
        ..ccitt::CcittParams::default()
    };
    for end in 0..data.len() {
        assert!(
            ccitt::decode(&data[..end], &params, &limits).is_err(),
            "fax truncation {end}"
        );
    }
}

#[test]
fn deferred_jpeg_height_is_an_explicit_capability_gap() {
    let mut encoded = fixture("image", "jpeg-baseline-gray.in");
    let frame = encoded.windows(2).position(|w| w == [255, 192]).unwrap();
    encoded[frame + 5..frame + 7].fill(0);
    let error = dct::decode(&encoded, None, &Limits::default()).unwrap_err();
    assert_eq!(
        error.feature(),
        Some(pagura_imagecodec::features::JPEG_PROCESS)
    );
}
