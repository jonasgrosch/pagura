//! Complete packaging and hostile metadata, using independent T.88 encoders.

use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};
use pagura_image::jbig2::{self, Bitmap};
use pagura_testkit::{fixture, fixture_path};
use std::collections::BTreeMap;

const AREA: &str = "recovery_jbig2_wrapper";

#[test]
fn complete_independent_bitmaps_and_metadata_refusals() {
    let mut count = 0;
    for entry in std::fs::read_dir(fixture_path(AREA, "")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|suffix| suffix != "params") {
            continue;
        }
        count += 1;
        let name = path.file_stem().unwrap().to_str().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let params: BTreeMap<_, _> = text
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        let data = fixture(AREA, &format!("{name}.jb2"));
        let globals = fixture(AREA, &format!("{name}.globals"));
        let limits = Limits::default();
        let strict = jbig2::decode(&data, Some(&globals), &limits);
        if params["outcome"] == "syntax" {
            assert_eq!(strict.unwrap_err().kind, ErrorKind::Syntax, "{name}");
        } else {
            assert!(strict.is_ok(), "{name}: {strict:?}");
        }
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let result =
            jbig2::decode_with_diagnostics(&data, Some(&globals), &limits, &mut diagnostics);
        let expected_tags: Vec<_> = params["repairs"]
            .split(',')
            .filter(|tag| !tag.is_empty())
            .collect();
        assert_eq!(
            diagnostics
                .as_slice()
                .iter()
                .map(pagura_core::Diagnostic::tag)
                .collect::<Vec<_>>(),
            expected_tags,
            "{name}"
        );
        if params["repair_outcome"] == "syntax" {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax, "{name}");
            continue;
        }
        let expected = Bitmap {
            width: params["width"].parse().unwrap(),
            height: params["height"].parse().unwrap(),
            data: fixture(AREA, &format!("{name}.out")),
        };
        assert_eq!(result.unwrap(), expected, "{name}");
        if !expected_tags.is_empty() {
            let mut strict = Diagnostics::new(Recovery::Strict);
            let error = jbig2::decode_with_diagnostics(&data, Some(&globals), &limits, &mut strict)
                .unwrap_err();
            assert_eq!(error.repair, Some(jbig2::repairs::FILE_WRAPPER), "{name}");
            assert_eq!(error.position, Position::Buffer(0), "{name}");
            assert!(strict.is_empty());
            assert_eq!(diagnostics.as_slice()[0].position, Position::Buffer(0));
        }
        if expected_tags.contains(&"jbig2.eof-page-association") {
            // The independently authored terminal header uses a one-byte page
            // field, or four bytes in this explicitly declared wide-field case.
            let trailing = if name == "native-long-page-associations" {
                8
            } else {
                5
            };
            assert_eq!(
                diagnostics.as_slice()[1].position,
                Position::Buffer(data.len() - trailing)
            );
        }
    }
    assert_eq!(count, 34);
}

#[test]
fn controls_count_toward_existing_segment_and_input_budgets() {
    let data = fixture(AREA, "native-arithmetic-file-eof0.jb2");
    let expected = fixture(AREA, "native-arithmetic-file-eof0.out");
    let limits = Limits {
        objects: 4,
        ..Limits::default()
    };
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    assert_eq!(
        jbig2::decode_with_diagnostics(&data, None, &limits, &mut diagnostics)
            .unwrap()
            .data,
        expected
    );
    for capped in [
        Limits {
            objects: 3,
            ..limits
        },
        Limits {
            pixels: 11 * 7 - 1,
            ..limits
        },
        Limits {
            decoded_bytes: expected.len() - 1,
            ..limits
        },
        Limits {
            decode_work: 0,
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
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        assert_eq!(
            jbig2::decode_with_diagnostics(&data, None, &capped, &mut diagnostics)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn wrapped_globals_remain_forbidden_and_repeated_reports_are_bounded() {
    let embedded = fixture(AREA, "native-arithmetic-embedded-control.jb2");
    let wrapped = fixture(AREA, "native-arithmetic-file-eof1.jb2");
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let error = jbig2::decode_with_diagnostics(
        &embedded,
        Some(&wrapped),
        &Limits::default(),
        &mut diagnostics,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.position, Position::Buffer(0));
    assert!(diagnostics.is_empty());
    for _ in 0..20 {
        jbig2::decode_with_diagnostics(&wrapped, None, &Limits::default(), &mut diagnostics)
            .unwrap();
    }
    assert_eq!(diagnostics.as_slice().len(), 2);
}
