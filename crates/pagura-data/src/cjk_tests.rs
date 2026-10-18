//! Collection data and OTF cmap oracles, independent of the font interpreter.
use super::tests::{dictionary, expected, integer, near, number, open, row, text};
use super::*;
use pagura_core::{Error, ErrorKind, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;
use pagura_text::{GlyphShape, PdfFont, features};

#[test]
fn collection_substitutes_match_regional_unicode_cmaps_and_pdf_metrics() -> Result<()> {
    let provider = Bundled::default();
    for ordering in ["Japan1", "GB1", "CNS1", "Korea1"] {
        for suffix in ["CIDFontType0", "CIDFontType2", "tounicode"] {
            check_font(&provider, &format!("substitute-cid-{ordering}-{suffix}"))?;
        }
    }
    for suffix in ["unicode", "glyph"] {
        check_font(&provider, &format!("substitute-cid-missing-{suffix}"))?;
    }
    Ok(())
}

fn check_font(provider: &Bundled, name: &str) -> Result<()> {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let document = Document::open(
            fixture("text", &format!("{name}.pdf")),
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )?;
        let font = PdfFont::load_with(&document, dictionary(&document)?, provider)?;
        let collection = fixture("text", &format!("{name}.collection"));
        let program = provider
            .cid_font(b"Adobe", &collection)?
            .ok_or_else(|| Error::syntax("missing CJK program"))?;
        assert!(Arc::ptr_eq(
            &program,
            &provider
                .cid_font(b"Adobe", &collection)?
                .ok_or_else(|| Error::syntax("missing cached CJK program"))?
        ));
        let glyphs = font.decode(&fixture("text", &format!("{name}.input")))?;
        let rows = expected(name, "out")?;
        assert_eq!(glyphs.len(), rows.len());
        for (glyph, value) in glyphs.iter().zip(rows) {
            let values = row(&value)?;
            assert_eq!(glyph.code, integer(&values[0])?);
            near(glyph.advance, number(&values[2])? / 1000.0);
            assert_eq!(glyph.unicode.as_deref(), text(&values[3]));
            if values[4].is_null() {
                assert!(matches!(glyph.shape, GlyphShape::Blank));
            } else {
                let GlyphShape::Outline {
                    program: selected,
                    glyph,
                    matrix,
                } = &glyph.shape
                else {
                    return Err(Error::syntax("missing CJK outline"));
                };
                assert!(Arc::ptr_eq(selected, &program));
                assert_eq!(glyph.0, integer(&values[4])?);
                assert_eq!(*matrix, program.font_matrix());
                program.outline(*glyph)?;
            }
        }
        assert!(document.diagnostics().iter().any(|d| d.kind
            == pagura_core::DiagnosticKind::Information(pagura_text::information::SUBSTITUTED)));
        if name.contains("missing") {
            assert!(document.diagnostics().iter().any(|d| d.kind
                == pagura_core::DiagnosticKind::Information(
                    pagura_text::information::CID_GLYPH_MISSING
                )));
        }
        assert!(font.retained_bytes() < program.data().len());
        let before = font.retained_bytes();
        font.decode(&fixture("text", &format!("{name}.input")))?;
        assert_eq!(font.retained_bytes(), before);
    }
    Ok(())
}

#[test]
fn vertical_substitutes_remain_explicit() -> Result<()> {
    let provider = Bundled::default();
    for name in [
        "substitute-cid-Japan1-vertical",
        "substitute-cid-GB1-vertical",
        "substitute-cid-CNS1-vertical",
        "substitute-cid-Korea1-vertical",
        "substitute-cid-wmode",
    ] {
        let document = open(name)?;
        let error = PdfFont::load_with(&document, dictionary(&document)?, &provider).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(features::CID_SUBSTITUTE_VERTICAL)
        );
        assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
    }
    Ok(())
}

#[test]
fn collection_mismatches_and_unknown_identity_fonts_are_not_guessed() -> Result<()> {
    let provider = Bundled::default();
    for (name, kind) in [
        ("substitute-cid-mismatched-collection", ErrorKind::Syntax),
        (
            "substitute-cid-identity-unknown",
            ErrorKind::Unsupported(features::NON_EMBEDDED),
        ),
    ] {
        let document = open(name)?;
        let error = PdfFont::load_with(&document, dictionary(&document)?, &provider).unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
    }
    Ok(())
}
