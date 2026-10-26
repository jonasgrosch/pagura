//! Fonts that generated variable text is laid out with (9.6, 9.7).

use pagura_core::{Dictionary, Limits, ObjectId};
use pagura_document::Document;
use pagura_document::source::ReadResult;
use pagura_text::{PdfFont, Provider};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Loads the fonts that generated variable text is laid out with.
///
/// An interpreter implements it over its own font cache, so that a field's
/// font is loaded once for both layout and drawing; [`ProviderFonts`] is the
/// stand-alone implementation.
pub trait FontSource {
    /// Loads the font dictionary `dictionary` of `document`, which is the
    /// indirect object `id` when it has one, as
    /// [`PdfFont::load_with_provider`] does.
    ///
    /// # Errors
    /// The errors of loading the font.
    fn font(
        &self,
        document: &Document,
        id: Option<ObjectId>,
        dictionary: &Dictionary,
    ) -> ReadResult<Arc<PdfFont>>;

    /// The provider whose standard-font metrics give a font without a
    /// descriptor its ascent and descent, if any.
    fn provider(&self) -> Option<&dyn Provider>;
}

/// The most fonts [`ProviderFonts`] keeps at once.
const KEPT_FONTS: usize = 256;

/// A [`FontSource`] that loads fonts with an optional provider (ADR 0009)
/// and keeps the fonts of indirect dictionaries for reuse while their
/// estimated storage stays within the document byte budget.
pub struct ProviderFonts<'p> {
    provider: Option<&'p dyn Provider>,
    maximum: usize,
    kept: RefCell<Kept>,
}

#[derive(Default)]
struct Kept {
    fonts: BTreeMap<ObjectId, Arc<PdfFont>>,
    bytes: usize,
}

impl std::fmt::Debug for ProviderFonts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderFonts")
            .field("provider", &self.provider.is_some())
            .field("kept", &self.kept.borrow().fonts.len())
            .finish_non_exhaustive()
    }
}

impl<'p> ProviderFonts<'p> {
    /// Loads with `provider`, or only from the document for `None`, keeping
    /// at most `limits.document_bytes` of fonts.
    pub fn new(provider: Option<&'p dyn Provider>, limits: &Limits) -> Self {
        Self {
            provider,
            maximum: limits.document_bytes,
            kept: RefCell::default(),
        }
    }
}

impl FontSource for ProviderFonts<'_> {
    fn font(
        &self,
        document: &Document,
        id: Option<ObjectId>,
        dictionary: &Dictionary,
    ) -> ReadResult<Arc<PdfFont>> {
        document.check_source()?;
        if let Some(font) = id.and_then(|id| self.kept.borrow().fonts.get(&id).cloned()) {
            return Ok(font);
        }
        let font = Arc::new(PdfFont::load_with_provider(
            document,
            dictionary,
            self.provider,
        )?);
        if let Some(id) = id {
            let bytes = font.retained_bytes();
            let mut kept = self.kept.borrow_mut();
            if kept.bytes.saturating_add(bytes) > self.maximum || kept.fonts.len() >= KEPT_FONTS {
                *kept = Kept::default();
            }
            if bytes <= self.maximum {
                kept.bytes += bytes;
                kept.fonts.insert(id, Arc::clone(&font));
            }
        }
        Ok(font)
    }

    fn provider(&self) -> Option<&dyn Provider> {
        self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_document::OpenOptions;
    use pagura_testkit::fixture;

    #[test]
    fn indirect_fonts_are_loaded_once_within_the_byte_budget() {
        let document =
            Document::open(fixture("forms", "render-text.pdf"), &OpenOptions::default()).unwrap();
        let form = document.acro_form().unwrap().unwrap();
        let resources = form.default_resources.unwrap();
        let fonts = document
            .resolve_dictionary(resources.get(b"Font").unwrap())
            .unwrap();
        let (_, value) = fonts.iter().next().unwrap();
        let id = value.as_reference();
        let dictionary = document.resolve_dictionary(value).unwrap();
        let source = ProviderFonts::new(None, &document.limits());
        let first = source.font(&document, id, dictionary).unwrap();
        let second = source.font(&document, id, dictionary).unwrap();
        assert_eq!(Arc::ptr_eq(&first, &second), id.is_some());
        let none = ProviderFonts::new(
            None,
            &Limits {
                document_bytes: 0,
                ..Default::default()
            },
        );
        let first = none.font(&document, id, dictionary).unwrap();
        let second = none.font(&document, id, dictionary).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(format!("{source:?}").starts_with("ProviderFonts"));
    }
}
