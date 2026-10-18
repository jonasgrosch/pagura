//! Decoding vectors through the public API: Type 3, embedded programs,
//! font stream repairs and explicit load failures.
use crate::support::{dictionary, near, number, open, rejected, row, text, uint, values};
use pagura_core::{
    Error, Limits, Recovery, Result,
    syntax::{Origin, Parser},
};
use pagura_raster::Transform;
use pagura_testkit::fixture;
use pagura_text::{FontKind, GlyphShape, PdfFont, WritingMode, repairs};

#[test]
fn only_outline_fonts_expose_their_program() -> Result<()> {
    let type3 = open("type3-differences", Recovery::Repair)?;
    assert!(
        PdfFont::load(&type3, dictionary(&type3)?)?
            .program()
            .is_none()
    );
    let embedded = open("embedded-cid-cff", Recovery::Repair)?;
    let font = PdfFont::load(&embedded, dictionary(&embedded)?)?;
    let program = font
        .program()
        .ok_or_else(|| Error::syntax("missing embedded program"))?;
    let glyph = font.decode(b"\0\x11")?.remove(0);
    let GlyphShape::Outline { program: drawn, .. } = glyph.shape else {
        return Err(Error::syntax("expected an outline glyph"));
    };
    assert!(std::sync::Arc::ptr_eq(program, &drawn));
    Ok(())
}

#[test]
fn type3_decoding_vectors() -> Result<()> {
    for name in [
        "type3-differences",
        "type3-undefined",
        "type3-matrix",
        "encoding-winansi",
        "encoding-macroman",
        "encoding-standard",
        "encoding-macexpert",
        "encoding-symbol",
        "encoding-dingbats",
        "encoding-pdfdoc",
        "tounicode-precedence",
        "tounicode-empty",
    ] {
        let document = open(name, Recovery::Repair)?;
        let font = PdfFont::load(&document, dictionary(&document)?)?;
        assert_eq!(font.kind(), FontKind::Type3);
        assert_eq!(font.writing_mode(), WritingMode::Horizontal);
        assert!(
            font.type3_resources()
                .is_some_and(|d| d.contains_key(b"ProcSet"))
        );
        let actual = font.decode(&fixture("text", &format!("{name}.input")))?;
        let expected = values(name, "out")?;
        assert_eq!(actual.len(), expected.len(), "{name}");
        for (glyph, expected) in actual.iter().zip(&expected) {
            let row = row(expected)?;
            assert_eq!(glyph.code, uint(&row[0])?, "{name}");
            assert_eq!(u32::from(glyph.byte_len), uint(&row[1])?);
            near(glyph.advance, number(&row[2])?);
            assert_eq!(Some(glyph.word_space), row[3].as_bool());
            assert_eq!(
                glyph.unicode.as_deref(),
                text(&row[4]),
                "{name}: {}",
                glyph.code
            );
            assert_eq!(glyph.vertical, None);
            if row[5].is_null() {
                assert!(matches!(glyph.shape, GlyphShape::Blank));
                continue;
            }
            let GlyphShape::Type3 { procedure, matrix } = glyph.shape else {
                return Err(Error::syntax("expected Type3 shape"));
            };
            assert_eq!(procedure.number, uint(&row[5])?);
            if name == "type3-matrix" {
                assert_eq!(
                    matrix,
                    Transform::new(0.002, 0.003, -0.004, 0.005, 2.0, 3.0)
                );
            }
        }
        assert!(font.decode(&[])?.is_empty());
    }
    Ok(())
}

#[test]
fn font_stream_length_faults_are_reported_repairs() -> Result<()> {
    for name in [
        "binding-length1",
        "binding-cff-length",
        "binding-type1-lengths",
    ] {
        let strict = open(name, Recovery::Strict)?;
        let error = rejected(PdfFont::load(&strict, dictionary(&strict)?))?;
        assert_eq!(
            error.repair,
            Some(repairs::STREAM_LENGTHS),
            "{name}: {error}"
        );
        let tag = fixture("text", &format!("{name}.repair"));
        assert_eq!(
            std::str::from_utf8(&tag).map(str::trim),
            Ok(repairs::STREAM_LENGTHS.as_str())
        );
        let repaired = open(name, Recovery::Repair)?;
        PdfFont::load(&repaired, dictionary(&repaired)?)?;
        assert!(
            repaired
                .diagnostics()
                .iter()
                .any(|item| item.repair() == Some(repairs::STREAM_LENGTHS)),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn font_error_vectors() -> Result<()> {
    for name in [
        "nonembedded-type1",
        "nonembedded-mmtype1",
        "nonembedded-truetype",
        "predefined-cmap",
        "differences-overflow",
        "binding-wrong-fontfile",
    ] {
        let document = open(name, Recovery::Repair)?;
        let error = rejected(PdfFont::load(&document, dictionary(&document)?))?;
        let expected = fixture("text", &format!("{name}.error"));
        let expected = std::str::from_utf8(&expected)
            .map_err(|_| Error::syntax("invalid error oracle"))?
            .trim();
        assert_eq!(
            error.feature().map_or(error.kind.name(), |f| f.as_str()),
            expected,
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn missing_cid_map_entry_uses_cid_zero() -> Result<()> {
    check_program_vector("cid-map-missing-entry")?;
    check_program_vector("cid-map-explicit-zero")
}

fn check_program_vector(name: &str) -> Result<()> {
    let document = open(name, Recovery::Repair)?;
    let font = PdfFont::load(&document, dictionary(&document)?)?;
    let glyphs = font.decode(&fixture("text", &format!("{name}.input")))?;
    let expectations = values(name, "out")?;
    assert_eq!(glyphs.len(), expectations.len(), "{name}");
    let matrix_bytes = fixture("text", &format!("{name}.matrix"));
    let matrix = Parser::new(&matrix_bytes, Origin::Buffer, Limits::default()).parse()?;
    let matrix = row(&row(&matrix)?[0])?;
    for (glyph, value) in glyphs.iter().zip(&expectations) {
        let row = row(value)?;
        assert_eq!(glyph.code, uint(&row[0])?, "{name}");
        assert_eq!(u32::from(glyph.byte_len), uint(&row[1])?);
        near(glyph.advance, number(&row[2])?);
        assert_eq!(Some(glyph.word_space), row[3].as_bool());
        assert_eq!(glyph.unicode.as_deref(), text(&row[4]), "{name}");
        let GlyphShape::Outline {
            program,
            glyph: id,
            matrix: actual,
        } = &glyph.shape
        else {
            return Err(Error::syntax("expected outline"));
        };
        assert_eq!(id.0, uint(&row[5])?, "{name}");
        for (actual, expected) in [actual.a, actual.b, actual.c, actual.d, actual.e, actual.f]
            .into_iter()
            .zip(matrix)
        {
            near(actual, number(expected)?);
        }
        program.outline(*id)?;
        if row[6].is_null() {
            assert!(glyph.vertical.is_none());
        } else {
            let vertical = glyph
                .vertical
                .ok_or_else(|| Error::syntax("missing vertical metric"))?;
            near(vertical.advance, number(&row[6])?);
            near(vertical.origin.0, number(&row[7])?);
            near(vertical.origin.1, number(&row[8])?);
        }
    }
    Ok(())
}

#[test]
fn embedded_program_vectors() -> Result<()> {
    for name in [
        "tt-mac-by-name",
        "tt-symbol-encoding-ignored",
        "tt-policy-name-mac",
        "tt-policy-name-full",
        "tt-policy-name-platform-zero",
        "tt-policy-full-before-platform-zero",
        "tt-policy-mac-before-full",
        "tt-policy-name-symbol",
        "tt-policy-raw-unicode",
        "tt-policy-raw-unicode-f000",
        "tt-policy-raw-full",
        "tt-policy-raw-platform-zero",
        "tt-policy-raw-bmp-before-full",
        "tt-policy-raw-unmapped",
        "tt-policy-multiscalar-unmapped",
        "tt-post-notdef-before-policy",
        "tt-policy-name-nonbmp",
        "tt-full-unicode-mac",
        "tt-platform-zero-mac",
        "tt-bmp-before-full",
        "tt-unicode-priority",
        "tt-unicode-missing-post",
        "tt-macroman-fallback",
        "tt-mac-euro",
        "tt-mac-currency-post",
        "tt-mac-extra-apple",
        "tt-mac-os-additions",
        "tt-post-without-cmap",
        "tt-post-differences",
        "tt-name-not-found",
        "tt-named-mac-undefined",
        "tt-dictionary-standard-fill",
        "tt-symbol-0000",
        "tt-symbol-f000",
        "tt-symbol-f100",
        "tt-symbol-f200",
        "tt-symbol-mac-only",
        "tt-symbol-present-missing",
        "tt-noencoding-direct",
        "tt-noencoding-symbol",
        "tt-units-per-em",
        "tt-tounicode-does-not-select",
        "embedded-cff",
        "embedded-opentype-cff",
        "embedded-opentype-truetype",
        "embedded-cid-cff",
        "embedded-cid-cff-gid",
        "embedded-type1",
        "embedded-type1-matrix",
        "embedded-cid-truetype-vertical",
        "embedded-cid-variable-codes",
        "embedded-cid-notdef",
    ] {
        check_program_vector(name)?;
    }
    Ok(())
}
