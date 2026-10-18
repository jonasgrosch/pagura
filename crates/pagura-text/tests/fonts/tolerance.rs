//! Independent strict/repair oracles for extraction faults and font tolerance.
use crate::support::{check_glyphs, dictionary, lines, open, rejected, row, uint, values};
use pagura_core::{Error, ErrorKind, Limits, Recovery, Result};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;
use pagura_text::{PdfFont, repairs};

fn tags(name: &str) -> Result<Vec<String>> {
    lines(name, "repair")
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
        let error = rejected(PdfFont::load(&strict, dictionary(&strict)?))?;
        check_strict_rejection(&strict, &error, name)?;
        // A cached nested parser failure must keep its original tag and context.
        let repeated = rejected(PdfFont::load(&strict, dictionary(&strict)?))?;
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
        let error = rejected(PdfFont::load(&strict, dictionary(&strict)?))?;
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
    let error = rejected(PdfFont::load(&document, dictionary(&document)?))?;
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
    let expected = values(name, "out")?;
    let row = row(&expected[0])?;
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

#[test]
fn simple_font_dictionary_faults_are_reported_repairs() -> Result<()> {
    for (name, tag) in [
        ("repair-widths-short", repairs::WIDTHS_LENGTH),
        ("repair-widths-long", repairs::WIDTHS_LENGTH),
        ("repair-widths-reversed", repairs::WIDTHS_LENGTH),
        ("repair-encoding-name", repairs::ENCODING_NAME),
        ("repair-encoding-base", repairs::ENCODING_NAME),
    ] {
        let strict = open(name, Recovery::Strict)?;
        let error = rejected(PdfFont::load(&strict, dictionary(&strict)?))?;
        assert_eq!(error.repair, Some(tag), "{name}: {error}");
        assert!(strict.diagnostics().is_empty(), "{name}");
        let repaired = open(name, Recovery::Repair)?;
        let font = PdfFont::load(&repaired, dictionary(&repaired)?)?;
        check_glyphs(&font, name, "out")?;
        assert_eq!(tags(name)?, [tag.as_str()], "{name}");
        assert!(
            repaired
                .diagnostics()
                .iter()
                .any(|item| item.repair() == Some(tag)),
            "{name}"
        );
    }
    Ok(())
}
