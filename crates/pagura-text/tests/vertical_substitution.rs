#![forbid(unsafe_code)]
//! Vertical provider glyph forms preserve PDF metrics, extraction and ownership.
use pagura_core::{Error, ErrorKind, Limits, ObjectId, Result};
use pagura_document::{Document, OpenOptions};
use pagura_font::{FontFormat, FontProgram, GlyphId};
use pagura_testkit::fixture;
use pagura_text::{
    CidToUnicode, GlyphShape, PdfFont, PredefinedCMap, Provider, StandardFont, WritingMode,
};
use std::sync::Arc;

struct Substitute {
    font: Arc<FontProgram>,
    unicode: Arc<CidToUnicode>,
}

impl Substitute {
    fn new(name: &str) -> Result<Self> {
        let limits = Limits::default();
        Ok(Self {
            font: Arc::new(FontProgram::parse(
                fixture("vertical_fonts", &format!("{name}.font")),
                FontFormat::TrueType,
                &limits,
            )?),
            unicode: Arc::new(CidToUnicode::from_scalars([(1, 'A')], &limits)?),
        })
    }
}

impl Provider for Substitute {
    fn cid_font(&self, _registry: &[u8], _ordering: &[u8]) -> Result<Option<Arc<FontProgram>>> {
        Ok(Some(Arc::clone(&self.font)))
    }
    fn cid_to_unicode(
        &self,
        _registry: &[u8],
        _ordering: &[u8],
    ) -> Result<Option<Arc<CidToUnicode>>> {
        Ok(Some(Arc::clone(&self.unicode)))
    }
    fn standard_font(&self, _name: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        Ok(None)
    }
    fn predefined_cmap(&self, _name: &[u8]) -> Result<Option<PredefinedCMap>> {
        Ok(None)
    }
}

#[test]
fn provider_forms_preserve_pdf_metrics_and_independent_extraction() -> Result<()> {
    let provider = Substitute::new("single-delta")?;
    let mut retained = [0; 2];
    for (mode, count) in retained.iter_mut().enumerate() {
        let document = Document::open(
            fixture("vertical_fonts", &format!("pdf-mode-{mode}.pdf")),
            &OpenOptions::default(),
        )?;
        let dictionary = document
            .object(ObjectId::new(4, 0))?
            .dictionary()
            .ok_or_else(|| Error::syntax("missing font"))?;
        let font = PdfFont::load_with_provider(&document, dictionary, Some(&provider))?;
        let unicode = font.unicode(&document, dictionary, Some(&provider))?;
        let glyphs = font.decode(&[0, 1], Some(&unicode))?;
        let glyph = &glyphs[0];
        assert!((glyph.advance - 0.731).abs() < 1e-12);
        assert_eq!(glyph.unicode.as_deref(), Some("Z"));
        let GlyphShape::Outline {
            program, glyph: id, ..
        } = &glyph.shape
        else {
            return Err(Error::syntax("missing outline"));
        };
        assert!(Arc::ptr_eq(program, &provider.font));
        if mode == 0 {
            assert_eq!(font.writing_mode(), WritingMode::Horizontal);
            assert_eq!(*id, GlyphId(1));
            assert_eq!(glyph.vertical, None);
        } else {
            assert_eq!(font.writing_mode(), WritingMode::Vertical);
            assert_eq!(*id, GlyphId(2));
            let metrics = glyph
                .vertical
                .ok_or_else(|| Error::syntax("missing vertical metrics"))?;
            assert!((metrics.advance + 0.913).abs() < 1e-12);
            assert!((metrics.origin.0 - 0.107).abs() < 1e-12);
            assert!((metrics.origin.1 - 0.809).abs() < 1e-12);
        }
        *count = font.retained_bytes();
        font.decode(&[0, 1, 0, 1], Some(&unicode))?;
        assert_eq!(*count, font.retained_bytes());
    }
    assert!(retained[1] > retained[0]);
    Ok(())
}

#[test]
fn missing_vertical_features_and_missing_providers_remain_distinct() -> Result<()> {
    let document = Document::open(
        fixture("vertical_fonts", "pdf-mode-1.pdf"),
        &OpenOptions::default(),
    )?;
    let dictionary = document
        .object(ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("missing font"))?;
    let provider = Substitute::new("missing-feature")?;
    assert_eq!(
        PdfFont::load_with_provider(&document, dictionary, Some(&provider))
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(pagura_text::features::CID_SUBSTITUTE_VERTICAL)
    );
    assert_eq!(
        PdfFont::load(&document, dictionary).unwrap_err().kind,
        ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED)
    );
    Ok(())
}
