#![forbid(unsafe_code)]
//! OpenType literal maps are authored independently of the font parser.
use pagura_core::{Error, ErrorKind, Limits, Result};
use pagura_font::{CmapSubtable, FontFormat, FontProgram, GlyphId};
use pagura_testkit::fixture;

fn font(name: &str) -> Result<FontProgram> {
    FontProgram::parse(
        fixture("vertical_fonts", &format!("{name}.font")),
        FontFormat::TrueType,
        &Limits::default(),
    )
}

#[test]
fn selected_features_follow_language_lookup_order_and_single_subtable_priority() -> Result<()> {
    for name in [
        "single-delta",
        "single-ranges",
        "extension",
        "lookup-order",
        "first-subtable",
        "vrt2-preferred",
        "unused-vert-offset",
        "unused-vert-entries",
        "unused-default-script",
        "default-script",
        "named-language",
        "missing-feature",
        "missing-script",
    ] {
        let program = font(name)?;
        let expected = fixture("vertical_fonts", &format!("{name}.map"));
        let expected =
            std::str::from_utf8(&expected).map_err(|_| Error::syntax("invalid oracle"))?;
        let map = program.vertical_substitution(*b"hani", Some(*b"JAN "))?;
        if expected.trim() == "none" {
            assert!(map.is_none(), "{name}");
            continue;
        }
        let map = map.ok_or_else(|| Error::syntax("missing vertical mapping"))?;
        for (input, expected) in expected.split_whitespace().enumerate() {
            let expected: u32 = expected
                .parse()
                .map_err(|_| Error::syntax("invalid glyph oracle"))?;
            assert_eq!(
                map.apply(GlyphId(u32::try_from(input).unwrap())),
                GlyphId(expected),
                "{name}"
            );
        }
        assert_eq!(
            program.glyph_for_code(CmapSubtable::WindowsUnicode, 65),
            Some(GlyphId(1))
        );
        assert_eq!(program.advance(GlyphId(1))?, Some(600.0));
        assert!(map.retained_bytes() <= 5 * 4);
    }
    let program = font("named-language")?;
    let default = program
        .vertical_substitution(*b"hani", Some(*b"ZHS "))?
        .unwrap();
    assert_eq!(default.apply(GlyphId(1)), GlyphId(2));
    assert_eq!(default.apply(GlyphId(2)), GlyphId(2));
    Ok(())
}

#[test]
fn malformed_selected_tables_and_unsupported_lookup_kinds_do_not_change_horizontal_parsing()
-> Result<()> {
    for name in [
        "coverage-order",
        "coverage-range-index",
        "coverage-count",
        "output-range",
        "coverage-range",
        "null-coverage",
        "lookup-index",
        "feature-index",
        "reserved-flags",
        "filtering",
        "contextual",
        "variable-feature",
        "extension-kind-0",
        "extension-kind-7",
        "extension-kind-9",
        "extension-kind-5",
    ] {
        let program = font(name)?;
        assert_eq!(
            program.glyph_for_code(CmapSubtable::WindowsUnicode, 65),
            Some(GlyphId(1))
        );
        let error = program
            .vertical_substitution(*b"hani", Some(*b"JAN "))
            .unwrap_err();
        let expected = fixture("vertical_fonts", &format!("{name}.error"));
        if expected == b"syntax\n" {
            assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
        } else {
            assert!(
                matches!(error.kind, ErrorKind::Unsupported(_)),
                "{name}: {error}"
            );
        }
    }
    Ok(())
}
