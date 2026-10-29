//! Bundled font data through documents: `pagura_data::Bundled` as the
//! provider of `pagura_text::PdfFont`, checked against the oracles that
//! `tools/fixturegen/text.py` writes independently.
#![forbid(unsafe_code)]
#![cfg(feature = "bundled-data")]
use pagura::{
    Diagnostic, DiagnosticKind, Dictionary, Document, Error, ErrorKind, ObjectId, OpenOptions,
    Result,
};
use pagura_core::{
    Limits, Value,
    syntax::{Origin, Parser},
};
use pagura_data::Bundled;
use pagura_testkit::fixture;
use pagura_text::{GlyphShape, PdfFont, Provider, features};
use std::sync::Arc;

const NO_DIAGNOSTICS: [Diagnostic; 0] = [];

fn rows(name: &str, suffix: &str) -> Result<Vec<Value>> {
    let bytes = fixture("text", &format!("{name}.{suffix}"));
    match Parser::new(&bytes, Origin::Buffer, Limits::default()).parse()? {
        Value::Array(values) => Ok(values),
        _ => Err(Error::syntax("oracle must be an array")),
    }
}

fn row(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("oracle must be a row"))
}

fn number(value: &Value) -> Result<f64> {
    value
        .as_number()
        .ok_or_else(|| Error::syntax("oracle must be numeric"))
}

fn integer(value: &Value) -> Result<u32> {
    value
        .as_integer()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| Error::syntax("oracle must be an unsigned integer"))
}

fn text(value: &Value) -> Option<&str> {
    value
        .as_string()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
}

fn names(inventory: &str) -> Result<Vec<String>> {
    rows(inventory, "out")?
        .iter()
        .map(|value| {
            text(&row(value)?[0])
                .map(str::to_owned)
                .ok_or_else(|| Error::syntax("oracle name must be a string"))
        })
        .collect()
}

fn open(name: &str) -> Result<Document> {
    open_with(name, &OpenOptions::default())
}

fn open_with(name: &str, options: &OpenOptions) -> Result<Document> {
    Document::open(fixture("text", &format!("{name}.pdf")), options)
}

/// The font dictionary every bundled-data fixture places in object 4.
fn font(document: &Document) -> Result<&Dictionary> {
    document
        .object(ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("missing font dictionary"))
}

fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

fn information(document: &Document, tag: &'static str) -> bool {
    document
        .diagnostics()
        .iter()
        .any(|d| d.kind == DiagnosticKind::Information(tag))
}

#[test]
fn standard_fonts_use_afm_widths_and_substitute_outlines() -> Result<()> {
    let provider = Bundled::default();
    for name in names("bundled-standard-inventory")? {
        let base = fixture("text", &format!("{name}.base"));
        let resource = provider
            .standard_font(&base)?
            .ok_or_else(|| Error::syntax("missing standard font"))?;
        let document = open(&name)?;
        assert!(PdfFont::load(&document, font(&document)?).is_err());
        let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))?;
        let unicode = loaded.unicode(&document, font(&document)?, Some(&provider))?;
        for value in rows(&name, "out")? {
            let values = row(&value)?;
            near(loaded.width(integer(&values[0])?), number(&values[1])?);
        }
        let input = fixture("text", &format!("{name}.input"));
        if resource.program.is_none() {
            let error = loaded.decode(&input, Some(&unicode)).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Unsupported(features::NON_EMBEDDED));
            continue;
        }
        assert_eq!(document.diagnostics().len(), 1);
        assert!(information(
            &document,
            pagura_text::information::SUBSTITUTED
        ));
        let glyphs = loaded.decode(&input, Some(&unicode))?;
        for (glyph, value) in glyphs.iter().zip(rows(&name, "out")?) {
            let values = row(&value)?;
            assert_eq!(glyph.code, integer(&values[0])?);
            near(glyph.advance, number(&values[1])?);
            assert_eq!(glyph.unicode.as_deref(), text(&values[2]), "{name}");
            let GlyphShape::Outline {
                program,
                glyph,
                matrix,
            } = &glyph.shape
            else {
                return Err(Error::syntax("missing substitute outlines"));
            };
            assert_eq!(glyph.0, integer(&values[3])?, "{name}");
            assert_eq!(*matrix, program.font_matrix());
            program.outline(*glyph)?;
        }
    }
    Ok(())
}

#[test]
fn descriptor_substitution_retains_pdf_metrics_and_rejects_unsafe_symbols() -> Result<()> {
    let provider = Bundled::default();
    for name in names("bundled-substitute-inventory")? {
        let document = open(&name)?;
        let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))?;
        let unicode = loaded.unicode(&document, font(&document)?, Some(&provider))?;
        let glyphs = loaded.decode(b"A", Some(&unicode))?;
        let values = rows(&name, "out")?;
        let values = row(&values[0])?;
        near(glyphs[0].advance, number(&values[1])?);
        assert_eq!(glyphs[0].unicode.as_deref(), text(&values[2]));
        let GlyphShape::Outline { program, glyph, .. } = &glyphs[0].shape else {
            return Err(Error::syntax("missing substitute outline"));
        };
        assert_eq!(glyph.0, integer(&values[3])?);
        let base = fixture("text", &format!("{name}.base"));
        let selected = provider
            .standard_font(&base)?
            .ok_or_else(|| Error::syntax("missing selected font"))?;
        assert!(Arc::ptr_eq(
            program,
            selected
                .program
                .as_ref()
                .ok_or_else(|| Error::syntax("missing selected outline"))?
        ));
        assert_eq!(document.diagnostics().len(), 1);
    }
    for (name, contradictory) in [
        ("substitute-unsafe-symbolic", false),
        ("substitute-unsafe-symbolic-both", true),
        ("substitute-unsafe-symbolic-known-base", false),
    ] {
        let document = open(name)?;
        assert_eq!(
            PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))
                .unwrap_err()
                .kind,
            ErrorKind::Unsupported(features::NON_EMBEDDED)
        );
        // Both Symbolic and Nonsymbolic set is a reported descriptor repair
        // that keeps Symbolic (9.8.2); the symbolic font is still refused.
        let repairs: Vec<_> = document
            .diagnostics()
            .iter()
            .map(|d| (d.repair(), d.object))
            .collect();
        let expected = if contradictory {
            vec![(
                Some(pagura_text::repairs::DESCRIPTOR_FLAGS),
                Some(ObjectId::new(5, 0)),
            )]
        } else {
            Vec::new()
        };
        assert_eq!(repairs, expected, "{name}");
    }
    // A known Latin family with an explicit Latin encoding: the contradicting
    // Symbolic flag is a reported repair, and the encoding selects the glyph.
    for name in [
        "substitute-symbolic-latin-encoding",
        "substitute-symbolic-latin-base-encoding",
    ] {
        let strict = open_with(
            name,
            &OpenOptions {
                recovery: pagura::Recovery::Strict,
                ..OpenOptions::default()
            },
        )?;
        assert_eq!(
            PdfFont::load_with_provider(&strict, font(&strict)?, Some(&provider))
                .unwrap_err()
                .repair,
            Some(pagura_text::repairs::SYMBOLIC_LATIN_ENCODING),
            "{name}"
        );
        let document = open(name)?;
        let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))?;
        let unicode = loaded.unicode(&document, font(&document)?, Some(&provider))?;
        let glyphs = loaded.decode(b"A", Some(&unicode))?;
        let values = rows(name, "out")?;
        let values = row(&values[0])?;
        near(glyphs[0].advance, number(&values[1])?);
        assert_eq!(glyphs[0].unicode.as_deref(), text(&values[2]));
        let GlyphShape::Outline { glyph, .. } = &glyphs[0].shape else {
            return Err(Error::syntax("missing substitute outline"));
        };
        assert_eq!(glyph.0, integer(&values[3])?, "{name}");
        let repairs: Vec<_> = document
            .diagnostics()
            .iter()
            .filter_map(Diagnostic::repair)
            .collect();
        assert_eq!(
            repairs,
            [pagura_text::repairs::SYMBOLIC_LATIN_ENCODING],
            "{name}"
        );
        assert!(information(
            &document,
            pagura_text::information::SUBSTITUTED
        ));
    }
    Ok(())
}

#[test]
fn predefined_cmap_bindings_decode_embedded_glyphs() -> Result<()> {
    let provider = Bundled::default();
    for name in names("bundled-cmap-inventory")? {
        let cmap_name = fixture("text", &format!("{name}.cmap"));
        let cmap = provider
            .predefined_cmap(&cmap_name)?
            .ok_or_else(|| Error::syntax("missing predefined CMap"))?;
        let document = open(&name)?;
        let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))?;
        let unicode = loaded.unicode(&document, font(&document)?, Some(&provider))?;
        assert_eq!(loaded.writing_mode(), cmap.writing_mode());
        let glyphs = loaded.decode(&fixture("text", &format!("{name}.input")), Some(&unicode))?;
        let expected = rows(&name, "glyphs")?;
        assert_eq!(glyphs.len(), expected.len());
        for (glyph, value) in glyphs.iter().zip(expected) {
            let values = row(&value)?;
            assert_eq!(glyph.code, integer(&values[0])?, "{name}");
            assert_eq!(u32::from(glyph.byte_len), integer(&values[1])?);
            near(glyph.advance, number(&values[2])?);
            assert_eq!(Some(glyph.word_space), values[3].as_bool());
            assert_eq!(glyph.unicode.as_deref(), text(&values[4]), "{name}");
            let GlyphShape::Outline { glyph: id, .. } = glyph.shape else {
                return Err(Error::syntax("missing embedded outline"));
            };
            assert_eq!(id.0, integer(&values[5])?);
            if let Some(metrics) = glyph.vertical {
                near(metrics.advance, number(&values[6])?);
                near(metrics.origin.0, number(&values[7])?);
                near(metrics.origin.1, number(&values[8])?);
            }
        }
        assert_eq!(document.diagnostics(), NO_DIAGNOSTICS);
    }
    Ok(())
}

#[test]
fn embedded_programs_take_precedence_and_do_not_report_substitution() -> Result<()> {
    let provider = Bundled::default();
    let document = open("tt-unicode-priority")?;
    let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))?;
    let unicode = loaded.unicode(&document, font(&document)?, Some(&provider))?;
    let glyphs = loaded.decode(b"A", Some(&unicode))?;
    let GlyphShape::Outline { program, glyph, .. } = &glyphs[0].shape else {
        return Err(Error::syntax("missing embedded program"));
    };
    assert_eq!(program.glyph_count(), 5);
    assert_eq!(glyph.0, 1);
    assert_eq!(document.diagnostics(), NO_DIAGNOSTICS);
    // An embedded program that cannot be used is replaced by the substitute
    // for its name and descriptor, a reported repair; strict mode keeps the
    // program's error with that repair (ADR 0009).
    let strict = open_with(
        "binding-wrong-fontfile",
        &OpenOptions {
            recovery: pagura::Recovery::Strict,
            ..OpenOptions::default()
        },
    )?;
    let error = PdfFont::load_with_provider(&strict, font(&strict)?, Some(&provider)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.repair, Some(pagura_text::repairs::EMBEDDED_UNUSABLE));
    let document = open("binding-wrong-fontfile")?;
    let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))?;
    assert!(loaded.program().is_some());
    let repairs: Vec<_> = document
        .diagnostics()
        .iter()
        .filter_map(Diagnostic::repair)
        .collect();
    assert_eq!(repairs, [pagura_text::repairs::EMBEDDED_UNUSABLE]);
    assert!(information(
        &document,
        pagura_text::information::SUBSTITUTED
    ));
    // Without a provider nothing can replace it: the program's error stays.
    let document = open("binding-wrong-fontfile")?;
    assert_eq!(
        PdfFont::load(&document, font(&document)?).unwrap_err().kind,
        ErrorKind::Syntax
    );
    Ok(())
}

#[test]
fn symbol_substitutes_select_glyphs_by_afm_name_and_keep_pdf_widths() -> Result<()> {
    let provider = Bundled::default();
    for name in [
        "substitute-Symbol-charset",
        "substitute-ZapfDingbats-charset",
        "substitute-Symbol-differences",
        "substitute-Symbol-differences-widths",
        "substitute-ZapfDingbats-differences",
        "substitute-ZapfDingbats-differences-widths",
        "substitute-Symbol-truetype-symbolic",
        "substitute-Symbol-truetype-nonsymbolic",
    ] {
        let document = open(name)?;
        let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))?;
        let unicode = loaded.unicode(&document, font(&document)?, Some(&provider))?;
        let glyphs = loaded.decode(&fixture("text", &format!("{name}.input")), Some(&unicode))?;
        let rows = rows(name, "out")?;
        assert_eq!(glyphs.len(), rows.len());
        for (glyph, value) in glyphs.iter().zip(&rows) {
            let values = row(value)?;
            assert_eq!(glyph.code, integer(&values[0])?);
            near(glyph.advance, number(&values[2])? / 1000.0);
            let GlyphShape::Outline {
                program,
                glyph,
                matrix,
            } = &glyph.shape
            else {
                return Err(Error::syntax("missing symbol outline"));
            };
            assert_eq!(glyph.0, integer(&values[3])?, "{name}");
            let glyph_name = text(&values[1]).ok_or_else(|| Error::syntax("missing glyph name"))?;
            assert_eq!(program.glyph_for_name(glyph_name.as_bytes()), Some(*glyph));
            assert_eq!(*matrix, program.font_matrix());
            program.outline(*glyph)?;
        }
        assert_eq!(document.diagnostics().len(), 1);
        assert!(information(
            &document,
            pagura_text::information::SUBSTITUTED
        ));
        assert_eq!(
            document.diagnostics()[0].message,
            if name.contains("ZapfDingbats") {
                "substitute outlines for ZapfDingbats"
            } else {
                "substitute outlines for Symbol"
            }
        );
    }
    Ok(())
}

#[test]
fn latin_identity_cid_fonts_draw_through_tounicode_with_pdf_widths() -> Result<()> {
    let provider = Bundled::default();
    let program = provider
        .standard_font(b"Helvetica")?
        .and_then(|font| font.program.clone())
        .ok_or_else(|| Error::syntax("missing Helvetica substitute"))?;
    for name in [
        "substitute-cid-identity-latin",
        "substitute-cid-identity-latin-no-tounicode",
    ] {
        let document = open(name)?;
        let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))?;
        let input = fixture("text", &format!("{name}.input"));
        let unicode = loaded.unicode(&document, font(&document)?, Some(&provider))?;
        // Rendering decodes without the extraction Unicode; selection uses
        // the font's own ToUnicode map either way.
        for extraction in [None, Some(&unicode)] {
            let glyphs = loaded.decode(&input, extraction)?;
            let rows = rows(name, "out")?;
            assert_eq!(glyphs.len(), rows.len(), "{name}");
            for (glyph, value) in glyphs.iter().zip(rows) {
                let values = row(&value)?;
                assert_eq!(glyph.code, integer(&values[0])?);
                near(glyph.advance, number(&values[2])? / 1000.0);
                if extraction.is_some() {
                    assert_eq!(glyph.unicode.as_deref(), text(&values[3]), "{name}");
                }
                if values[4].is_null() {
                    assert!(matches!(glyph.shape, GlyphShape::Blank), "{name}");
                } else {
                    let GlyphShape::Outline {
                        program: selected,
                        glyph,
                        ..
                    } = &glyph.shape
                    else {
                        return Err(Error::syntax("missing Latin outline"));
                    };
                    assert!(Arc::ptr_eq(selected, &program));
                    assert_eq!(glyph.0, integer(&values[4])?);
                }
            }
        }
        assert!(information(
            &document,
            pagura_text::information::SUBSTITUTED
        ));
        assert!(information(
            &document,
            pagura_text::information::CID_GLYPH_MISSING
        ));
        assert!(
            document.diagnostics().iter().all(|d| d.repair().is_none()),
            "{name}"
        );
    }
    let document = open("substitute-cid-identity-latin-vertical")?;
    assert_eq!(
        PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::CID_SUBSTITUTE_VERTICAL)
    );
    Ok(())
}

#[cfg(not(feature = "cjk-fonts"))]
#[test]
fn cjk_programs_are_opt_in_and_missing_programs_fail_explicitly() -> Result<()> {
    let provider = Bundled::default();
    for ordering in ["Japan1", "GB1", "CNS1", "Korea1"] {
        let document = open(&format!("substitute-cid-{ordering}-CIDFontType2"))?;
        assert_eq!(
            PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))
                .unwrap_err()
                .kind,
            ErrorKind::Unsupported(features::NON_EMBEDDED)
        );
        assert_eq!(document.diagnostics(), NO_DIAGNOSTICS);
    }
    Ok(())
}

#[cfg(feature = "cjk-fonts")]
mod cjk {
    use super::{
        Bundled, NO_DIAGNOSTICS, PdfFont, Provider, font, information, integer, near, number, open,
        open_with, row, rows, text,
    };
    use pagura::{Error, ErrorKind, OpenOptions, Recovery, Result};
    use pagura_testkit::fixture;
    use pagura_text::{GlyphShape, features};
    use std::sync::Arc;

    #[test]
    fn collection_substitutes_match_regional_unicode_cmaps_and_pdf_metrics() -> Result<()> {
        let provider = Bundled::default();
        for ordering in ["Japan1", "GB1", "CNS1", "Korea1"] {
            for suffix in ["CIDFontType0", "CIDFontType2", "tounicode"] {
                check(&provider, &format!("substitute-cid-{ordering}-{suffix}"))?;
            }
        }
        for suffix in ["unicode", "glyph"] {
            check(&provider, &format!("substitute-cid-missing-{suffix}"))?;
        }
        Ok(())
    }

    fn check(provider: &Bundled, name: &str) -> Result<()> {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = open_with(
                name,
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )?;
            let loaded = PdfFont::load_with_provider(&document, font(&document)?, Some(provider))?;
            let unicode = loaded.unicode(&document, font(&document)?, Some(provider))?;
            let collection = fixture("text", &format!("{name}.collection"));
            let program = provider
                .cid_font(b"Adobe", &collection)?
                .ok_or_else(|| Error::syntax("missing CJK program"))?;
            let input = fixture("text", &format!("{name}.input"));
            let glyphs = loaded.decode(&input, Some(&unicode))?;
            let rows = rows(name, "out")?;
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
            assert!(information(
                &document,
                pagura_text::information::SUBSTITUTED
            ));
            if name.contains("missing") {
                assert!(information(
                    &document,
                    pagura_text::information::CID_GLYPH_MISSING
                ));
            }
            assert!(loaded.retained_bytes() < program.data().len());
            let before = loaded.retained_bytes();
            loaded.decode(&input, Some(&unicode))?;
            assert_eq!(loaded.retained_bytes(), before);
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
            let error = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))
                .unwrap_err();
            assert_eq!(
                error.kind,
                ErrorKind::Unsupported(features::CID_SUBSTITUTE_VERTICAL)
            );
            assert_eq!(document.diagnostics(), NO_DIAGNOSTICS);
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
            let error = PdfFont::load_with_provider(&document, font(&document)?, Some(&provider))
                .unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(document.diagnostics(), NO_DIAGNOSTICS);
        }
        Ok(())
    }
}
