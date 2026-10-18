//! `text`: PDF font dictionaries load and decode strings into glyphs, or
//! fail with a structured error (clause 9; ADR 0009).
//!
//! Layout: a selector byte, then parts split at [`super::wrapper::SEPARATOR`].
//! The first part is the string to decode; the others become objects 3, 4,
//! … of a fixed document, so a font dictionary can refer to its encoding,
//! CMap, `/ToUnicode`, descendant and font program streams. The selector's
//! low three bits choose the font dictionary's object number (3 + bits), and
//! its high bit selects strict recovery.

use super::wrapper::{self, FIRST_OBJECT, Pdf};
use pagura::{Document, ObjectId, OpenOptions};
use pagura_text::{GlyphShape, PdfFont};

/// How many decoded glyphs have their shapes loaded.
const SHAPES: usize = 64;

/// Loads the selected font dictionary and decodes the string.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let parts = wrapper::parts(rest);
    let Some((&string, objects)) = parts.split_first() else {
        return;
    };
    let mut pdf = Pdf::without_pages();
    pdf.parts(objects);
    let options = OpenOptions {
        limits: super::limits(),
        recovery: super::recovery(selector & 0x80 != 0),
        ..OpenOptions::default()
    };
    let Some(document) = super::checked(Document::open(pdf.finish(), &options)) else {
        return;
    };
    let root = ObjectId::new(FIRST_OBJECT + u32::from(selector & 7), 0);
    if let Some(dictionary) = super::checked(document.object(root)).and_then(|o| o.dictionary())
        && let Some(font) = super::checked(PdfFont::load(&document, dictionary))
    {
        let _ = (font.kind(), font.writing_mode(), font.type3_resources());
        if let Some(glyphs) = super::checked(font.decode(string)) {
            let consumed: usize = glyphs.iter().map(|g| usize::from(g.byte_len)).sum();
            assert_eq!(
                consumed,
                string.len(),
                "glyph codes do not cover the string"
            );
            for glyph in glyphs.iter().take(SHAPES) {
                shape(&document, &glyph.shape);
            }
        }
    }
    super::assert_diagnostics(&document.diagnostics(), options.recovery);
}

fn shape(document: &Document, shape: &GlyphShape) {
    match shape {
        GlyphShape::Outline { program, glyph, .. } => {
            super::checked(program.outline(*glyph));
            let _ = program.advance(*glyph);
        }
        GlyphShape::Type3 { procedure, .. } => {
            if let Some(object) = super::checked(document.object(*procedure)) {
                super::checked(document.stream_data(object));
            }
        }
        _ => {}
    }
}
