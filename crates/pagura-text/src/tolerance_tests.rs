//! Independent strict/repair oracles for extraction faults and font tolerance.
use super::*;
use pagura_core::{
    ErrorKind, Recovery, Value,
    syntax::{Origin, Parser},
};
use pagura_document::OpenOptions;
use pagura_testkit::fixture;

fn open(name: &str, recovery: Recovery) -> Result<Document> {
    Document::open(
        fixture("text", &format!("{name}.pdf")),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )
}
fn dictionary(document: &Document) -> Result<&Dictionary> {
    document
        .object(ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("missing fixture font"))
}
fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("expected oracle array"))
}
fn number(value: &Value) -> Result<f64> {
    value
        .as_number()
        .ok_or_else(|| Error::syntax("expected oracle number"))
}
fn uint(value: &Value) -> Result<u32> {
    value
        .as_integer()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Error::syntax("expected oracle integer"))
}
fn parse(name: &str, suffix: &str) -> Result<Value> {
    let bytes = fixture("text", &format!("{name}.{suffix}"));
    Parser::new(&bytes, Origin::Buffer, Limits::default()).parse()
}
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}
fn tags(name: &str) -> Result<Vec<String>> {
    let bytes = fixture("text", &format!("{name}.repair"));
    Ok(std::str::from_utf8(&bytes)
        .map_err(|_| Error::syntax("invalid tag oracle"))?
        .lines()
        .map(str::to_owned)
        .collect())
}
fn check_glyphs(font: &PdfFont, name: &str, suffix: &str) -> Result<()> {
    let glyphs = font.decode(&fixture("text", &format!("{name}.input")))?;
    let expected = parse(name, suffix)?;
    assert_eq!(glyphs.len(), array(&expected)?.len(), "{name}");
    for (glyph, row) in glyphs.iter().zip(array(&expected)?) {
        let row = array(row)?;
        assert_eq!(glyph.code, uint(&row[0])?, "{name}");
        assert_eq!(u32::from(glyph.byte_len), uint(&row[1])?, "{name}");
        near(glyph.advance, number(&row[2])?);
        assert_eq!(Some(glyph.word_space), row[3].as_bool());
        assert_eq!(
            glyph.unicode.as_deref().map(str::as_bytes),
            row[4].as_string(),
            "{name}: {}",
            glyph.code
        );
        assert!(glyph.vertical.is_none());
        if row[5].is_null() {
            assert!(matches!(glyph.shape, GlyphShape::Blank));
        } else {
            let GlyphShape::Outline {
                program,
                glyph: id,
                matrix,
            } = &glyph.shape
            else {
                return Err(Error::syntax("expected drawable outline"));
            };
            assert_eq!(id.0, uint(&row[5])?, "{name}");
            let values = parse(name, "matrix")?;
            for (actual, expected) in [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f]
                .into_iter()
                .zip(array(&array(&values)?[0])?)
            {
                near(actual, number(expected)?);
            }
            program.outline(*id)?;
        }
    }
    Ok(())
}
fn check_strict_rejection(document: &Document, error: &Error, name: &str) -> Result<()> {
    let expected = fixture("text", &format!("{name}.strict.error"));
    let expected = std::str::from_utf8(&expected)
        .map_err(|_| Error::syntax("invalid strict tag oracle"))?
        .trim();
    if name.starts_with("unicode-") {
        let bytes = fixture("text", &format!("{name}.error"));
        let kind = std::str::from_utf8(&bytes)
            .map_err(|_| Error::syntax("invalid error oracle"))?
            .trim();
        assert_eq!(
            error
                .feature()
                .map_or(error.kind.name(), pagura_core::Feature::as_str),
            kind,
            "{name}"
        );
    }
    assert_eq!(
        error.repair.map(pagura_core::Repair::as_str),
        Some(expected),
        "{name}: {error}"
    );
    assert!(error.object.is_some() || name == "repair-tounicode-object");
    assert!(document.diagnostics().is_empty(), "{name}");
    Ok(())
}

#[test]
fn extraction_repairs_reject_font_loading_in_strict_mode() -> Result<()> {
    for name in [
        "repair-tounicode-simple",
        "repair-tounicode-mixed-sources",
        "repair-tounicode-alias-conflict",
        "repair-tounicode-utf16",
        "repair-tounicode-range",
        "repair-tounicode-range-end",
        "repair-tounicode-range-surrogates",
        "repair-tounicode-parent-invalid",
        "repair-tounicode-block-count",
        "repair-tounicode-cycle",
        "repair-tounicode-unsupported",
        "repair-tounicode-truncated",
        "repair-tounicode-limit",
        "repair-tounicode-object",
        "repair-tounicode-codespace",
        "repair-tounicode-checksum",
        "repair-tounicode-key-solidus",
        "unicode-surrogate",
        "unicode-cycle",
        "unicode-range-overflow",
        "unicode-odd-destination",
        "unicode-two-byte",
        "unicode-prefix-overlap",
        "unicode-range-array-length",
        "unicode-block-count",
        "unicode-usecmap-mismatch",
        "unicode-cid-code-length",
        "unicode-simple-source-length",
    ] {
        let strict = open(name, Recovery::Strict)?;
        let error = PdfFont::load(&strict, dictionary(&strict)?).unwrap_err();
        check_strict_rejection(&strict, &error, name)?;
        // A cached nested parser failure must keep its original tag and context.
        let repeated = PdfFont::load(&strict, dictionary(&strict)?).unwrap_err();
        assert_eq!(error, repeated, "{name}");
        assert!(strict.diagnostics().is_empty(), "{name}");
        let repaired = open(name, Recovery::Repair)?;
        let font = PdfFont::load(&repaired, dictionary(&repaired)?)?;
        check_glyphs(&font, name, "out")?;
        for tag in tags(name)? {
            assert!(
                repaired.diagnostics().iter().any(|item| item.tag() == tag),
                "{name}: {tag}"
            );
        }
    }
    Ok(())
}

#[test]
fn painting_critical_repairs_require_permission_and_preserve_exact_glyphs() -> Result<()> {
    for name in [
        "repair-cmap-block-count",
        "repair-cmap-codespace",
        "repair-cmap-codespace-wide",
        "repair-cid-to-gid-identity",
    ] {
        let strict = open(name, Recovery::Strict)?;
        let error = PdfFont::load(&strict, dictionary(&strict)?).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        check_strict_rejection(&strict, &error, name)?;
        let repaired = open(name, Recovery::Repair)?;
        let font = PdfFont::load(&repaired, dictionary(&repaired)?)?;
        check_glyphs(&font, name, "out")?;
        assert!(repaired.diagnostics().iter().any(|item| {
            Some(item.tag())
                == tags(name)
                    .ok()
                    .and_then(|tags| tags.first().cloned())
                    .as_deref()
        }));
    }
    Ok(())
}

#[test]
fn oversized_cmap_blocks_remain_bounded() -> Result<()> {
    let document = Document::open(
        fixture("text", "repair-cmap-block-count.pdf"),
        &OpenOptions {
            limits: Limits {
                value_nodes: 100,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )?;
    let error = PdfFont::load(&document, dictionary(&document)?).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    Ok(())
}

#[test]
fn repeated_unicode_faults_use_bounded_document_diagnostics() -> Result<()> {
    let name = "repair-tounicode-diagnostic-cap";
    let document = open(name, Recovery::Repair)?;
    let font = PdfFont::load(&document, dictionary(&document)?)?;
    let glyphs = font.decode(b"A")?;
    assert_eq!(glyphs[0].unicode.as_deref(), Some("A"));
    let expected = parse(name, "out")?;
    let row = array(&array(&expected)?[0])?;
    assert_eq!(
        document.diagnostics().len(),
        usize::try_from(uint(&row[0])?).map_err(|_| Error::syntax("invalid cap oracle"))?
    );
    assert_eq!(
        document.suppressed_diagnostics(),
        usize::try_from(uint(&row[1])?).map_err(|_| Error::syntax("invalid suppression oracle"))?
    );
    Ok(())
}

#[test]
fn null_unicode_entries_are_absent_without_repairs() -> Result<()> {
    for name in [
        "tounicode-null-direct",
        "tounicode-null-indirect",
        "tounicode-null-undefined",
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = open(name, recovery)?;
            let font = PdfFont::load(&document, dictionary(&document)?)?;
            check_glyphs(&font, name, "out")?;
            assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
        }
    }
    Ok(())
}
