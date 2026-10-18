//! Independent locked-data glyph oracles and strict/repaired descriptor results.
use super::*;
use pagura_core::{Error, Limits, ObjectId, Result};
use pagura_core::{
    Name, Recovery, Value,
    syntax::{Origin, Parser},
};
use pagura_document::Document;
use pagura_document::OpenOptions;
use pagura_font::FontProgram;
use pagura_testkit::fixture;
use std::collections::BTreeMap;
use std::sync::Arc;

struct Outlines;

impl Provider for Outlines {
    fn standard_font(&self, base: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        let bytes: &[u8] = match base {
            b"Helvetica" => include_bytes!("../../../data/fonts/Arimo-Regular.ttf"),
            b"Helvetica-Bold" => include_bytes!("../../../data/fonts/Arimo-Bold.ttf"),
            b"Helvetica-Oblique" => include_bytes!("../../../data/fonts/Arimo-Italic.ttf"),
            b"Times-Roman" => include_bytes!("../../../data/fonts/Tinos-Regular.ttf"),
            b"Times-Italic" => include_bytes!("../../../data/fonts/Tinos-Italic.ttf"),
            b"Symbol" => include_bytes!("../../../data/fonts/FoxitSymbol.cff"),
            b"ZapfDingbats" => include_bytes!("../../../data/fonts/FoxitDingbats.cff"),
            _ => return Ok(None),
        };
        let symbolic = matches!(base, b"Symbol" | b"ZapfDingbats");
        let format = if symbolic {
            pagura_font::FontFormat::Cff
        } else {
            pagura_font::FontFormat::TrueType
        };
        let program = FontProgram::parse(bytes, format, &Limits::default())?;
        // Resources use locked data, never the fixtures' expected glyphs.
        let widths = match base {
            b"Symbol" => afm_widths(include_str!("../../../data/afm/Symbol.afm"))?,
            b"ZapfDingbats" => afm_widths(include_str!("../../../data/afm/ZapfDingbats.afm"))?,
            _ => BTreeMap::new(),
        };
        Ok(Some(Arc::new(StandardFont {
            metrics: Arc::new(StandardMetrics {
                name: Name::new(base),
                encoding: encoding::BaseEncoding::from_table_name(if symbolic {
                    base
                } else {
                    b"StandardEncoding"
                })
                .ok_or_else(|| Error::syntax("unknown base encoding"))?
                .names(),
                widths,
                descriptor: StandardDescriptor::default(),
            }),
            program: Some(Arc::new(program)),
        })))
    }
    fn predefined_cmap(&self, _: &[u8]) -> Result<Option<PredefinedCMap>> {
        Ok(None)
    }
    fn cid_font(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<FontProgram>>> {
        Ok(self
            .standard_font(b"Helvetica")?
            .and_then(|font| font.program.clone()))
    }
    fn cid_to_unicode(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<CidToUnicode>>> {
        Ok(Some(Arc::new(CidToUnicode::new(
            [(0, " ".to_owned()), (1, "A".to_owned())]
                .into_iter()
                .collect(),
            &Limits::default(),
        )?)))
    }
}

fn afm_widths(afm: &str) -> Result<BTreeMap<Name, f64>> {
    afm.lines()
        .filter(|line| line.starts_with("C "))
        .map(|line| {
            let mut fields = line.split(';').map(str::trim);
            let width = fields
                .clone()
                .find_map(|field| field.strip_prefix("WX "))
                .and_then(|width| width.parse().ok())
                .ok_or_else(|| Error::syntax("invalid AFM test resource width"))?;
            let name = fields
                .find_map(|field| field.strip_prefix("N "))
                .ok_or_else(|| Error::syntax("invalid AFM test resource name"))?;
            Ok((Name::new(name.as_bytes()), width))
        })
        .collect()
}

fn values(name: &str, suffix: &str) -> Result<Vec<Value>> {
    let bytes = fixture("text", &format!("{name}.{suffix}"));
    Parser::new(&bytes, Origin::Buffer, Limits::default())
        .parse()?
        .as_array()
        .map(<[Value]>::to_vec)
        .ok_or_else(|| Error::syntax("expected oracle array"))
}

#[test]
fn invalid_optional_weights_have_exact_repaired_glyphs_and_strict_tags() -> Result<()> {
    for value in values("substitution-recovery-inventory", "out")? {
        let name = value
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_string)
            .and_then(|s| std::str::from_utf8(s).ok())
            .ok_or_else(|| Error::syntax("invalid fixture name"))?;
        let tag = fixture("text", &format!("{name}.tag"));
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("text", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..Default::default()
                },
            )?;
            let dict = document
                .object(ObjectId::new(4, 0))?
                .dictionary()
                .ok_or_else(|| Error::syntax("missing font"))?;
            let result = PdfFont::load_with_provider(&document, dict, Some(&Outlines));
            if recovery == Recovery::Strict && !tag.is_empty() {
                let error = result.unwrap_err();
                assert_eq!(
                    error.repair.map(|r| r.as_str().as_bytes()),
                    Some(tag.as_slice())
                );
                assert_eq!(error.object, Some(ObjectId::new(5, 0)));
                assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
                continue;
            }
            let font = result?;
            check_glyphs(name, &font)?;
            let kinds: Vec<_> = document.diagnostics().iter().map(|d| d.kind).collect();
            assert_eq!(
                kinds,
                if tag.is_empty() {
                    vec![pagura_core::DiagnosticKind::Information(
                        information::SUBSTITUTED,
                    )]
                } else {
                    vec![
                        pagura_core::DiagnosticKind::Repair(repairs::DESCRIPTOR_WEIGHT),
                        pagura_core::DiagnosticKind::Information(information::SUBSTITUTED),
                    ]
                }
            );
        }
    }
    Ok(())
}

fn check_glyphs(name: &str, font: &PdfFont) -> Result<()> {
    let glyphs = font.decode(&fixture("text", &format!("{name}.input")))?;
    let expected = values(name, "out")?;
    assert_eq!(glyphs.len(), expected.len());
    let matrices = values(name, "matrix")?;
    let matrix_expected = matrices[0]
        .as_array()
        .ok_or_else(|| Error::syntax("expected matrix"))?;
    let resource = Outlines
        .standard_font(&fixture("text", &format!("{name}.base")))?
        .ok_or_else(|| Error::syntax("expected substitute"))?;
    let canonical = resource
        .program
        .as_ref()
        .ok_or_else(|| Error::syntax("missing expected program"))?;
    for (glyph, row) in glyphs.iter().zip(expected) {
        let row = row
            .as_array()
            .ok_or_else(|| Error::syntax("expected glyph row"))?;
        assert_eq!(Some(i64::from(glyph.code)), row[0].as_integer());
        assert!(
            (glyph.advance
                - row[1]
                    .as_number()
                    .ok_or_else(|| Error::syntax("expected width"))?)
            .abs()
                < 1e-12
        );
        assert_eq!(
            glyph.unicode.as_deref().map(str::as_bytes),
            row[2].as_string()
        );
        let GlyphShape::Outline {
            glyph: id,
            matrix,
            program,
        } = &glyph.shape
        else {
            return Err(Error::syntax("missing outline"));
        };
        assert_eq!(Some(i64::from(id.0)), row[3].as_integer());
        assert_eq!(program.data(), canonical.data(), "{name}");
        for (actual, expected) in [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f]
            .iter()
            .zip(matrix_expected)
        {
            assert_eq!(Some(*actual), expected.as_number());
        }
    }
    Ok(())
}

#[test]
fn descriptor_styles_and_symbol_aliases_preserve_pdf_positions_in_both_modes() -> Result<()> {
    for value in values("substitute-style-inventory", "out")? {
        let name = value
            .as_array()
            .and_then(|row| row.first())
            .and_then(Value::as_string)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .ok_or_else(|| Error::syntax("invalid fixture name"))?;
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("text", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..Default::default()
                },
            )?;
            let dictionary = document
                .object(ObjectId::new(4, 0))?
                .dictionary()
                .ok_or_else(|| Error::syntax("missing font"))?;
            let font = PdfFont::load_with_provider(&document, dictionary, Some(&Outlines))?;
            check_glyphs(name, &font)?;
            let glyphs = font.decode(&fixture("text", &format!("{name}.input")))?;
            let mut position = 0.0;
            for (glyph, expected) in glyphs.iter().zip(values(name, "positions")?) {
                assert!((position - expected.as_number().unwrap()).abs() < 1e-12);
                position += glyph.advance;
            }
            assert_eq!(document.diagnostics().len(), 1, "{name}");
            let diagnostic = &document.diagnostics()[0];
            assert_eq!(
                diagnostic.kind,
                pagura_core::DiagnosticKind::Information(information::SUBSTITUTED)
            );
            let base = fixture("text", &format!("{name}.base"));
            assert_eq!(
                diagnostic.message,
                format!("substitute outlines for {}", String::from_utf8_lossy(&base))
            );
        }
    }
    Ok(())
}

#[test]
fn cyrillic_names_agree_with_all_sixty_six_independent_records() -> Result<()> {
    for value in values("glyph-name-cyrillic", "out")? {
        let row = value
            .as_array()
            .ok_or_else(|| Error::syntax("expected name row"))?;
        let name = row[0]
            .as_string()
            .ok_or_else(|| Error::syntax("expected glyph name"))?;
        assert_eq!(
            encoding::unicode(name).as_deref().map(str::as_bytes),
            row[1].as_string()
        );
    }
    for name in [
        b"afii010017".as_slice(),
        b"afii+0017",
        b"afii10016",
        b"afii10050",
        b"afii10098",
    ] {
        assert!(encoding::unicode(name).is_none());
    }
    Ok(())
}

#[test]
fn substituted_fonts_reject_malformed_unicode_in_strict_mode() -> Result<()> {
    for name in ["strict-substitution-simple", "strict-substitution-cid"] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("text", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )?;
            let dictionary = document
                .object(ObjectId::new(4, 0))?
                .dictionary()
                .ok_or_else(|| Error::syntax("missing font"))?;
            let result = PdfFont::load_with_provider(&document, dictionary, Some(&Outlines));
            if recovery == Recovery::Strict {
                let error = result.unwrap_err();
                let tag = fixture("text", &format!("{name}.strict.error"));
                assert_eq!(
                    error.repair.map(pagura_core::Repair::as_str),
                    Some(
                        std::str::from_utf8(&tag)
                            .map_err(|_| Error::syntax("invalid tag"))?
                            .trim()
                    )
                );
                assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
                continue;
            }
            let font = result?;
            let glyphs = font.decode(&fixture("text", &format!("{name}.input")))?;
            let expected = values(name, "out")?;
            assert_eq!(glyphs.len(), expected.len());
            for (glyph, row) in glyphs.iter().zip(expected) {
                let row = row
                    .as_array()
                    .ok_or_else(|| Error::syntax("invalid glyph oracle"))?;
                assert_eq!(Some(i64::from(glyph.code)), row[0].as_integer());
                assert_eq!(Some(glyph.advance), row[1].as_number());
                assert_eq!(
                    glyph.unicode.as_deref().map(str::as_bytes),
                    row[2].as_string()
                );
                assert!(matches!(glyph.shape, GlyphShape::Outline { .. }));
            }
            let diagnostics = document.diagnostics();
            assert_eq!(
                diagnostics.iter().map(|item| item.kind).collect::<Vec<_>>(),
                [
                    pagura_core::DiagnosticKind::Repair(repairs::TOUNICODE_UTF16),
                    pagura_core::DiagnosticKind::Information(information::SUBSTITUTED)
                ]
            );
        }
    }
    Ok(())
}

#[test]
fn null_or_dangling_font_files_still_use_the_provider() -> Result<()> {
    // A FontFile entry that is null or refers to a missing object embeds
    // nothing (7.3.9, 7.3.10), so the provider's substitute applies.
    let name = "substitute-style-bold-weight";
    let document = Document::open(
        fixture("text", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )?;
    let original = document
        .object(ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("missing font"))?;
    for value in [Value::Null, Value::Reference(ObjectId::new(99, 0))] {
        let mut descriptor = document
            .object(ObjectId::new(5, 0))?
            .dictionary()
            .ok_or_else(|| Error::syntax("missing descriptor"))?
            .clone();
        descriptor.insert(Name::new(b"FontFile2"), value);
        let mut font = original.clone();
        font.insert(Name::new(b"FontDescriptor"), Value::Dictionary(descriptor));
        let substituted = PdfFont::load_with_provider(&document, &font, Some(&Outlines))?;
        check_glyphs(name, &substituted)?;
        let error = PdfFont::load(&document, &font)
            .err()
            .ok_or_else(|| Error::syntax("expected a missing program"))?;
        assert_eq!(error.feature(), Some(features::NON_EMBEDDED));
    }
    Ok(())
}
