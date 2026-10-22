//! Reverse simple-font Unicode mappings using the existing PDF font decoder.
use super::{optional, optional_dictionary};
use pagura_core::{Dictionary, Error, Result};
use pagura_document::Document;
use pagura_text::{FontKind, Provider, WritingMode};
use std::collections::BTreeMap;

pub(super) struct Font {
    pub codes: BTreeMap<char, (u8, f64)>,
    pub ascent: f64,
    pub descent: f64,
}
impl Font {
    pub fn load(
        document: &Document,
        resources: &Dictionary,
        name: &[u8],
        provider: Option<&dyn Provider>,
        cache: Option<&std::sync::Mutex<crate::interpreter::text::cache::FontCache>>,
    ) -> Result<Self> {
        let fonts = resources
            .get(b"Font")
            .ok_or(Error::syntax("form resources have no Font dictionary"))?;
        let fonts = document.resolve_dictionary(fonts)?;
        let dictionary = fonts
            .get(name)
            .ok_or(Error::syntax("DA font is absent from form resources"))?;
        let font =
            crate::interpreter::text::cache::pdf_font(document, dictionary, provider, cache)?;
        let dictionary = document.resolve_dictionary(dictionary)?;
        if font.kind() == FontKind::Type0 || font.writing_mode() != WritingMode::Horizontal {
            return Err(Error::unsupported(
                crate::features::APPEARANCE_FONT_ENCODING,
                "appearance generation requires a horizontal simple font",
            ));
        }
        if document.limits().operations < 256 || document.limits().document_bytes < 256 * 64 {
            return Err(Error::limit("appearance font mapping budget exceeded"));
        }
        let mut codes = BTreeMap::new();
        for code in 0..=u8::MAX {
            for glyph in font.decode(&[code])? {
                if let Some(text) = glyph.unicode {
                    let mut chars = text.chars();
                    if let Some(c) = chars.next().filter(|_| chars.next().is_none()) {
                        codes.entry(c).or_insert((code, glyph.advance));
                    }
                }
            }
        }
        let (mut ascent, mut descent) = (0.8, -0.2);
        if let Some(base) = dictionary.get_name(b"BaseFont")
            && let Some(provider) = provider
            && let Some(standard) = provider.standard_font(base.as_bytes())?
        {
            ascent = standard.metrics.descriptor.ascent.unwrap_or(800.0) / 1000.0;
            descent = standard.metrics.descriptor.descent.unwrap_or(-200.0) / 1000.0;
        }
        if let Some(descriptor) = optional_dictionary(document, dictionary, b"FontDescriptor")? {
            ascent = metric(document, descriptor, b"Ascent")?.unwrap_or(ascent * 1000.0) / 1000.0;
            descent =
                metric(document, descriptor, b"Descent")?.unwrap_or(descent * 1000.0) / 1000.0;
        }
        if !ascent.is_finite() || !descent.is_finite() || ascent <= descent {
            return Err(Error::syntax(
                "invalid variable text font ascent and descent",
            ));
        }
        Ok(Self {
            codes,
            ascent,
            descent,
        })
    }
    pub fn encode(&self, c: char) -> Result<(u8, f64)> {
        self.codes.get(&c).copied().ok_or(Error::unsupported(
            crate::features::APPEARANCE_FONT_ENCODING,
            "field character has no reversible font encoding",
        ))
    }
}
fn metric(document: &Document, dictionary: &Dictionary, key: &[u8]) -> Result<Option<f64>> {
    optional(document, dictionary, key)?
        .map(|v| {
            document
                .resolve(v)?
                .as_number()
                .ok_or(Error::syntax("font metric must be numeric"))
        })
        .transpose()
}
