//! A checked CID fallback must identify every used glyph from source Unicode.
use crate::support::{dictionary, near, number, open, rejected, row, text, uint, values};
use pagura_core::{Error, ErrorKind, Limits, Name, Position, Recovery, Result};
use pagura_font::{FontFormat, FontProgram};
use pagura_testkit::fixture;
use pagura_text::{GlyphShape, PdfFont, Provider, StandardFont, StandardMetrics, information};
use std::sync::Arc;

/// No collection substitute: Adobe-Identity is not a Unicode collection.
struct Latin;

impl Provider for Latin {
    fn cid_font(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<FontProgram>>> {
        Ok(None)
    }
    fn predefined_cmap(&self, _: &[u8]) -> Result<Option<pagura_text::PredefinedCMap>> {
        Ok(None)
    }
    fn cid_to_unicode(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<pagura_text::CidToUnicode>>> {
        Ok(None)
    }
    fn standard_font(&self, name: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        let bytes: &[u8] = match name {
            b"Times-Roman" => include_bytes!("../../../../data/fonts/Tinos-Regular.ttf"),
            b"Times-Bold" => include_bytes!("../../../../data/fonts/Tinos-Bold.ttf"),
            b"Times-Italic" => include_bytes!("../../../../data/fonts/Tinos-Italic.ttf"),
            b"Times-BoldItalic" => include_bytes!("../../../../data/fonts/Tinos-BoldItalic.ttf"),
            _ => return Ok(None),
        };
        Ok(Some(Arc::new(StandardFont::new(
            Arc::new(StandardMetrics::new(Name::new(name))),
            Some(Arc::new(FontProgram::parse(
                bytes,
                FontFormat::TrueType,
                &Limits::default(),
            )?)),
        ))))
    }
}

#[test]
fn checked_cid_glyph_ids_and_pdf_advances_are_independent() -> Result<()> {
    for style in ["regular", "bold", "italic", "bolditalic"] {
        for subtype in ["CIDFontType0", "CIDFontType2"] {
            check_glyphs(&format!("checked-cid-{style}-{subtype}"))?;
        }
    }
    for name in ["checked-cid-identity", "checked-cid-unnamed"] {
        check_glyphs(name)?;
    }
    Ok(())
}

fn check_glyphs(name: &str) -> Result<()> {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let document = open(name, recovery)?;
        let dict = dictionary(&document)?;
        let font = PdfFont::load_with_provider(&document, dict, Some(&Latin))?;
        let unicode = font.unicode(&document, dict, Some(&Latin))?;
        let input = fixture("text", &format!("{name}.input"));
        // Rendering does not ask for extraction text. Both paths must select
        // the same nonzero glyph, including a valid empty-outline space.
        for text_map in [None, Some(&unicode)] {
            let actual = font.decode(&input, text_map)?;
            let expected = values(name, "out")?;
            assert_eq!(actual.len(), expected.len(), "{name}");
            for (glyph, expected) in actual.iter().zip(&expected) {
                let expected = row(expected)?;
                assert_eq!(glyph.code, uint(&expected[0])?, "{name}");
                assert_eq!(glyph.byte_len, 2);
                near(glyph.advance, number(&expected[2])? / 1000.0);
                assert_eq!(
                    glyph.unicode.as_deref(),
                    text_map.and_then(|_| text(&expected[3]))
                );
                let GlyphShape::Outline {
                    program, glyph: id, ..
                } = &glyph.shape
                else {
                    return Err(Error::syntax(
                        "checked CID glyph must have a selected outline",
                    ));
                };
                assert_eq!(id.0, uint(&expected[4])?, "{name}");
                assert_ne!(id.0, 0);
                program.outline(*id)?;
            }
        }
        let diagnostics = document.diagnostics();
        assert_eq!(diagnostics.len(), 1, "{name}");
        assert_eq!(
            diagnostics[0].kind,
            pagura_core::DiagnosticKind::Information(information::SUBSTITUTED)
        );
        assert!(
            document
                .diagnostics()
                .iter()
                .all(|d| !matches!(d.kind, pagura_core::DiagnosticKind::Repair(_)))
        );
    }
    Ok(())
}

#[test]
fn checked_cid_refuses_incomplete_identity_without_extraction() -> Result<()> {
    for fault in [
        "no-unicode",
        "unmapped-unicode",
        "ligature",
        "empty-unicode",
        "missing-glyph",
        "encoding-hole",
        "notdef-cid",
        "unknown-style",
        "unknown-family",
        "invalid-subset",
        "punctuation-name",
        "case-name",
        "vertical",
        "simple-unchanged",
    ] {
        let name = format!("checked-cid-{fault}");
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = open(&name, recovery)?;
            let dict = dictionary(&document)?;
            let input = fixture("text", &format!("{name}.input"));
            let error = rejected(
                PdfFont::load_with_provider(&document, dict, Some(&Latin))
                    .and_then(|font| font.decode(&input, None)),
            )?;
            let ErrorKind::Unsupported(feature) = error.kind else {
                return Err(Error::syntax("expected explicit unsupported coverage"));
            };
            assert_eq!(
                feature.as_str().as_bytes(),
                fixture("text", &format!("{name}.error")).trim_ascii()
            );
            if [
                "unmapped-unicode",
                "ligature",
                "empty-unicode",
                "missing-glyph",
                "encoding-hole",
                "notdef-cid",
            ]
            .contains(&fault)
            {
                assert_eq!(error.position, Position::Buffer(4), "{name}");
            }
            assert!(
                document
                    .diagnostics()
                    .iter()
                    .all(|d| !matches!(d.kind, pagura_core::DiagnosticKind::Repair(_)))
            );
        }
    }
    Ok(())
}

#[test]
fn checked_cid_malformed_unicode_keeps_strict_refusal() -> Result<()> {
    for (name, strict_repair) in [
        (
            "checked-cid-malformed-unicode",
            pagura_text::repairs::TOUNICODE_UTF16,
        ),
        (
            "checked-cid-cid-is-not-unicode",
            pagura_text::repairs::TOUNICODE_INVALID,
        ),
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = open(name, recovery)?;
            let result =
                PdfFont::load_with_provider(&document, dictionary(&document)?, Some(&Latin))
                    .and_then(|font| font.decode(&fixture("text", &format!("{name}.input")), None));
            let error = rejected(result)?;
            if recovery == Recovery::Strict {
                assert_eq!(error.kind, ErrorKind::Syntax);
                assert_eq!(error.repair, Some(strict_repair));
                assert_eq!(document.diagnostics(), []);
            } else {
                assert_eq!(
                    error.kind,
                    ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED)
                );
                assert!(
                    document
                        .diagnostics()
                        .iter()
                        .any(|d| d.kind == pagura_core::DiagnosticKind::Repair(strict_repair))
                );
                if strict_repair == pagura_text::repairs::TOUNICODE_UTF16 {
                    assert_eq!(error.position, Position::Buffer(4));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn checked_cid_requires_provider_outlines_and_retains_existing_limits() -> Result<()> {
    let name = "checked-cid-regular-CIDFontType0";
    let bytes = fixture("text", &format!("{name}.pdf"));
    let document = open(name, Recovery::Strict)?;
    let error = rejected(PdfFont::load(&document, dictionary(&document)?))?;
    assert!(matches!(error.kind, ErrorKind::Unsupported(_)));
    for limits in [
        Limits {
            font_work: 0,
            ..Limits::default()
        },
        Limits {
            text_items: 0,
            ..Limits::default()
        },
        Limits {
            string_bytes: 1,
            ..Limits::default()
        },
    ] {
        let result = pagura_document::Document::open(
            bytes.clone(),
            &pagura_document::OpenOptions {
                recovery: Recovery::Strict,
                limits,
                ..Default::default()
            },
        )
        .and_then(|document| {
            PdfFont::load_with_provider(&document, dictionary(&document)?, Some(&Latin))?
                .decode(&fixture("text", &format!("{name}.input")), None)
        });
        let error = rejected(result)?;
        assert_eq!(error.kind, ErrorKind::Limit);
    }
    Ok(())
}
