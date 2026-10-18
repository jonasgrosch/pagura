//! Independent locked-data glyph oracles and strict/repaired descriptor results.
use super::*;
use pagura_core::{
    Name, Recovery, Value,
    syntax::{Origin, Parser},
};
use pagura_document::OpenOptions;
use pagura_testkit::fixture;
use std::collections::BTreeMap;

struct Outlines;

impl Provider for Outlines {
    fn standard_font(&self, base: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        let bytes: &[u8] = match base {
            b"Helvetica" => include_bytes!("../../../data/fonts/Arimo-Regular.ttf"),
            b"Helvetica-Bold" => include_bytes!("../../../data/fonts/Arimo-Bold.ttf"),
            b"Times-Roman" => include_bytes!("../../../data/fonts/Tinos-Regular.ttf"),
            b"Times-Italic" => include_bytes!("../../../data/fonts/Tinos-Italic.ttf"),
            _ => return Ok(None),
        };
        let program =
            FontProgram::parse(bytes, pagura_font::FontFormat::TrueType, &Limits::default())?;
        // These fixtures carry explicit PDF widths and Encoding. The provider
        // supplies only locked outlines; no oracle values enter this resource.
        Ok(Some(Arc::new(StandardFont {
            metrics: Arc::new(StandardMetrics {
                name: Name::new(base),
                encoding: std::array::from_fn(|_| None),
                widths: BTreeMap::new(),
                descriptor: StandardDescriptor::default(),
            }),
            program: Some(Arc::new(program)),
        })))
    }
    fn predefined_cmap(&self, _: &[u8]) -> Result<Option<Arc<PredefinedCMap>>> {
        Ok(None)
    }
    fn cid_to_unicode(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<CidToUnicode>>> {
        Ok(None)
    }
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
            let result = PdfFont::load_with(&document, dict, &Outlines);
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
            let repairs: Vec<_> = document.diagnostics().iter().map(|d| d.repair).collect();
            assert_eq!(
                repairs,
                if tag.is_empty() {
                    vec![SUBSTITUTED]
                } else {
                    vec![repairs::DESCRIPTOR_WEIGHT, SUBSTITUTED]
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
