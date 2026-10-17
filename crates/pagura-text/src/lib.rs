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

use pagura_core::{Dictionary, Error, Feature, ObjectId, Result};
use pagura_document::Document;
use pagura_font::{FontProgram, GlyphId};
use pagura_raster::Transform;
use std::sync::Arc;

/// Stable tags for text capabilities not implemented yet.
pub mod features {
    use pagura_core::Feature;

    /// A font whose program is not embedded and has no substitute yet.
    pub const NON_EMBEDDED: Feature = Feature::new("font.non-embedded");
    /// A predefined CMap other than Identity-H/V, whose data is not bundled.
    pub const PREDEFINED_CMAP: Feature = Feature::new("font.predefined-cmap");
}

/// Placeholder tag while the font layer is under construction. Must not
/// survive the implementation.
const PENDING: Feature = Feature::new("text.pending");

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
    /// An outline glyph of an embedded font program.
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
}

impl PdfFont {
    /// Loads the font dictionary `font` (already resolved) of `document`.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for malformed dictionaries, `Unsupported`
    /// (for example [`features::NON_EMBEDDED`]) for fonts that cannot be
    /// drawn yet, and errors from loading the font program.
    pub fn load(document: &Document, font: &Dictionary) -> Result<Self> {
        let _ = (document, font);
        Err(Error::unsupported(
            PENDING,
            "PDF fonts are not implemented yet",
        ))
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
        None
    }

    /// Splits a string operand into glyphs (clauses 9.4.3 and 9.7.6).
    ///
    /// # Errors
    ///
    /// Returns syntax errors for codes outside the font's code space that
    /// recovery cannot map, and errors from loading glyph data.
    pub fn decode(&self, string: &[u8]) -> Result<Vec<Glyph>> {
        let _ = string;
        Err(Error::unsupported(
            PENDING,
            "text decoding is not implemented yet",
        ))
    }
}
