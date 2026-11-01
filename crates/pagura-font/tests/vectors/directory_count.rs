//! Exact count repair preserves every glyph and every original program byte.
use super::*;

fn bytes(name: &str) -> std::sync::Arc<[u8]> {
    pagura_testkit::fixture("font_directory", &format!("{name}.font")).into()
}

#[test]
fn exact_excess_count_keeps_strict_error_and_all_glyphs() {
    for (kind, format, offset) in [
        ("ttf", FontFormat::TrueType, 52),
        ("cff", FontFormat::OpenType, 20),
        ("cff-cid", FontFormat::OpenType, 20),
    ] {
        let original = bytes(&format!("{kind}-count-plus-one"));
        let error = FontProgram::parse(original.clone(), format, &Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.message, "sfnt table overlaps directory");
        assert_eq!(error.position, Position::Buffer(offset));
        assert_eq!(error.object, None);
        assert_eq!(error.repair, None);
        let mut strict = Diagnostics::new(Recovery::Strict);
        let explicit = FontProgram::parse_with_diagnostics(
            original.clone(),
            format,
            &Limits::default(),
            &mut strict,
        )
        .unwrap_err();
        assert_eq!(explicit.kind, error.kind);
        assert_eq!(explicit.message, error.message);
        assert_eq!(explicit.position, error.position);
        assert_eq!(explicit.object, error.object);
        assert_eq!(explicit.repair, None);
        assert_eq!(strict.as_slice(), []);

        let valid = FontProgram::parse(bytes(&format!("{kind}-valid")), format, &Limits::default())
            .unwrap();
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let repaired = FontProgram::parse_with_diagnostics(
            original.clone(),
            format,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(repaired.data(), original.as_ref());
        assert_eq!(repaired.glyph_count(), valid.glyph_count());
        assert_eq!(repaired.font_matrix(), valid.font_matrix());
        for index in 0..valid.glyph_count() {
            let glyph = GlyphId(index);
            assert_eq!(repaired.glyph_name(glyph), valid.glyph_name(glyph));
            assert_eq!(
                repaired.advance(glyph).unwrap(),
                valid.advance(glyph).unwrap()
            );
            assert_eq!(
                repaired.outline(glyph).unwrap(),
                valid.outline(glyph).unwrap()
            );
        }
        let repairs = diagnostics.as_slice();
        assert_eq!(repairs.len(), 1);
        assert_eq!(
            repairs[0].repair(),
            Some(pagura_font::repairs::SFNT_DIRECTORY_COUNT)
        );
        assert_eq!(repairs[0].position, Position::Buffer(4));
    }
}

#[test]
fn ambiguous_or_incomplete_directories_never_become_a_font_prefix() {
    for kind in ["ttf", "cff-cid"] {
        let format = if kind == "ttf" {
            FontFormat::TrueType
        } else {
            FontFormat::OpenType
        };
        for fault in [
            "count-plus-two",
            "first-offset-inside-record",
            "duplicate-prefix-tag",
            "wrong-first-payload-header",
            "prefix-out-of-bounds",
            "unknown-signature",
            "truncated-extra-header",
        ] {
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let mut diagnostics = Diagnostics::new(recovery);
                let error = FontProgram::parse_with_diagnostics(
                    bytes(&format!("{kind}-{fault}")),
                    format,
                    &Limits::default(),
                    &mut diagnostics,
                )
                .unwrap_err();
                let expected_kind = if fault == "unknown-signature" {
                    ErrorKind::Unsupported(pagura_font::features::FORMAT)
                } else {
                    ErrorKind::Syntax
                };
                assert_eq!(error.kind, expected_kind, "{kind}/{fault}/{recovery:?}");
                assert!(
                    diagnostics
                        .as_slice()
                        .iter()
                        .all(|d| d.repair() != Some(pagura_font::repairs::SFNT_DIRECTORY_COUNT))
                );
            }
        }
    }
}

#[test]
fn proof_work_and_existing_entry_and_input_limits_propagate() {
    for (kind, format) in [
        ("ttf", FontFormat::TrueType),
        ("cff-cid", FontFormat::OpenType),
    ] {
        for limits in [
            Limits {
                font_work: 1,
                ..Limits::default()
            },
            Limits {
                font_entries: 0,
                ..Limits::default()
            },
            Limits {
                decoded_bytes: 1,
                ..Limits::default()
            },
        ] {
            let mut diagnostics = Diagnostics::new(Recovery::Repair);
            let error = FontProgram::parse_with_diagnostics(
                bytes(&format!("{kind}-count-plus-one")),
                format,
                &limits,
                &mut diagnostics,
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Limit);
            assert!(
                diagnostics
                    .as_slice()
                    .iter()
                    .all(|d| d.repair() != Some(pagura_font::repairs::SFNT_DIRECTORY_COUNT))
            );
        }
    }
}

#[test]
fn complete_decline_contracts_and_ttc_outer_scope() {
    use pagura_testkit::{Json, Manifest, fixture_text};
    let mut checked = 0;
    for case in Manifest::load("font_directory").cases() {
        if case.string("expected_repair_render") != Some("refused") {
            continue;
        }
        let name = case.name();
        let contract = Json::parse(&fixture_text(
            "font_directory",
            &format!("{name}.contract.json"),
        ))
        .unwrap();
        for (mode, recovery) in [("strict", Recovery::Strict), ("repair", Recovery::Repair)] {
            let expected = contract
                .get("portable_contracts")
                .unwrap()
                .get(mode)
                .unwrap()
                .get("font")
                .unwrap();
            let detail = expected.get("error").unwrap();
            let format = if case.string("format") == Some("truetype") {
                FontFormat::TrueType
            } else {
                FontFormat::OpenType
            };
            let mut diagnostics = Diagnostics::new(recovery);
            let error = FontProgram::parse_with_diagnostics(
                bytes(name),
                format,
                &Limits::default(),
                &mut diagnostics,
            )
            .unwrap_err();
            let expected_kind = if detail.get("kind").unwrap().as_str() == Some("Syntax") {
                ErrorKind::Syntax
            } else {
                assert_eq!(
                    detail.get("kind").unwrap().as_str(),
                    Some("Unsupported(Feature(\"font.format\"))")
                );
                ErrorKind::Unsupported(pagura_font::features::FORMAT)
            };
            assert_eq!(error.kind, expected_kind, "{name}/{mode}");
            assert_eq!(Some(error.message), detail.get("message").unwrap().as_str());
            assert_eq!(
                error.position,
                Position::Buffer(
                    usize::try_from(detail.get("position").unwrap().as_u64().unwrap()).unwrap()
                )
            );
            assert_eq!(error.object, None);
            assert_eq!(
                error.repair.map(pagura_core::Repair::as_str),
                detail.get("repair").unwrap().as_str()
            );
            let tags: Vec<_> = diagnostics
                .as_slice()
                .iter()
                .map(pagura_core::Diagnostic::tag)
                .collect();
            let expected_tags: Vec<_> = expected
                .get("diagnostics")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|j| j.as_str().unwrap())
                .collect();
            assert_eq!(tags, expected_tags, "{name}/{mode}");
            checked += 1;
        }
    }
    assert_eq!(checked, 56);
}
