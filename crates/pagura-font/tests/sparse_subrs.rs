#![forbid(unsafe_code)]
//! Unavailable Type 1 slots retain indexes, diagnostics and resource limits.
use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};
use pagura_font::{FontFormat, FontProgram, GlyphId, repairs};
use pagura_testkit::fixture;

fn source(name: &str) -> Vec<u8> {
    fixture("font", &format!("{name}.font"))
}

#[test]
fn called_hole_is_never_a_synthesized_return_program() {
    let bytes = source("type1-sparse-called");
    let mut strict = Diagnostics::new(Recovery::Strict);
    let error = FontProgram::parse_with_diagnostics(
        bytes.clone(),
        FontFormat::Type1,
        &Limits::default(),
        &mut strict,
    )
    .unwrap_err();
    assert_eq!(error.repair, Some(repairs::TYPE1_SPARSE_SUBRS));
    assert!(strict.is_empty());
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let font = FontProgram::parse_with_diagnostics(
        bytes.clone(),
        FontFormat::Type1,
        &Limits::default(),
        &mut diagnostics,
    )
    .unwrap();
    assert_eq!(font.data(), bytes);
    let error = font.outline(GlyphId(1)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.repair, Some(repairs::GLYPH_DATA));
    let call_position = error.position;
    assert_eq!(error.message, "unassigned Type 1 subroutine called");
    assert!(matches!(error.position, Position::Buffer(_)));
    // This is the existing explicit glyph-recovery policy, not successful
    // reconstruction of a missing subroutine or an accuracy claim.
    font.outline_with_diagnostics(GlyphId(1), &mut diagnostics)
        .unwrap();
    assert_eq!(diagnostics.as_slice().len(), 2);
    assert_eq!(
        diagnostics.as_slice()[0].repair(),
        Some(repairs::TYPE1_SPARSE_SUBRS)
    );
    assert_eq!(
        diagnostics.as_slice()[1].repair(),
        Some(repairs::GLYPH_DATA)
    );
    assert_eq!(diagnostics.as_slice()[1].position, call_position);
    assert!(matches!(call_position, Position::Buffer(offset) if offset < bytes.len()));
    assert_eq!(font.advance(GlyphId(1)).unwrap(), Some(600.0));
}

#[test]
fn sparse_slots_do_not_waive_parse_or_outline_budgets() {
    let bytes = source("repair-type1-sparse-nested-pfa");
    for limits in [
        Limits {
            font_work: 0,
            ..Limits::default()
        },
        Limits {
            font_entries: 0,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 0,
            ..Limits::default()
        },
    ] {
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let error = FontProgram::parse_with_diagnostics(
            bytes.clone(),
            FontFormat::Type1,
            &limits,
            &mut diagnostics,
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(diagnostics.is_empty());
    }
    // Four declared slots remain four charged entries, including holes.
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let error = FontProgram::parse_with_diagnostics(
        bytes.clone(),
        FontFormat::Type1,
        &Limits {
            font_entries: 3,
            ..Limits::default()
        },
        &mut diagnostics,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert!(diagnostics.is_empty());
    // The glyph dictionary comes after the reported sparse array.
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let error = FontProgram::parse_with_diagnostics(
        bytes.clone(),
        FontFormat::Type1,
        &Limits {
            font_entries: 9,
            ..Limits::default()
        },
        &mut diagnostics,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(diagnostics.as_slice().len(), 1);
    assert_eq!(
        diagnostics.as_slice()[0].repair(),
        Some(repairs::TYPE1_SPARSE_SUBRS)
    );
    for limits in [
        Limits {
            depth: 0,
            ..Limits::default()
        },
        Limits {
            path_segments: 0,
            ..Limits::default()
        },
    ] {
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let font = FontProgram::parse_with_diagnostics(
            bytes.clone(),
            FontFormat::Type1,
            &limits,
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(
            font.outline_with_diagnostics(GlyphId(1), &mut diagnostics)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
        assert_eq!(diagnostics.as_slice().len(), 1);
        assert_eq!(
            diagnostics.as_slice()[0].repair(),
            Some(repairs::TYPE1_SPARSE_SUBRS)
        );
    }
}

#[test]
fn malformed_subroutine_assignments_are_not_sparse_slot_repairs() {
    for name in ["duplicate", "index", "record"] {
        let bytes = source(&format!("type1-sparse-invalid-{name}"));
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let mut diagnostics = Diagnostics::new(recovery);
            let error = FontProgram::parse_with_diagnostics(
                bytes.clone(),
                FontFormat::Type1,
                &Limits::default(),
                &mut diagnostics,
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
            assert!(diagnostics.is_empty(), "{name}");
        }
    }
}
