//! Independent generator vectors and hostile-input boundaries.
use super::arithmetic::Context;
use super::bitmap::{PIXEL_WORK, SHARED_BYTES};
use super::{Bitmap, MESSAGES, SEGMENT_BYTES, decode};
use pagura_core::convert::{u32_to_usize, usize_to_u64};
use pagura_core::{ErrorKind, Limits, Position};
use pagura_testkit::{fixture, fixture_path};
use std::rc::Rc;

/// Limits under which the decoder must not consult the fields that JBIG2
/// once borrowed for its own budgets (ADR 0016).
fn unborrowed() -> Limits {
    Limits {
        objects: 0,
        value_nodes: 0,
        operations: 0,
        raster_work: 0,
        stream_bytes: 0,
        file_bytes: 0,
        ..Limits::default()
    }
}

/// Asserts that `limits` decode `data` to `expected` and that each of
/// `capped` fails with its `Limit` message.
fn assert_exact_budgets(
    data: &[u8],
    globals: Option<&[u8]>,
    expected: &[u8],
    limits: Limits,
    capped: &[(Limits, &str)],
) {
    assert_eq!(decode(data, globals, &limits).unwrap().data(), expected);
    for (capped, message) in capped {
        let error = decode(data, globals, capped).unwrap_err();
        assert_eq!((error.kind, error.message), (ErrorKind::Limit, *message));
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
                let width: u32 = params["width"].parse().unwrap();
                let height: u32 = params["height"].parse().unwrap();
                let expected = fixture("jbig2", &format!("{name}.out"));
                if (bitmap.width(), bitmap.height(), bitmap.data())
                    != (width, height, expected.as_slice())
                {
                    failures.push(format!("{name}: bitmap mismatch"));
                }
                let capped = Limits {
                    pixels: u32_to_usize(width * height) - 1,
                    ..limits
                };
                assert_eq!(
                    decode(&data, Some(&globals), &capped).unwrap_err().kind,
                    ErrorKind::Limit,
                    "{name}"
                );
                // The page is the output: it alone is charged to
                // `decoded_bytes`, so exactly its size suffices.
                let exact = Limits {
                    decoded_bytes: expected.len(),
                    ..unborrowed()
                };
                assert_eq!(
                    decode(&data, Some(&globals), &exact).as_ref(),
                    Ok(&bitmap),
                    "{name}"
                );
                let capped = Limits {
                    decoded_bytes: expected.len() - 1,
                    ..limits
                };
                let error = decode(&data, Some(&globals), &capped).unwrap_err();
                assert_eq!(
                    (error.kind, error.message),
                    (ErrorKind::Limit, MESSAGES.output),
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
    // A 19×9 page and one immediate template-2 generic region of the same
    // size: the page is the output; the region and its 1024 contexts are
    // working memory, next to the two segment records.
    let (pixels, page, contexts) = (19 * 9, expected.len(), 1024);
    let memory = 2 * SEGMENT_BYTES + page + contexts * size_of::<Context>();
    // One unit per segment, the pixel work of the page and of the region,
    // and one unit per byte or context allocated.
    let work = 2 + 2 * pixels * PIXEL_WORK + page + page + contexts;
    let limits = Limits {
        pixels,
        decode_work: work,
        decode_bytes: memory,
        decoded_bytes: page,
        ..unborrowed()
    };
    assert_exact_budgets(
        &data,
        None,
        &expected,
        limits,
        &[
            (
                Limits {
                    decoded_bytes: page - 1,
                    ..limits
                },
                MESSAGES.output,
            ),
            (
                Limits {
                    decode_bytes: memory - 1,
                    ..limits
                },
                MESSAGES.memory,
            ),
            (
                Limits {
                    decode_work: work - 1,
                    ..limits
                },
                MESSAGES.work,
            ),
            (
                Limits {
                    pixels: pixels - 1,
                    ..limits
                },
                "JBIG2 bitmap exceeds the pixel budget",
            ),
        ],
    );
}

#[test]
fn dictionaries_are_working_memory_and_the_page_is_the_output() {
    let data = fixture("jbig2", "symbols-t2-text.jb2");
    let globals = fixture("jbig2", "symbols-t2-text.globals");
    let expected = fixture("jbig2", "symbols-t2-text.out");
    // The globals hold one template-2 dictionary of five symbols, 1, 2 or 3
    // rows high, all exported; the page stream holds the 19×12 page and an
    // immediate text region that refers to the dictionary.
    let (symbols, symbol_bytes, contexts) = (5, 3 + 3 + 2 + 1 + 2, 1024);
    // What the dictionary keeps: its record, its symbols and its export
    // table.
    let kept =
        SEGMENT_BYTES + symbol_bytes + symbols * SHARED_BYTES + symbols * size_of::<Rc<Bitmap>>();
    // While it is decoded, its table of new symbols and its contexts too.
    let dictionary = kept + symbols * size_of::<Rc<Bitmap>>() + contexts * size_of::<Context>();
    let page = expected.len();
    // The text region's working memory next to the dictionary and the page
    // information: its referral, the imported symbols, the identifier
    // contexts and the region, all released once it is drawn.
    let text = kept
        + 2 * SEGMENT_BYTES
        + size_of::<u32>()
        + symbols * size_of::<Rc<Bitmap>>()
        + symbols.next_power_of_two() * size_of::<Context>()
        + page;
    let memory = dictionary.max(text);
    assert_eq!(memory, dictionary);
    let limits = Limits {
        decode_bytes: memory,
        decoded_bytes: page,
        ..unborrowed()
    };
    assert_exact_budgets(
        &data,
        Some(&globals),
        &expected,
        limits,
        &[
            (
                Limits {
                    decoded_bytes: page - 1,
                    ..limits
                },
                MESSAGES.output,
            ),
            (
                Limits {
                    decode_bytes: memory - 1,
                    ..limits
                },
                MESSAGES.memory,
            ),
        ],
    );
    // The dictionary alone exceeds the page bytes, and one byte less than it
    // needs fails while the globals are decoded.
    assert!(dictionary > page);
    let short = Limits {
        decode_bytes: dictionary - 1,
        ..limits
    };
    assert!(
        decode(&[], Some(&globals), &short).is_err_and(|error| error.message == MESSAGES.memory)
    );
    let enough = Limits {
        decode_bytes: dictionary,
        ..limits
    };
    assert_eq!(
        decode(&[], Some(&globals), &enough).unwrap_err().message,
        "missing JBIG2 page information"
    );
}

#[test]
fn working_memory_bounds_one_region_at_a_time() {
    let data = fixture("jbig2", "arithmetic-t2-checker.jb2");
    let expected = fixture("jbig2", "arithmetic-t2-checker.out");
    // The page information segment, then one immediate generic region
    // drawn with OR; the headers have short segment numbers, no referrals
    // and one-byte page associations, so their data lengths are at 7..11.
    let length = u32::from_be_bytes(data[7..11].try_into().unwrap());
    let (page_information, region) = data.split_at(11 + u32_to_usize(length));
    // Drawing the same region many times over leaves the page as it is.
    let regions = 64_u32;
    let mut stream = page_information.to_vec();
    for number in 2..2 + regions {
        let mut segment = region.to_vec();
        segment[..4].copy_from_slice(&number.to_be_bytes());
        stream.extend_from_slice(&segment);
    }
    // Every segment record stays; of the regions' working memory, only one
    // region's contexts and bitmap are held at a time.
    let transients = 1024 * size_of::<Context>() + expected.len();
    let memory = (1 + u32_to_usize(regions)) * SEGMENT_BYTES + transients;
    assert!(memory < u32_to_usize(regions) * transients);
    let limits = Limits {
        decode_bytes: memory,
        ..unborrowed()
    };
    assert_exact_budgets(
        &stream,
        None,
        &expected,
        limits,
        &[(
            Limits {
                decode_bytes: memory - 1,
                ..limits
            },
            MESSAGES.memory,
        )],
    );
}

#[test]
fn truncations_and_mutations_remain_bounded() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 4096,
        decode_bytes: 400_000,
        decode_work: 400_000,
        ..unborrowed()
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
                    assert!(bitmap.data().len() <= limits.decoded_bytes);
                    assert!(
                        u64::from(bitmap.width()) * u64::from(bitmap.height())
                            <= usize_to_u64(limits.pixels)
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
    // The dictionary segment's own unit of work comes first, so the setup
    // charge is the one that fails.
    let limits = Limits {
        decode_work: super::huffman::STANDARD_ROWS,
        ..Limits::default()
    };
    let error = decode(&data, Some(&globals), &limits).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, MESSAGES.work);
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
    // The region declares 0xFFFFFFFF rows, but contains only three; the
    // next segment is a one-row MMR region below it on the 5×5 page.
    let page = expected.len();
    let (region, contexts, follower) = (3, 1024, 1);
    // The follower's two changing-element lists of a 5-pixel row.
    let lists = 2 * 13 * size_of::<usize>();
    // The first region's contexts and bitmap are released before the
    // follower allocates its own, so the first region sets the peak.
    let memory = 2 * SEGMENT_BYTES + region + contexts * size_of::<Context>();
    assert!(3 * SEGMENT_BYTES + follower + lists < memory);
    // Every MMR bit read is work: the row's two horizontal-mode codes take
    // 12 and 11 bits, the end-of-block code 24.
    let mmr_bits = 12 + 11 + 24;
    let work = 3 + (25 + 15 + 5) * PIXEL_WORK + page + region + contexts + follower + mmr_bits;
    let limits = Limits {
        pixels: 25,
        decode_work: work,
        decode_bytes: memory,
        decoded_bytes: page,
        ..unborrowed()
    };
    assert_exact_budgets(
        &data,
        None,
        &expected,
        limits,
        &[
            (
                Limits {
                    decoded_bytes: page - 1,
                    ..limits
                },
                MESSAGES.output,
            ),
            (
                Limits {
                    decode_bytes: memory - 1,
                    ..limits
                },
                MESSAGES.memory,
            ),
            (
                Limits {
                    decode_work: work - 1,
                    ..limits
                },
                MESSAGES.work,
            ),
        ],
    );
}
