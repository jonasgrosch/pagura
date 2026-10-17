#![forbid(unsafe_code)]
//! PDF fonts and text decoding (ISO 32000-2 clause 9; ADR 0009).
//!
//! [`PdfFont::load`] interprets a font dictionary — simple fonts (Type 1,
//! MMType1, TrueType, Type 3) and composite fonts (Type 0 with CIDFontType0
//! or CIDFontType2 descendants) — with its encoding or CMap, widths, embedded
//! font program and `/ToUnicode` map. [`PdfFont::decode`] splits a string
//! operand into glyphs carrying everything the renderer and text extraction
//! need. Positioning glyphs on the page (text state, `Tm`, `TJ` adjustments)
//! belongs to the content interpreter in `pagura-render`.
//!
//! [`PdfFont::load_with`] accepts a [`Provider`] for non-embedded programs and
//! predefined CMaps; [`PdfFont::load`] retains explicit failures without one.
//! Font-program parsing is delegated to `pagura-font`. TrueType selection uses the program's subtable availability
//! and post names (9.6.5.4). Glyph-name Unicode fallback implements AGL name
//! decomposition with Latin, Dingbats and common Symbol names; unbundled legacy
//! names remain unknown. A `/ToUnicode` sequence always takes precedence.
//!
//! Loading and decoding enforce document limits. CID intervals stay compact;
//! Unicode expansion, inherited CMap graphs and decoded glyph counts are bounded.

use pagura_core::{Dictionary, Error, Limits, ObjectId, Position, Result};
use pagura_document::Document;
use pagura_font::{FontProgram, GlyphId};
use pagura_raster::Transform;
use std::sync::Arc;

mod cidmap;
mod cmap;
mod encoding;
mod font;
mod metrics;
mod objects;
mod program;
mod provider;
pub mod repairs;
mod substitute;

/// Stable diagnostic tag for a host-provided outline substitution.
pub use program::STREAM_LENGTHS;
pub use substitute::SUBSTITUTED;

pub use provider::{
    CidToUnicode, PredefinedCMap, Provider, StandardDescriptor, StandardFont, StandardMetrics,
};
mod selection;
mod tables;

/// Stable tags for text capabilities not implemented yet.
pub mod features {
    use pagura_core::Feature;

    /// A font whose program is not embedded and has no substitute yet.
    pub const NON_EMBEDDED: Feature = Feature::new("font.non-embedded");
    /// A CMap requires PostScript evaluation beyond declarative mappings.
    pub const CMAP_PROGRAM: Feature = Feature::new("font.cmap-program");
    /// Historical ambiguity tag; supported TrueType selection no longer emits it.
    pub const TRUETYPE_CMAP: Feature = Feature::new("font.truetype-cmap");
    /// A predefined CMap other than Identity-H/V, whose data is not bundled.
    pub const PREDEFINED_CMAP: Feature = Feature::new("font.predefined-cmap");
}

/// The font dictionary's `/Subtype`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontKind {
    /// `Type1`, including the standard 14 fonts.
    Type1,
    /// `MMType1` (multiple master instance).
    MmType1,
    /// `TrueType`.
    TrueType,
    /// `Type3`: glyphs are content streams.
    Type3,
    /// `Type0`: a composite font with one CIDFont descendant.
    Type0,
}

/// Horizontal or vertical writing (clause 9.7.4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WritingMode {
    /// Glyphs advance along x.
    #[default]
    Horizontal,
    /// Glyphs advance along y (CMaps ending in `-V`, `/WMode 1`).
    Vertical,
}

/// Vertical metrics of one glyph (clause 9.7.4.3, `/W2`, `/DW2`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalMetrics {
    /// Vertical displacement, in text space.
    pub advance: f64,
    /// Position vector from the horizontal to the vertical origin, in text space.
    pub origin: (f64, f64),
}

/// How a glyph is drawn.
#[derive(Debug, Clone)]
pub enum GlyphShape {
    /// An outline glyph of an embedded or explicitly substituted font program.
    Outline {
        /// The program.
        program: Arc<FontProgram>,
        /// The glyph in it.
        glyph: GlyphId,
        /// Glyph space to text space.
        matrix: Transform,
    },
    /// A Type 3 glyph procedure (a content stream) with the font's matrix.
    Type3 {
        /// The `/CharProcs` stream.
        procedure: ObjectId,
        /// `/FontMatrix`: glyph space to text space.
        matrix: Transform,
    },
    /// No mark is drawn (for example `.notdef` mapped to nothing), but the
    /// glyph still advances.
    Blank,
}

/// One decoded glyph of a string operand.
#[derive(Debug, Clone)]
pub struct Glyph {
    /// The character code.
    pub code: u32,
    /// Bytes the code occupied in the string.
    pub byte_len: u8,
    /// Horizontal displacement `w0` in text space (width / 1000 for
    /// non-Type 3 fonts), before character and word spacing.
    pub advance: f64,
    /// Vertical metrics in vertical writing mode.
    pub vertical: Option<VerticalMetrics>,
    /// Whether word spacing applies: a single-byte code 32 (clause 9.3.3).
    pub word_space: bool,
    /// Unicode text for extraction (`/ToUnicode`, then encoding glyph
    /// names), if known.
    pub unicode: Option<String>,
    /// How to draw it.
    pub shape: GlyphShape,
}

/// A loaded PDF font. Load each font dictionary once per document and share it.
#[derive(Debug)]
pub struct PdfFont {
    kind: FontKind,
    writing_mode: WritingMode,
    source: font::Source,
    mapping: font::Mapping,
    metrics: metrics::Metrics,
    to_unicode: Option<cmap::CMap>,
    limits: Limits,
    cid_unicode: Option<Arc<CidToUnicode>>,
}

impl PdfFont {
    /// Loads the font dictionary `font` (already resolved) of `document`.
    ///
    /// Faulty ToUnicode interpretations are reported and discarded without
    /// affecting glyph selection, even in Strict mode (9.10.3). Repair mode
    /// retains unambiguous values through the documented text repairs.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for malformed painting-critical dictionaries, `Unsupported`
    /// (for example [`features::NON_EMBEDDED`]) for fonts that cannot be
    /// drawn yet, and errors from loading the font program.
    pub fn load(document: &Document, font: &Dictionary) -> Result<Self> {
        font::load(document, font)
    }

    /// Loads a font with host-supplied substitutes, metrics and predefined CMaps.
    /// Embedded programs take precedence (9.6.2.2, 9.7.5.2; ADR 0009).
    ///
    /// # Errors
    /// As [`Self::load`], plus errors from the provider. Unavailable or unsafe
    /// substitutions remain explicit unsupported capabilities.
    pub fn load_with(
        document: &Document,
        font: &Dictionary,
        provider: &dyn Provider,
    ) -> Result<Self> {
        font::load_with(document, font, Some(provider))
    }

    /// Horizontal width in text space, by simple character code or CID.
    /// Available even when a provider supplies metrics without outlines.
    pub fn width(&self, code: u32) -> f64 {
        self.metrics.width(code)
    }

    /// The font's subtype.
    pub fn kind(&self) -> FontKind {
        self.kind
    }

    /// The writing mode.
    pub fn writing_mode(&self) -> WritingMode {
        self.writing_mode
    }

    /// The `/Resources` of a Type 3 font, for its glyph procedures.
    pub fn type3_resources(&self) -> Option<&Dictionary> {
        match &self.source {
            font::Source::Type3 { resources, .. } => resources.as_ref(),
            font::Source::Program { .. } | font::Source::MetricsOnly => None,
        }
    }

    /// Splits a string operand into glyphs (clauses 9.4.3 and 9.7.6).
    ///
    /// # Errors
    ///
    /// Returns syntax errors for codes outside the font's code space that
    /// recovery cannot map, and errors from loading glyph data.
    pub fn decode(&self, string: &[u8]) -> Result<Vec<Glyph>> {
        self.decode_inner(string)
            .map_err(|error| error.at(Position::Buffer(0)))
    }

    fn decode_inner(&self, string: &[u8]) -> Result<Vec<Glyph>> {
        if !string.is_empty() && matches!(self.source, font::Source::MetricsOnly) {
            return Err(Error::unsupported(
                features::NON_EMBEDDED,
                "font metrics have no available outline program",
            ));
        }
        if string.len() > self.limits.string_bytes {
            return Err(Error::limit("text string byte budget exceeded"));
        }
        let mut glyphs = Vec::new();
        let mut offset = 0;
        let mut work = 0;
        let mut unicode_bytes = 0;
        while let Some(input) = string.get(offset..).filter(|s| !s.is_empty()) {
            cmap::charge(&mut work, 1, self.limits.operations)
                .map_err(|e| e.at(Position::Buffer(offset)))?;
            if glyphs.len() >= self.limits.value_nodes {
                return Err(
                    Error::limit("decoded glyph budget exceeded").at(Position::Buffer(offset))
                );
            }
            let result = self.decode_glyph(input, &mut work);
            let glyph = result.map_err(|e| e.at(Position::Buffer(offset)))?;
            cmap::charge(
                &mut unicode_bytes,
                glyph.unicode.as_ref().map_or(0, String::len),
                self.limits.decoded_bytes,
            )
            .map_err(|e| e.at(Position::Buffer(offset)))?;
            offset += usize::from(glyph.byte_len);
            glyphs.push(glyph);
        }
        Ok(glyphs)
    }

    fn decode_glyph(&self, input: &[u8], work: &mut usize) -> Result<Glyph> {
        let (code, selector, name, fallback) = match &self.mapping {
            font::Mapping::Simple(encoding) => {
                let byte = *input
                    .first()
                    .ok_or_else(|| Error::syntax("empty character code"))?;
                (
                    cmap::Code {
                        value: u32::from(byte),
                        len: 1,
                    },
                    u32::from(byte),
                    encoding.names[usize::from(byte)].as_ref(),
                    encoding.unicode[usize::from(byte)].as_deref(),
                )
            }
            font::Mapping::Composite(map) => {
                let (code, valid) = map.next(input, work, self.limits)?;
                let mut cid = if valid {
                    map.cid(code, work, self.limits)?
                } else {
                    None
                };
                let unicode = cid.and_then(|cid| self.cid_unicode.as_ref()?.get(cid));
                if cid.is_some_and(|cid| self.lacks_gid_entry(cid)) {
                    cid = Some(0);
                } else if cid.is_none_or(|cid| !self.has_cid(cid)) {
                    cid = Some(map.notdef(code, work, self.limits)?);
                }
                let cid = cid.filter(|cid| self.has_cid(*cid)).unwrap_or(0);
                (code, cid, None, unicode)
            }
        };
        let unicode = self
            .to_unicode
            .as_ref()
            .and_then(|map| map.text(code))
            .map(str::to_owned)
            .or_else(|| fallback.map(str::to_owned));
        Ok(Glyph {
            code: code.value,
            byte_len: code.len,
            advance: self.metrics.width(selector),
            vertical: (self.writing_mode == WritingMode::Vertical)
                .then(|| self.metrics.vertical(selector)),
            word_space: code.len == 1 && code.value == 32,
            unicode,
            shape: self.shape(code, selector, name, fallback),
        })
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tolerance_tests;
