//! Decoded glyphs: what the renderer and text extraction receive (9.4.3).
use pagura_core::ObjectId;
use pagura_font::{FontProgram, GlyphId};
use pagura_raster::Transform;
use std::sync::Arc;

/// Vertical metrics of one glyph (clause 9.7.4.3, `/W2`, `/DW2`).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct VerticalMetrics {
    /// Vertical displacement, in text space.
    pub advance: f64,
    /// Position vector from the horizontal to the vertical origin, in text space.
    pub origin: (f64, f64),
}

/// How a glyph is drawn.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
#[non_exhaustive]
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

impl Glyph {
    /// A glyph without vertical metrics, word spacing or known text; the
    /// public fields set the rest. Decoding builds glyphs itself; this is for
    /// callers that need one, such as tests of glyph consumers.
    pub fn new(code: u32, byte_len: u8, advance: f64, shape: GlyphShape) -> Self {
        Self {
            code,
            byte_len,
            advance,
            vertical: None,
            word_space: false,
            unicode: None,
            shape,
        }
    }
}
