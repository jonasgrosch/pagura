//! Independent generator vectors and hostile-input boundaries.
use super::{Bitmap, decode};
use pagura_core::{ErrorKind, Limits, Position};
use pagura_testkit::{fixture, fixture_path};

#[test]
fn independent_bitmaps_and_hostile_vectors() {
    let mut count = 0;
    for entry in std::fs::read_dir(fixture_path("jbig2", "")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|s| s != "params") {
            continue;
        }
        count += 1;
        let name = path.file_stem().unwrap().to_str().unwrap();
        let params = std::fs::read_to_string(&path).unwrap();
        let params: std::collections::BTreeMap<_, _> = params
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        let data = fixture("jbig2", &format!("{name}.jb2"));
        let globals = fixture("jbig2", &format!("{name}.globals"));
        let limits = Limits::default();
        let result = decode(&data, Some(&globals), &limits);
        match params["outcome"] {
            "valid" => {
                let bitmap = result.unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(
                    bitmap,
                    Bitmap {
                        width: params["width"].parse().unwrap(),
                        height: params["height"].parse().unwrap(),
                        data: fixture("jbig2", &format!("{name}.out"))
                    },
                    "{name}"
                );
                let capped = Limits {
                    pixels: (bitmap.width * bitmap.height) as usize - 1,
                    ..limits
                };
                assert_eq!(
                    decode(&data, Some(&globals), &capped).unwrap_err().kind,
                    ErrorKind::Limit,
                    "{name}"
                );
                let capped = Limits {
                    decoded_bytes: bitmap.data.len() - 1,
                    ..limits
                };
                assert_eq!(
                    decode(&data, Some(&globals), &capped).unwrap_err().kind,
                    ErrorKind::Limit,
                    "{name}"
                );
            }
            outcome => {
                let error = result.expect_err(name);
                assert_eq!(error.kind.name(), outcome, "{name}: {error}");
                if let Some(feature) = params.get("feature") {
                    assert_eq!(error.feature().unwrap().as_str(), *feature, "{name}");
                }
                assert!(
                    matches!(error.position, Position::Buffer(_)),
                    "{name}: {error}"
                );
            }
        }
    }
    assert!(count >= 30);
}

#[test]
fn exact_generic_budgets_are_enforced_before_work() {
    let data = fixture("jbig2", "arithmetic-t2-checker.jb2");
    let expected = fixture("jbig2", "arithmetic-t2-checker.out");
    // The page, temporary region, and 1024 two-byte MQ contexts coexist.
    let bytes = expected.len() * 2 + 1024 * size_of::<super::arithmetic::Context>();
    let work = 19 * 9 * 18 * 2;
    let limits = Limits {
        pixels: 19 * 9,
        decoded_bytes: bytes,
        raster_work: work,
        objects: 2,
        operations: 2,
        stream_bytes: data.len(),
        file_bytes: data.len(),
        ..Limits::default()
    };
    assert_eq!(decode(&data, None, &limits).unwrap().data, expected);
    for capped in [
        Limits {
            decoded_bytes: bytes - 1,
            ..limits
        },
        Limits {
            pixels: 19 * 9 - 1,
            ..limits
        },
        Limits {
            raster_work: work - 1,
            ..limits
        },
        Limits {
            objects: 1,
            ..limits
        },
        Limits {
            operations: 1,
            ..limits
        },
        Limits {
            stream_bytes: data.len() - 1,
            ..limits
        },
        Limits {
            file_bytes: data.len() - 1,
            ..limits
        },
    ] {
        assert_eq!(
            decode(&data, None, &capped).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn truncations_and_mutations_remain_bounded() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 160_000,
        objects: 64,
        value_nodes: 64,
        operations: 256,
        raster_work: 200_000,
        stream_bytes: 4096,
        file_bytes: 8192,
        ..Limits::default()
    };
    for name in [
        "arithmetic-t0-noise",
        "mmr-geometry",
        "symbols-t2-text",
        "refinement-t0-prediction-1",
        "text-refinement-t1-offset-1-2",
        "symbol-multiple-aggregation-t0",
        "symbol-mixed-aggregation-state-t1",
        "retained-refinement-contexts-t0",
    ] {
        let data = fixture("jbig2", &format!("{name}.jb2"));
        let globals = fixture("jbig2", &format!("{name}.globals"));
        for cut in 0..data.len() {
            let _ = decode(&data[..cut], Some(&globals), &limits);
        }
        for cut in 1..globals.len() {
            let _ = decode(&data, Some(&globals[..cut]), &limits);
        }
        for index in 0..data.len() {
            for bit in [1, 8, 128] {
                let mut changed = data.clone();
                changed[index] ^= bit;
                if let Ok(bitmap) = decode(&changed, Some(&globals), &limits) {
                    assert!(bitmap.data.len() <= limits.decoded_bytes);
                    assert!(
                        u64::from(bitmap.width) * u64::from(bitmap.height) <= limits.pixels as u64
                    );
                }
            }
        }
    }
}

#[test]
fn refinement_errors_reach_the_expected_coding_field() {
    for (name, message) in [
        (
            "refinement-adaptive-noncausal",
            "noncausal JBIG2 refinement adaptive pixel",
        ),
        (
            "refinement-reserved-flags",
            "reserved JBIG2 refinement flags",
        ),
        (
            "refinement-two-references",
            "JBIG2 refinement requires at most one reference",
        ),
        (
            "retained-contexts-missing",
            "missing JBIG2 retained symbol contexts",
        ),
        (
            "retained-contexts-incompatible",
            "incompatible JBIG2 retained generic contexts",
        ),
        (
            "retained-refinement-contexts-missing",
            "missing JBIG2 retained refinement contexts",
        ),
        (
            "text-refinement-zero-width",
            "invalid JBIG2 refined symbol dimension",
        ),
        (
            "text-refinement-invalid-indicator",
            "invalid JBIG2 text refinement indicator",
        ),
    ] {
        let data = fixture("jbig2", &format!("{name}.jb2"));
        let globals = fixture("jbig2", &format!("{name}.globals"));
        let error = decode(&data, Some(&globals), &Limits::default()).expect_err(name);
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
        assert_eq!(error.message, message, "{name}");
        assert!(matches!(error.position, Position::Buffer(_)), "{name}");
    }
}
