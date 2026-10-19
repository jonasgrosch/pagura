//! Strict admission tags and independent exact repaired results (ADR 0003).
use super::*;

#[test]
fn generated_repair_vectors() {
    for (suffix, format) in [
        ("sfnt-version", FontFormat::TrueType),
        ("table-alignment", FontFormat::TrueType),
        ("optional-table-missing", FontFormat::TrueType),
        ("cmap-header", FontFormat::TrueType),
        ("table-bounds", FontFormat::TrueType),
        ("required-table-bounds", FontFormat::TrueType),
        ("metrics-bearings", FontFormat::TrueType),
        ("loca-truncated", FontFormat::TrueType),
        ("glyf-truncated", FontFormat::TrueType),
        ("glyph-missing", FontFormat::TrueType),
        ("glyph-truncated", FontFormat::TrueType),
        ("component-matrix", FontFormat::TrueType),
        ("cmap0-prefix", FontFormat::TrueType),
        ("cmap2-prefix", FontFormat::TrueType),
        ("cmap4-prefix", FontFormat::TrueType),
        ("cmap10-prefix", FontFormat::TrueType),
        ("cmap13-prefix", FontFormat::TrueType),
        ("cff-global-escape", FontFormat::Cff),
        ("cmap-truncated", FontFormat::TrueType),
        ("cmap-groups", FontFormat::TrueType),
        ("cmap-sentinel", FontFormat::TrueType),
        ("cmap-byte-alignment", FontFormat::TrueType),
        ("cmap-subtable", FontFormat::TrueType),
        ("cff-escape", FontFormat::Cff),
        ("cff-subr-escape", FontFormat::Cff),
        ("type1-escape", FontFormat::Type1),
        ("type1-subr-escape", FontFormat::Type1),
    ] {
        repair_vector(&format!("repair-{suffix}"), format);
    }
    vector("sfnt-legacy-true", FontFormat::TrueType);
}

fn repair_vector(name: &str, format: FontFormat) {
    let bytes = pagura_testkit::fixture("font", &format!("{name}.font"));
    let strict =
        String::from_utf8(pagura_testkit::fixture("font", &format!("{name}.strict"))).unwrap();
    let words: Vec<_> = strict.split_ascii_whitespace().collect();
    let tag = *words.last().unwrap();
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    let parsed = FontProgram::parse_with_diagnostics(
        bytes.clone(),
        format,
        &Limits::default(),
        &mut diagnostics,
    );
    let error = if words[0] == "parse" {
        parsed.unwrap_err()
    } else {
        parsed
            .unwrap()
            .outline_with_diagnostics(GlyphId(words[1].parse().unwrap()), &mut diagnostics)
            .unwrap_err()
    };
    assert_eq!(error.repair.unwrap().as_str(), tag, "{name}: {error}");
    assert!(matches!(error.position, Position::Buffer(_)), "{name}");
    assert_eq!(diagnostics.as_slice(), []);
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let font = FontProgram::parse_with_diagnostics(
        bytes.clone(),
        format,
        &Limits::default(),
        &mut diagnostics,
    )
    .unwrap();
    assert_eq!(font.data(), bytes);
    let expected =
        String::from_utf8(pagura_testkit::fixture("font", &format!("{name}.expect"))).unwrap();
    check_vector(&font, &expected, name, format, &mut diagnostics);
    let expected_tags =
        String::from_utf8(pagura_testkit::fixture("font", &format!("{name}.repairs"))).unwrap();
    let expected_tags: std::collections::BTreeSet<_> = expected_tags.lines().collect();
    let actual_tags: std::collections::BTreeSet<_> = diagnostics
        .as_slice()
        .iter()
        .map(pagura_core::Diagnostic::tag)
        .collect();
    assert_eq!(actual_tags, expected_tags, "{name}");
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .all(|d| matches!(d.position, Position::Buffer(_)))
    );
}

#[test]
fn required_metadata_and_resource_failures_still_reject() {
    for name in [
        "required-head-missing",
        "required-hhea-missing",
        "required-hmtx-missing",
        "required-loca-missing",
        "required-maxp-missing",
        "truncated-hmtx",
        "descending-loca",
    ] {
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        assert!(
            FontProgram::parse_with_diagnostics(
                pagura_testkit::fixture("font", &format!("{name}.font")),
                FontFormat::TrueType,
                &Limits::default(),
                &mut diagnostics
            )
            .is_err(),
            "{name}"
        );
    }
    // Exhausted budgets reject in both modes: raising the limits admits them.
    for (name, limits) in [
        (
            "cff-branching-subrs",
            Limits {
                font_work: 2000,
                ..Limits::default()
            },
        ),
        (
            "cff-recursive",
            Limits {
                depth: 5,
                ..Limits::default()
            },
        ),
    ] {
        let font = FontProgram::parse(
            pagura_testkit::fixture("font", &format!("{name}.font")),
            FontFormat::Cff,
            &limits,
        )
        .unwrap();
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let error = font
            .outline_with_diagnostics(GlyphId(1), &mut diagnostics)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{name}: {error}");
        assert_eq!(diagnostics.as_slice(), []);
    }
}

#[test]
fn format_ceilings_and_nonfinite_arithmetic_are_malformed_glyphs() {
    // TN 5177 Appendix B ceilings (operand stack, ten nested calls) and
    // arithmetic beyond f64 range make one glyph malformed, whatever the
    // limits: strict rejects it as syntax, repair replaces only that glyph.
    for name in ["cff-nonfinite", "cff-recursive", "cff-stack"] {
        let font = font(name, FontFormat::Cff);
        let error = font.outline(GlyphId(1)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
        assert!(matches!(error.position, Position::Buffer(_)), "{name}");
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let path = font
            .outline_with_diagnostics(GlyphId(1), &mut diagnostics)
            .unwrap();
        assert!(path.is_empty(), "{name}");
        let tags: Vec<_> = diagnostics
            .as_slice()
            .iter()
            .map(pagura_core::Diagnostic::tag)
            .collect();
        assert_eq!(tags, [pagura_font::repairs::GLYPH_DATA.as_str()], "{name}");
        assert!(font.outline(GlyphId(0)).is_ok(), "{name}");
    }
}

#[test]
fn repair_diagnostics_are_bounded_and_repeated_calls_are_independent() {
    let font = font("repair-component-matrix", FontFormat::TrueType);
    // The strict compatibility API admits no repair.
    assert_eq!(
        font.outline(GlyphId(3)).unwrap_err().repair,
        Some(pagura_font::repairs::COMPONENT_FLAGS)
    );
    for _ in 0..2 {
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        assert!(
            !font
                .outline_with_diagnostics(GlyphId(3), &mut diagnostics)
                .unwrap()
                .is_empty()
        );
        assert_eq!(diagnostics.as_slice().len(), 1);
    }
}

#[test]
fn repair_mode_mutations_keep_work_and_diagnostics_bounded() {
    let limits = Limits {
        font_work: 2000,
        font_entries: 1000,
        depth: 8,
        path_segments: 500,
        decoded_bytes: 16384,
        ..Limits::default()
    };
    for (name, format) in [
        ("repair-cff-subr-escape", FontFormat::Cff),
        ("repair-cff-empty-other-blues", FontFormat::Cff),
        ("repair-cff-reserved-private", FontFormat::Cff),
        ("repair-cff-supplement-truncated", FontFormat::Cff),
        ("repair-type1-duplicate-name", FontFormat::Type1),
        ("repair-type1-subr-escape", FontFormat::Type1),
        ("repair-component-matrix", FontFormat::TrueType),
        ("repair-cmap-groups", FontFormat::TrueType),
    ] {
        let original = pagura_testkit::fixture("font", &format!("{name}.font"));
        for i in 0..original.len().min(128) {
            let mut bytes = original.clone();
            let at = i * original.len() / original.len().min(128);
            bytes[at] ^= 0xff;
            let mut diagnostics = Diagnostics::new(Recovery::Repair);
            if let Ok(font) =
                FontProgram::parse_with_diagnostics(bytes, format, &limits, &mut diagnostics)
            {
                assert!(diagnostics.take().len() <= 9);
                for glyph in 0..font.glyph_count().min(5) {
                    if let Ok(path) =
                        font.outline_with_diagnostics(GlyphId(glyph), &mut diagnostics)
                    {
                        assert!(path.segments().len() <= limits.path_segments);
                        assert!(finite(&path));
                    }
                    assert!(diagnostics.take().len() <= 9);
                }
            } else {
                assert!(diagnostics.take().len() <= 9);
            }
        }
    }
}

#[test]
fn unused_cff_blue_hint_order_is_reported_in_all_containers() {
    for (name, format) in [
        ("operators", FontFormat::Cff),
        ("family-operators", FontFormat::Cff),
        ("values", FontFormat::Cff),
        ("other", FontFormat::Cff),
        ("family", FontFormat::Cff),
        ("family-other", FontFormat::Cff),
        ("opentype", FontFormat::OpenType),
        ("cid", FontFormat::Cff),
        ("cff2", FontFormat::OpenType),
    ] {
        repair_vector(&format!("repair-cff-blue-{name}"), format);
    }
}

#[test]
fn malformed_font_metadata_has_exact_repaired_geometry_and_mappings() {
    for suffix in [
        "empty-blue-values",
        "empty-other-blues",
        "empty-family-blues",
        "empty-family-other-blues",
        "empty-snap-h",
        "empty-snap-v",
        "scalar-hint-arity",
        "reserved-private",
        "reserved-top",
        "reserved-repeated",
        "supplement-unencoded",
        "supplement-missing-sid",
        "supplement-conflict",
        "supplement-truncated",
        "supplement-missing-count",
    ] {
        repair_vector(&format!("repair-cff-{suffix}"), FontFormat::Cff);
    }
    for suffix in ["name", "notdef", "empty"] {
        repair_vector(
            &format!("repair-type1-duplicate-{suffix}"),
            FontFormat::Type1,
        );
    }
}

#[test]
fn relevant_dict_operands_and_reserved_number_bytes_still_reject() {
    for suffix in [
        "width-arity",
        "subrs-arity",
        "seed-arity",
        "matrix-arity",
        "reserved-number-31",
        "reserved-number-255",
    ] {
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        assert!(
            FontProgram::parse_with_diagnostics(
                pagura_testkit::fixture("font", &format!("cff-dict-{suffix}.font")),
                FontFormat::Cff,
                &Limits::default(),
                &mut diagnostics
            )
            .is_err(),
            "{suffix}"
        );
        assert!(diagnostics.is_empty());
    }
}

#[test]
fn duplicate_empty_name_keeps_the_first_charstring() {
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let font = FontProgram::parse_with_diagnostics(
        pagura_testkit::fixture("font", "repair-type1-duplicate-empty.font"),
        FontFormat::Type1,
        &Limits::default(),
        &mut diagnostics,
    )
    .unwrap();
    assert_eq!(font.glyph_for_name(b""), Some(GlyphId(2)));
    assert_eq!(font.advance(GlyphId(2)), Some(600.0));
    assert_eq!(diagnostics.as_slice().len(), 1);
}

#[test]
fn metadata_repairs_coalesce_and_identify_the_operator_or_record() {
    let data = pagura_testkit::fixture("font", "repair-cff-reserved-repeated.font");
    let op_at = data.windows(2).position(|p| p == [12, 255]).unwrap();
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    let error = FontProgram::parse_with_diagnostics(
        data.clone(),
        FontFormat::Cff,
        &Limits::default(),
        &mut diagnostics,
    )
    .unwrap_err();
    assert_eq!(error.position, Position::Buffer(op_at));
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    FontProgram::parse_with_diagnostics(
        data,
        FontFormat::Cff,
        &Limits::default(),
        &mut diagnostics,
    )
    .unwrap();
    assert_eq!(diagnostics.as_slice().len(), 1);
    assert_eq!(diagnostics.as_slice()[0].position, Position::Buffer(op_at));
}

fn finite(path: &pagura_raster::Path) -> bool {
    path.segments().iter().all(|segment| match *segment {
        Segment::MoveTo(p) | Segment::LineTo(p) => p.is_finite(),
        Segment::CubicTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
        Segment::Close => true,
    })
}
