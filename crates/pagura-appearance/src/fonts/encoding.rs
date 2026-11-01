//! Reverse simple-font Unicode mappings using the PDF font decoder.

use crate::features;
use crate::fonts::FontSource;
use crate::values::{optional, optional_dictionary};
use pagura_core::{Dictionary, Error, Position, Result};
use pagura_document::Document;
use pagura_text::{FontKind, FontUnicode, PdfFont, WritingMode};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A font's reversible single-byte codes, advances and vertical
/// extent in text space units.
pub(crate) struct Encoding {
    codes: BTreeMap<char, Code>,
    pub(crate) ascent: f64,
    pub(crate) descent: f64,
}

/// A character's code and horizontal advance in text space.
#[derive(Clone, Copy)]
pub(crate) struct Code {
    pub(crate) code: u8,
    pub(crate) advance: f64,
}

impl Encoding {
    /// Loads the DA font `name` of `resources` through `fonts`.
    pub(crate) fn load(
        document: &Document,
        resources: &Dictionary,
        name: &[u8],
        fonts: &dyn FontSource,
    ) -> Result<Self> {
        let entries = resources
            .get(b"Font")
            .ok_or(Error::syntax("form resources have no Font dictionary"))?;
        let entries = document.resolve_dictionary(entries)?;
        let entry = entries
            .get(name)
            .ok_or(Error::syntax("DA font is absent from form resources"))?;
        let dictionary = document.resolve_dictionary(entry)?;
        let (font, unicode) = load(document, entry.as_reference(), dictionary, fonts)?;
        if font.kind() == FontKind::Type0 || font.writing_mode() != WritingMode::Horizontal {
            return Err(Error::unsupported(
                features::APPEARANCE_FONT_ENCODING,
                "appearance generation requires a horizontal simple font",
            ));
        }
        let limits = document.limits();
        if limits.operations < 256 || limits.document_bytes < 256 * 64 {
            return Err(Error::limit("appearance font mapping budget exceeded"));
        }
        let mut codes = BTreeMap::new();
        for code in 0..=u8::MAX {
            for glyph in font.decode(&[code], Some(&unicode))? {
                if let Some(text) = glyph.unicode {
                    let mut chars = text.chars();
                    if let Some(c) = chars.next().filter(|_| chars.next().is_none()) {
                        codes.entry(c).or_insert(Code {
                            code,
                            advance: glyph.advance,
                        });
                    }
                }
            }
        }
        let (ascent, descent) = extent(document, dictionary, fonts)?;
        Ok(Self {
            codes,
            ascent,
            descent,
        })
    }

    /// The code that shows `c`.
    pub(crate) fn encode(&self, c: char) -> Result<Code> {
        self.codes.get(&c).copied().ok_or(Error::unsupported(
            features::APPEARANCE_FONT_ENCODING,
            "appearance character has no reversible font encoding",
        ))
    }
}

/// Loads a font through `fonts` with the Unicode of its codes, placing
/// their errors in the font object. Buffer offsets inside font programs
/// mean nothing at the widget, which places the error instead.
fn load(
    document: &Document,
    id: Option<pagura_core::ObjectId>,
    dictionary: &Dictionary,
    fonts: &dyn FontSource,
) -> Result<(Arc<PdfFont>, FontUnicode)> {
    let loaded = fonts.font(document, id, dictionary).and_then(|font| {
        let unicode = font.unicode(document, dictionary, fonts.provider())?;
        Ok((font, unicode))
    });
    loaded.map_err(|mut error| {
        if matches!(error.position, Position::Buffer(_)) {
            error.position = Position::Unknown;
        }
        match id {
            Some(id) if error.repair != Some(pagura_text::repairs::DESCRIPTOR_FLAGS) => {
                error.in_object(id)
            }
            _ => error,
        }
    })
}

/// Ascent and descent in em: the descriptor's, else the provider's
/// standard-font metrics, else 0.8 and -0.2.
fn extent(
    document: &Document,
    dictionary: &Dictionary,
    fonts: &dyn FontSource,
) -> Result<(f64, f64)> {
    let (mut ascent, mut descent) = (0.8, -0.2);
    if let Some(base) = dictionary.get_name(b"BaseFont")
        && let Some(provider) = fonts.provider()
        && let Some(standard) = provider.standard_font(base.as_bytes())?
    {
        ascent = standard.metrics.descriptor.ascent.unwrap_or(800.0) / 1000.0;
        descent = standard.metrics.descriptor.descent.unwrap_or(-200.0) / 1000.0;
    }
    if let Some(descriptor) = optional_dictionary(document, dictionary, b"FontDescriptor")? {
        ascent = metric(document, descriptor, b"Ascent")?.unwrap_or(ascent * 1000.0) / 1000.0;
        descent = metric(document, descriptor, b"Descent")?.unwrap_or(descent * 1000.0) / 1000.0;
    }
    if !ascent.is_finite() || !descent.is_finite() || ascent <= descent {
        return Err(Error::syntax(
            "invalid variable text font ascent and descent",
        ));
    }
    Ok((ascent, descent))
}

fn metric(document: &Document, dictionary: &Dictionary, key: &[u8]) -> Result<Option<f64>> {
    optional(document, dictionary, key)?
        .map(|value| {
            document
                .resolve(value)?
                .as_number()
                .ok_or(Error::syntax("font metric must be numeric"))
        })
        .transpose()
}
