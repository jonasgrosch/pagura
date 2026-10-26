//! Locked-data substitutes: exact glyphs, strict tags and repaired
//! descriptor results for non-embedded fonts.
use crate::support::{dictionary, lines, load_text, open, rejected, values};
use pagura_core::{Error, Limits, Name, ObjectId, Recovery, Result, Value};
use pagura_document::{Document, OpenOptions};
use pagura_font::{FontFormat, FontProgram};
use pagura_testkit::fixture;
use pagura_text::{
    CidToUnicode, FontKind, FontUnicode, GlyphShape, PdfFont, PredefinedCMap, Provider,
    StandardFont, StandardMetrics, features, information, repairs,
};
use std::sync::Arc;

/// Standard fonts from the bundled outline files, with AFM metrics and
/// built-in encodings: locked data, never the fixtures' expected glyphs.
struct Outlines;

impl Provider for Outlines {
    fn standard_font(&self, base: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        let bytes: &[u8] = match base {
            b"Helvetica" => include_bytes!("../../../../data/fonts/Arimo-Regular.ttf"),
            b"Helvetica-Bold" => include_bytes!("../../../../data/fonts/Arimo-Bold.ttf"),
            b"Helvetica-Oblique" => include_bytes!("../../../../data/fonts/Arimo-Italic.ttf"),
            b"Times-Roman" => include_bytes!("../../../../data/fonts/Tinos-Regular.ttf"),
            b"Times-Italic" => include_bytes!("../../../../data/fonts/Tinos-Italic.ttf"),
            b"Symbol" => include_bytes!("../../../../data/fonts/FoxitSymbol.cff"),
            b"ZapfDingbats" => include_bytes!("../../../../data/fonts/FoxitDingbats.cff"),
            _ => return Ok(None),
        };
        let (format, afm) = match base {
            b"Symbol" => (
                FontFormat::Cff,
                include_str!("../../../../data/afm/Symbol.afm"),
            ),
            b"ZapfDingbats" => (
                FontFormat::Cff,
                include_str!("../../../../data/afm/ZapfDingbats.afm"),
            ),
            // The Latin AFM files encode StandardEncoding (9.6.2.2).
            _ => (
                FontFormat::TrueType,
                include_str!("../../../../data/afm/Helvetica.afm"),
            ),
        };
        let program = FontProgram::parse(bytes, format, &Limits::default())?;
        let mut metrics = StandardMetrics::new(Name::new(base));
        for (code, name, width) in afm_characters(afm)? {
            if let Some(code) = code {
                metrics.encoding[usize::from(code)] = Some(name.clone());
            }
            // Latin substitutes keep the PDF's own widths.
            if format == FontFormat::Cff {
                metrics.widths.insert(name, width);
            }
        }
        Ok(Some(Arc::new(StandardFont::new(
            Arc::new(metrics),
            Some(Arc::new(program)),
        ))))
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

/// The `C code ; WX width ; N name` records of an AFM file; code -1 is
/// unencoded.
fn afm_characters(afm: &str) -> Result<Vec<(Option<u8>, Name, f64)>> {
    afm.lines()
        .filter(|line| line.starts_with("C "))
        .map(|line| {
            let fields: Vec<&str> = line.split(';').map(str::trim).collect();
            let field = |prefix: &str| fields.iter().find_map(|f| f.strip_prefix(prefix));
            let code = field("C ")
                .and_then(|code| code.parse::<i32>().ok())
                .ok_or_else(|| Error::syntax("invalid AFM test resource code"))?;
            let width = field("WX ")
                .and_then(|width| width.parse().ok())
                .ok_or_else(|| Error::syntax("invalid AFM test resource width"))?;
            let name =
                field("N ").ok_or_else(|| Error::syntax("invalid AFM test resource name"))?;
            Ok((u8::try_from(code).ok(), Name::new(name.as_bytes()), width))
        })
        .collect()
}

#[test]
fn invalid_optional_weights_have_exact_repaired_glyphs_and_strict_tags()
-> pagura_document::source::ReadResult<()> {
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
            let result = load_text(&document, dict, Some(&Outlines));
            if recovery == Recovery::Strict && !tag.is_empty() {
                let error = result.unwrap_err();
                assert_eq!(
                    error
                        .as_error()
                        .unwrap()
                        .repair
                        .map(|r| r.as_str().as_bytes()),
                    Some(tag.as_slice())
                );
                assert_eq!(error.as_error().unwrap().object, Some(ObjectId::new(5, 0)));
                assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
                continue;
            }
            let font = result?;
            check_substituted(name, &font)?;
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

fn check_substituted(name: &str, (font, unicode): &(PdfFont, FontUnicode)) -> Result<()> {
    let glyphs = font.decode(&fixture("text", &format!("{name}.input")), Some(unicode))?;
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
fn descriptor_styles_and_symbol_aliases_preserve_pdf_positions_in_both_modes()
-> pagura_document::source::ReadResult<()> {
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
            let font = load_text(&document, dictionary, Some(&Outlines))?;
            check_substituted(name, &font)?;
            let glyphs = font
                .0
                .decode(&fixture("text", &format!("{name}.input")), None)?;
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
fn substituted_fonts_reject_malformed_unicode_in_strict_mode()
-> pagura_document::source::ReadResult<()> {
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
            let result = load_text(&document, dictionary, Some(&Outlines));
            if recovery == Recovery::Strict {
                let error = result.unwrap_err();
                let tag = fixture("text", &format!("{name}.strict.error"));
                assert_eq!(
                    error
                        .as_error()
                        .unwrap()
                        .repair
                        .map(pagura_core::Repair::as_str),
                    Some(
                        std::str::from_utf8(&tag)
                            .map_err(|_| Error::syntax("invalid tag"))?
                            .trim()
                    )
                );
                // The substitute loads before its Unicode is rejected.
                assert_eq!(
                    document
                        .diagnostics()
                        .iter()
                        .map(|item| item.kind)
                        .collect::<Vec<_>>(),
                    [pagura_core::DiagnosticKind::Information(
                        information::SUBSTITUTED
                    )]
                );
                continue;
            }
            let (font, unicode) = result?;
            let glyphs = font.decode(&fixture("text", &format!("{name}.input")), Some(&unicode))?;
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
                // The font loads before its Unicode.
                [
                    pagura_core::DiagnosticKind::Information(information::SUBSTITUTED),
                    pagura_core::DiagnosticKind::Repair(repairs::TOUNICODE_UTF16)
                ]
            );
        }
    }
    Ok(())
}

#[test]
fn null_or_dangling_font_files_still_use_the_provider() -> pagura_document::source::ReadResult<()> {
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
        let substituted = load_text(&document, &font, Some(&Outlines))?;
        check_substituted(name, &substituted)?;
        let error = PdfFont::load(&document, &font)
            .err()
            .ok_or_else(|| Error::syntax("expected a missing program"))?;
        assert_eq!(
            error.as_error().unwrap().feature(),
            Some(features::NON_EMBEDDED)
        );
    }
    Ok(())
}

#[test]
fn substituted_cid_fonts_still_repair_contradictory_descriptor_flags()
-> pagura_document::source::ReadResult<()> {
    // The CID substitute never reads the flags, but Symbolic and Nonsymbolic
    // both set is still the descriptor's repair (9.8.2, Table 121).
    let name = "repair-cid-substitute-flags";
    let strict = open(name, Recovery::Strict)?;
    let error = rejected(PdfFont::load_with_provider(
        &strict,
        dictionary(&strict)?,
        Some(&Outlines),
    ))?;
    assert_eq!(error.repair, Some(repairs::DESCRIPTOR_FLAGS), "{error}");
    assert_eq!(error.object, Some(ObjectId::new(6, 0)));
    let repaired = open(name, Recovery::Repair)?;
    let font = PdfFont::load_with_provider(&repaired, dictionary(&repaired)?, Some(&Outlines))?;
    assert_eq!(font.kind(), FontKind::Type0);
    assert_eq!(lines(name, "repair")?, [repairs::DESCRIPTOR_FLAGS.as_str()]);
    let flags: Vec<_> = repaired
        .diagnostics()
        .into_iter()
        .filter(|item| item.repair() == Some(repairs::DESCRIPTOR_FLAGS))
        .map(|item| item.object)
        .collect();
    assert_eq!(flags, [Some(ObjectId::new(6, 0))]);
    Ok(())
}
