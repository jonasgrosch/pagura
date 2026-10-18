//! Data-derived oracles generated independently in tools/fixturegen/text.py.
use super::*;
use pagura_core::{
    Error, Value,
    syntax::{Origin, Parser},
};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;
use pagura_text::{GlyphShape, PdfFont};

pub(super) fn expected(name: &str, suffix: &str) -> Result<Vec<Value>> {
    let bytes = fixture("text", &format!("{name}.{suffix}"));
    let value = Parser::new(&bytes, Origin::Buffer, Limits::default()).parse()?;
    if let Value::Array(values) = value {
        Ok(values)
    } else {
        Err(Error::syntax("oracle must be an array"))
    }
}
pub(super) fn row(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("oracle must be a row"))
}
pub(super) fn number(value: &Value) -> Result<f64> {
    value
        .as_number()
        .ok_or_else(|| Error::syntax("oracle must be numeric"))
}
pub(super) fn integer(value: &Value) -> Result<u32> {
    value
        .as_integer()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| Error::syntax("oracle must be an unsigned integer"))
}
pub(super) fn text(value: &Value) -> Option<&str> {
    value
        .as_string()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
}
pub(super) fn names(inventory: &str) -> Result<Vec<String>> {
    expected(inventory, "out")?
        .iter()
        .map(|value| {
            text(&row(value)?[0])
                .map(str::to_owned)
                .ok_or_else(|| Error::syntax("oracle name must be a string"))
        })
        .collect()
}
pub(super) fn open(name: &str) -> Result<Document> {
    Document::open(
        fixture("text", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
}
pub(super) fn dictionary(document: &Document) -> Result<&pagura_core::Dictionary> {
    document
        .object(pagura_core::ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("missing font dictionary"))
}
pub(super) fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

#[cfg(feature = "standard-fonts")]
#[test]
fn standard_fonts_aliases_afm_metrics_and_outlines() -> Result<()> {
    let provider = Bundled::default();
    for name in names("bundled-standard-inventory")? {
        let base = fixture("text", &format!("{name}.base"));
        let resource = provider
            .standard_font(&base)?
            .ok_or_else(|| Error::syntax("missing standard font"))?;
        assert_eq!(resource.metrics.name.as_bytes(), base);
        let again = provider
            .standard_font(&base)?
            .ok_or_else(|| Error::syntax("missing cached font"))?;
        assert!(Arc::ptr_eq(&resource, &again));
        let document = open(&name)?;
        assert!(PdfFont::load(&document, dictionary(&document)?).is_err());
        let font = PdfFont::load_with(&document, dictionary(&document)?, &provider)?;
        for value in expected(&name, "out")? {
            let values = row(&value)?;
            near(font.width(integer(&values[0])?), number(&values[1])?);
        }
        let input = fixture("text", &format!("{name}.input"));
        if resource.program.is_none() {
            let error = font.decode(&input).unwrap_err();
            assert_eq!(
                error.kind,
                pagura_core::ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED)
            );
        } else {
            assert_eq!(document.diagnostics().len(), 1);
            assert_eq!(
                document.diagnostics()[0].kind,
                pagura_core::DiagnosticKind::Information(pagura_text::SUBSTITUTED)
            );
            let glyphs = font.decode(&input)?;
            for (glyph, value) in glyphs.iter().zip(expected(&name, "out")?) {
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
        if name == format!("standard14-{}", String::from_utf8_lossy(&base)) {
            check_afm_metrics(&name, &resource)?;
        }
    }
    Ok(())
}

#[cfg(feature = "standard-fonts")]
#[test]
fn descriptor_substitution_retains_pdf_metrics_and_rejects_unsafe_symbols() -> Result<()> {
    let provider = Bundled::default();
    for name in names("bundled-substitute-inventory")? {
        let document = open(&name)?;
        let font = PdfFont::load_with(&document, dictionary(&document)?, &provider)?;
        let glyphs = font.decode(b"A")?;
        let values = expected(&name, "out")?;
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
    for name in [
        "substitute-unsafe-symbolic",
        "substitute-unsafe-symbolic-both",
        "substitute-unsafe-symbolic-known-base",
    ] {
        let document = open(name)?;
        assert_eq!(
            PdfFont::load_with(&document, dictionary(&document)?, &provider)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED)
        );
        assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
    }
    Ok(())
}

#[cfg(feature = "cmaps")]
#[test]
fn every_predefined_cmap_and_pdf_binding_matches_file_oracles() -> Result<()> {
    let provider = Bundled::default();
    for name in names("bundled-cmap-inventory")? {
        let cmap_name = fixture("text", &format!("{name}.cmap"));
        let cmap = provider
            .predefined_cmap(&cmap_name)?
            .ok_or_else(|| Error::syntax("missing predefined CMap"))?;
        let again = provider
            .predefined_cmap(&cmap_name)?
            .ok_or_else(|| Error::syntax("missing cached CMap"))?;
        assert!(Arc::ptr_eq(&cmap, &again));
        let mode = fixture("text", &format!("{name}.mode"));
        assert_eq!(
            cmap.writing_mode(),
            if mode == b"1" {
                pagura_text::WritingMode::Vertical
            } else {
                pagura_text::WritingMode::Horizontal
            }
        );
        let collection = fixture("text", &format!("{name}.collection"));
        assert_eq!(
            cmap.collection(),
            Some((b"Adobe".as_slice(), collection.as_slice()))
        );
        let input = fixture("text", &format!("{name}.input"));
        let mut bytes = input.as_slice();
        for value in expected(&name, "out")? {
            let values = row(&value)?;
            let (code, len, cid) = cmap.decode_code(bytes, &Limits::default())?;
            assert_eq!(
                (code, u32::from(len), cid),
                (
                    integer(&values[0])?,
                    integer(&values[1])?,
                    integer(&values[2])?
                ),
                "{name}"
            );
            bytes = &bytes[usize::from(len)..];
        }
        assert_eq!(bytes, []);
        let document = open(&name)?;
        let font = PdfFont::load_with(&document, dictionary(&document)?, &provider)?;
        assert_eq!(font.writing_mode(), cmap.writing_mode());
        let glyphs = font.decode(&input)?;
        let expected = expected(&name, "glyphs")?;
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
        assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
    }
    Ok(())
}

#[cfg(feature = "cmaps")]
#[test]
fn collection_unicode_aliases_match_independent_rows() -> Result<()> {
    let provider = Bundled::default();
    for collection in ["CNS1", "GB1", "Japan1", "Korea1"] {
        let mapping = provider
            .cid_to_unicode(b"Adobe", collection.as_bytes())?
            .ok_or_else(|| Error::syntax("missing collection"))?;
        let again = provider
            .cid_to_unicode(b"Adobe", collection.as_bytes())?
            .ok_or_else(|| Error::syntax("missing cached collection"))?;
        assert!(Arc::ptr_eq(&mapping, &again));
        for value in expected(&format!("collection-{collection}"), "out")? {
            let values = row(&value)?;
            assert_eq!(mapping.get(integer(&values[0])?), text(&values[1]));
        }
    }
    Ok(())
}

#[test]
fn unavailable_resources_are_explicit_and_not_cached_as_guesses() -> Result<()> {
    let provider = Bundled::default();
    assert!(provider.cid_font(b"Unknown", b"Japan1")?.is_none());
    assert!(provider.cid_font(b"Adobe", b"Unknown")?.is_none());
    #[cfg(not(feature = "cjk-fonts"))]
    assert!(provider.cid_font(b"Adobe", b"Japan1")?.is_none());
    assert!(provider.standard_font(b"Unknown")?.is_none());
    assert!(provider.predefined_cmap(b"Unknown")?.is_none());
    assert!(provider.cid_to_unicode(b"Unknown", b"Japan1")?.is_none());
    assert!(provider.cid_to_unicode(b"Adobe", b"Unknown")?.is_none());
    #[cfg(not(feature = "standard-fonts"))]
    assert!(provider.standard_font(b"Helvetica")?.is_none());
    #[cfg(not(feature = "cmaps"))]
    assert!(provider.predefined_cmap(b"90ms-RKSJ-H")?.is_none());
    Ok(())
}

#[test]
fn parsing_budgets_fail_before_publication() {
    // Each font budget is zeroed together with the general field that the
    // pagura-text and pagura-font parsers of these files have used.
    for limits in [
        Limits {
            font_work: 0,
            operations: 0,
            ..Limits::default()
        },
        Limits {
            font_entries: 0,
            value_nodes: 0,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 0,
            ..Limits::default()
        },
    ] {
        let provider = Bundled::new(limits);
        #[cfg(feature = "cjk-fonts")]
        assert_eq!(
            provider.cid_font(b"Adobe", b"Japan1").unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
        #[cfg(feature = "standard-fonts")]
        assert_eq!(
            provider.standard_font(b"Helvetica").unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
        #[cfg(feature = "cmaps")]
        {
            assert_eq!(
                provider.predefined_cmap(b"90ms-RKSJ-H").unwrap_err().kind,
                pagura_core::ErrorKind::Limit
            );
            assert_eq!(
                provider
                    .cid_to_unicode(b"Adobe", b"Japan1")
                    .unwrap_err()
                    .kind,
                pagura_core::ErrorKind::Limit
            );
        }
    }
    #[cfg(feature = "cmaps")]
    assert_eq!(
        Bundled::new(Limits {
            depth: 1,
            ..Limits::default()
        })
        .predefined_cmap(b"90ms-RKSJ-V")
        .unwrap_err()
        .kind,
        pagura_core::ErrorKind::Limit
    );
}

#[cfg(feature = "standard-fonts")]
#[test]
fn embedded_programs_take_precedence_and_do_not_report_substitution() -> Result<()> {
    let provider = Bundled::default();
    let document = open("tt-unicode-priority")?;
    let font = PdfFont::load_with(&document, dictionary(&document)?, &provider)?;
    let glyphs = font.decode(b"A")?;
    let GlyphShape::Outline { program, glyph, .. } = &glyphs[0].shape else {
        return Err(Error::syntax("missing embedded program"));
    };
    assert_eq!(program.glyph_count(), 5);
    assert_eq!(glyph.0, 1);
    assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
    // An embedded but invalid binding is an error, never a substitution.
    let document = open("binding-wrong-fontfile")?;
    assert_eq!(
        PdfFont::load_with(&document, dictionary(&document)?, &provider)
            .unwrap_err()
            .kind,
        pagura_core::ErrorKind::Syntax
    );
    Ok(())
}

#[cfg(feature = "cmaps")]
#[test]
fn resource_inheritance_cannot_redefine_its_adopted_codespace() {
    let provider = Bundled::default();
    let error = PredefinedCMap::parse(
        b"/90ms-RKSJ-H usecmap 1 begincodespacerange <00> <ff> endcodespacerange",
        &Limits::default(),
        |name| {
            provider
                .predefined_cmap(name)?
                .ok_or_else(|| Error::syntax("missing parent"))
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Syntax);
}

#[cfg(feature = "standard-fonts")]
fn check_afm_metrics(name: &str, resource: &StandardFont) -> Result<()> {
    for value in expected(name, "metrics")? {
        let values = row(&value)?;
        let glyph_name = pagura_core::Name::new(
            values[1]
                .as_string()
                .ok_or_else(|| Error::syntax("missing AFM glyph name"))?,
        );
        near(
            *resource
                .metrics
                .widths
                .get(&glyph_name)
                .ok_or_else(|| Error::syntax("missing AFM glyph width"))?,
            number(&values[2])?,
        );
        if let Some(code) = values[0]
            .as_integer()
            .and_then(|code| u8::try_from(code).ok())
        {
            assert_eq!(
                resource.metrics.encoding[usize::from(code)].as_ref(),
                Some(&glyph_name)
            );
            near(
                resource
                    .metrics
                    .width(code)
                    .ok_or_else(|| Error::syntax("missing AFM code width"))?,
                number(&values[2])?,
            );
        }
    }
    let descriptor = expected(name, "descriptor")?;
    let values = row(&descriptor[0])?;
    let d = &resource.metrics.descriptor;
    for (actual, expected) in d.bbox.iter().zip(values) {
        near(*actual, number(expected)?);
    }
    near(d.italic_angle, number(&values[4])?);
    assert_eq!(Some(d.fixed_pitch), values[5].as_bool());
    for (actual, expected) in [
        d.ascent,
        d.descent,
        d.cap_height,
        d.x_height,
        d.stem_v,
        d.stem_h,
    ]
    .into_iter()
    .zip(&values[6..])
    {
        assert_eq!(actual, expected.as_number());
    }

    Ok(())
}

#[test]
fn notices_accompany_every_enabled_data_set() {
    let notices: Vec<_> = notices().collect();
    let has = |subject: &str, phrase: &str| {
        notices
            .iter()
            .any(|(name, text)| *name == subject && text.contains(phrase))
    };
    if cfg!(feature = "standard-fonts") {
        assert!(has(
            "Adobe Core 14 AFM files",
            "not distributed without this file"
        ));
        assert!(has(
            "Arimo, Tinos and Cousine",
            "Apache License, Version 2.0"
        ));
        assert!(has("Apache License, Version 2.0", "TERMS AND CONDITIONS"));
        assert!(has(
            "Foxit Symbol and Dingbats",
            "Redistributions in binary form"
        ));
    }
    if cfg!(feature = "cjk-fonts") {
        assert!(has("Source Han Sans", "SIL OPEN FONT LICENSE"));
    } else {
        assert!(!notices.iter().any(|(name, _)| *name == "Source Han Sans"));
    }
    if cfg!(feature = "cmaps") {
        assert!(has(
            "Adobe cmap-resources",
            "Redistributions in binary form"
        ));
    }
}

#[cfg(all(feature = "cmaps", not(feature = "cjk-fonts")))]
#[test]
fn cjk_programs_are_opt_in_and_missing_programs_fail_explicitly() -> Result<()> {
    let provider = Bundled::default();
    for ordering in ["Japan1", "GB1", "CNS1", "Korea1"] {
        let document = open(&format!("substitute-cid-{ordering}-CIDFontType2"))?;
        assert_eq!(
            PdfFont::load_with(&document, dictionary(&document)?, &provider)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED)
        );
        assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
    }
    Ok(())
}
