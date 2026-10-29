//! Independent generator vectors and hostile-input boundaries.
use super::{Bitmap, decode};
use pagura_core::{ErrorKind, Limits, Position};
use pagura_testkit::{fixture, fixture_path};

#[test]
fn contradictory_page_capabilities_are_reported_without_changing_samples() {
    use pagura_core::{Diagnostics, Recovery};

    // The independent encoders write an eleven-byte first-segment header,
    // then four u32 page fields before the flag byte (T.88 7.4.8).
    const PAGE_FLAGS: usize = 11 + 16;
    for (name, missing) in [
        ("intermediate-default-black", 32),
        ("refinement-t1-prediction-0", 34),
        ("refinement-page-t1-prediction-0", 2),
    ] {
        let mut data = fixture("jbig2", &format!("{name}.jb2")).to_vec();
        *data.get_mut(PAGE_FLAGS).unwrap() &= !missing;
        let limits = Limits::default();
        let error = decode(&data, None, &limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
        assert_eq!(error.repair, Some(super::repairs::PAGE_CAPABILITIES));
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let bitmap = super::decode_with_diagnostics(&data, None, &limits, &mut diagnostics)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(bitmap.data, fixture("jbig2", &format!("{name}.out")));
        assert_eq!(diagnostics.as_slice().len(), 1, "{name}");
        assert_eq!(
            diagnostics.as_slice()[0].repair(),
            Some(super::repairs::PAGE_CAPABILITIES)
        );
        assert!(matches!(
            diagnostics.as_slice()[0].position,
            Position::Buffer(_)
        ));
        for capped in [
            Limits {
                pixels: 0,
                ..limits
            },
            Limits {
                decoded_bytes: 0,
                ..limits
            },
            Limits {
                decode_work: 0,
                ..limits
            },
            Limits {
                objects: 0,
                ..limits
            },
        ] {
            let error = super::decode_with_diagnostics(
                &data,
                None,
                &capped,
                &mut Diagnostics::new(Recovery::Repair),
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit, "{name}");
        }
        data.pop();
        assert_eq!(
            super::decode_with_diagnostics(&data, None, &limits, &mut diagnostics)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax,
            "truncation remains an error: {name}"
        );
    }
}

#[test]
fn independent_bitmaps_and_hostile_vectors() {
    let mut count = 0;
    let mut failures = Vec::new();
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
                let bitmap = match result {
                    Ok(bitmap) => bitmap,
                    Err(error) => {
                        failures.push(format!("{name}: {error}"));
                        continue;
                    }
                };
                let expected = Bitmap {
                    width: params["width"].parse().unwrap(),
                    height: params["height"].parse().unwrap(),
                    data: fixture("jbig2", &format!("{name}.out")),
                };
                if bitmap != expected {
                    failures.push(format!("{name}: bitmap mismatch"));
                }
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
                if error.kind.name() != outcome {
                    failures.push(format!("{name}: expected {outcome}, got {error}"));
                    continue;
                }
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
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
        decode_work: work + 2,
        objects: 2,
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
            decode_work: work + 1,
            ..limits
        },
        Limits {
            objects: 1,
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
        decode_work: 204_096,
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
        "huffman-user-width-table",
        "huffman-symbol-aggregation-t0",
        "huffman-retained-refinement-t1",
        "huffman-layout-1-3",
        "halftone-arithmetic-t0-skip1-r128-op4",
        "halftone-mmr-t0-skip0-r0-op2",
        "unknown-length-arithmetic",
        "unknown-length-mmr",
        "unknown-length-arithmetic-t0-prediction1-followed",
        "unknown-length-mmr-width3-followed",
        "arithmetic-layout-clipped-refined-1-2",
        "huffman-layout-clipped-1-3",
        "arithmetic-layout-striped-0-0",
        "profiles-page-superset",
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
            "retained-context-adaptive-mismatch",
            "incompatible JBIG2 retained bitmap parameters",
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

#[test]
fn huffman_table_setup_is_charged_before_construction() {
    let data = fixture("jbig2", "huffman-symbols-raw-4-2.jb2");
    let globals = fixture("jbig2", "huffman-symbols-raw-4-2.globals");
    let limits = Limits {
        decode_work: super::huffman::STANDARD_ROWS,
        ..Limits::default()
    };
    let error = decode(&data, Some(&globals), &limits).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, "JBIG2 cumulative operations exceed budget");
}

#[test]
fn unknown_length_allocates_actual_rows_and_keeps_following_segment() {
    let data = fixture(
        "jbig2",
        "unknown-length-arithmetic-t2-prediction0-followed.jb2",
    );
    let expected = fixture(
        "jbig2",
        "unknown-length-arithmetic-t2-prediction0-followed.out",
    );
    // The region declares 0xFFFFFFFF rows, but contains only three. The
    // page, region and template-2 contexts coexist; the next region adds a
    // fourth painted row after the count without increasing the peak bytes.
    let bytes = 5 + 3 + 1024 * size_of::<super::arithmetic::Context>();
    let work = (25 + 15 + 5) * 18;
    let limits = Limits {
        pixels: 25,
        decoded_bytes: bytes,
        decode_work: work + 3,
        objects: 3,
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
            decode_work: work + 2,
            ..limits
        },
        Limits {
            stream_bytes: data.len() - 1,
            ..limits
        },
    ] {
        assert_eq!(
            decode(&data, None, &capped).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}
