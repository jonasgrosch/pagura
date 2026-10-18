//! Positioned text, geometric layout policy and search (9.10, 14.8, 14.9.4).
//!
//! Content interpretation belongs exclusively to `pagura-render`. These types
//! consume its records, never content streams. Quads are selection cells, not
//! exact ink outlines. Layout is policy, not a claim of tagged-PDF reading order.
use crate::{FontKind, WritingMode};
use pagura_core::{Error, Limits, Name, ObjectId, Result};
use pagura_raster::Point;
use std::{ops::Range, sync::Arc};

mod layout;
mod search;
#[cfg(test)]
mod tests;
pub use search::{SearchOptions, TextMatch};

/// A selection quadrilateral in default user space, in cell-corner order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad(pub [Point; 4]);

/// Page-local identity of a selected font. Direct dictionaries have no object ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFont {
    /// Indirect PDF font dictionary, when available.
    pub object: Option<ObjectId>,
    /// Font subtype.
    pub kind: FontKind,
    /// PDF BaseFont name, when present.
    pub base_name: Option<Name>,
}

/// One marked-content occurrence. IDs distinguish repeated tags and MCIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedContent {
    /// Page-local occurrence ID.
    pub id: usize,
    /// BMC/BDC tag.
    pub tag: Name,
    /// Local marked-content identifier, when specified.
    pub mcid: Option<i64>,
    /// Exact replacement text (14.9.4), including an explicitly empty string.
    pub actual_text: Option<String>,
    /// Alternate description (14.9.3), used when ActualText is absent.
    pub alt: Option<String>,
}

/// A marked-content span, including spans containing no text-showing operation.
#[derive(Debug, Clone)]
pub struct MarkedSpan {
    /// Properties of this occurrence.
    pub content: Arc<MarkedContent>,
    /// Ancestors, outermost first.
    pub ancestors: Arc<[Arc<MarkedContent>]>,
    /// Half-open range of source glyphs enclosed by this occurrence.
    pub glyphs: Range<usize>,
    /// Whether optional content hides the occurrence at its start.
    pub hidden: bool,
}

/// A glyph in content order, before layout, in default user space (9.4.4).
#[derive(Debug, Clone)]
pub struct PositionedGlyph {
    /// ToUnicode or glyph-name Unicode, without a guessed value on failure.
    pub unicode: Option<String>,
    /// Index in [`PositionedText::fonts`].
    pub font: usize,
    /// Text-state font size before the text matrix and CTM.
    pub size: f64,
    /// Horizontal or vertical writing.
    pub writing_mode: WritingMode,
    /// Metric selection cell transformed into default user space.
    pub quad: Quad,
    /// Baseline origin, including text rise, before glyph-origin adjustment.
    pub origin: Point,
    /// Unit-em vector in the writing direction, transformed by Trm.
    pub axis: Point,
    /// Length of the transformed perpendicular em vector.
    pub em: f64,
    /// End of the intrinsic advance, excluding Tc and Tw.
    pub advance_end: Point,
    /// Text rendering mode 0–7; mode 3 remains extractable.
    pub render_mode: u8,
    /// Hidden by optional content. This is independent of render mode/clipping.
    pub hidden: bool,
    /// Marked-content path, outermost first.
    pub marked_content: Arc<[Arc<MarkedContent>]>,
    /// Individual string operand identity, including strings in TJ arrays.
    pub show: usize,
}

/// Separate extraction output; never retained in a rendering display list.
#[derive(Debug, Clone, Default)]
pub struct PositionedText {
    /// Fonts selected by the content interpreter.
    pub fonts: Vec<TextFont>,
    /// Glyphs in content-stream order, including unknown and hidden glyphs.
    pub glyphs: Vec<PositionedGlyph>,
    /// Balanced marked-content occurrences, including empty replacements.
    pub marked: Vec<MarkedSpan>,
}

/// Explicit, deterministic layout policy, independent of rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutOptions {
    /// Include optional-content-hidden glyphs. Default false; mode 3 is included.
    pub include_hidden: bool,
    /// Minimum unoccupied inline gap, in ems, that inserts a space. Default .2.
    pub space_gap: f64,
    /// Maximum perpendicular baseline displacement within a line, in ems (.35).
    pub baseline_tolerance: f64,
    /// Consecutive line distance above this many ems starts a paragraph (1.8).
    pub paragraph_gap: f64,
}
impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            space_gap: 0.2,
            baseline_tolerance: 0.35,
            paragraph_gap: 1.8,
        }
    }
}

/// One semantic character. Unknown Unicode is `None`, exported as U+FFFD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Character {
    /// Unicode scalar; no character code is guessed when Unicode is missing.
    pub unicode: Option<char>,
    /// Source glyphs; replacement text can refer to several glyphs.
    pub glyphs: Range<usize>,
    /// Whether layout inserted this separator, rather than decoding it.
    pub inserted: bool,
    /// UTF-8 byte range in the corresponding content or reading-order string.
    pub range: Range<usize>,
}

/// An exported text line; its range excludes the trailing LF separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    /// UTF-8 byte range in [`TextPage::text`].
    pub range: Range<usize>,
    /// Word indices contained in this line.
    pub words: Range<usize>,
    /// Whether a paragraph break follows this line.
    pub paragraph_end: bool,
}

/// A word under the alphanumeric/underscore policy (not Unicode UAX #29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextWord {
    /// UTF-8 byte range in [`TextPage::text`].
    pub range: Range<usize>,
    /// Line index.
    pub line: usize,
}

/// Text in content order and policy reading order, with highlight geometry.
///
/// Layout is reader policy (9.10; 14.8), not structure-tree reconstruction.
/// Outermost marked occurrences keep content order; consecutive ActualText
/// replacements stay together. Unmarked blocks group by one-degree orientation
/// in discovery order, cluster inline starts within 1.5 em into columns, then
/// sort perpendicular baselines descending. Horizontal columns run left to
/// right; vertical columns run right to left. Unpositioned replacements anchor
/// content order and attach without geometric separators.
///
/// Direction dot products above .98 and the configured baseline tolerance join
/// glyphs into lines; inline displacements over 2.5 em split fragments. Positive
/// intrinsic-advance gaps above `space_gap` insert spaces. Large perpendicular
/// gaps (`paragraph_gap`), column shifts over 2.5 em and changed orientations
/// separate paragraphs. Ems are transformed perpendicular font-size vectors.
/// Negative sizes and rotations retain their signed writing directions.
///
/// Outermost ActualText/Alt replaces its associated text; ActualText wins over
/// Alt, including empty values (14.9.3–14.9.4). Alt supplies word breaks.
/// ReversedChars reverses individual shown strings (14.8.2.5.3); that sequence
/// relies on explicit whitespace. Replacement characters share all associated
/// source quads; graphics-only replacements have none. Unknown Unicode exports
/// U+FFFD while retaining `None`, a search barrier. Hidden optional content is
/// excluded by default; OCR, clip-only and clipped text remain extractable.
#[derive(Debug, Clone)]
pub struct TextPage {
    positioned: Arc<PositionedText>,
    content: Vec<Character>,
    content_text: String,
    reading: Vec<Character>,
    text: String,
    lines: Vec<TextLine>,
    words: Vec<TextWord>,
    limits: Limits,
}

impl TextPage {
    /// Lays out bounded interpreter output. Content order is retained separately.
    ///
    /// # Errors
    /// Returns invalid-argument errors for nonfinite geometry/options, or limit
    /// errors before exceeding text, work or estimated output-storage budgets.
    pub fn from_positioned(
        text: PositionedText,
        options: LayoutOptions,
        limits: Limits,
    ) -> Result<Self> {
        layout::build(Arc::new(text), options, limits)
    }
    /// Repeats layout with a different policy while sharing the original records.
    ///
    /// # Errors
    /// As [`Self::from_positioned`].
    pub fn with_layout(&self, options: LayoutOptions) -> Result<Self> {
        layout::build(Arc::clone(&self.positioned), options, self.limits)
    }
    /// Unicode characters in content order, after replacements and ReversedChars.
    pub fn characters(&self) -> &[Character] {
        &self.content
    }
    /// Characters in reading order, including inserted spaces and separators.
    pub fn reading_characters(&self) -> &[Character] {
        &self.reading
    }
    /// Original glyph records, in content order.
    pub fn glyphs(&self) -> &[PositionedGlyph] {
        &self.positioned.glyphs
    }
    /// Selected font identities.
    pub fn fonts(&self) -> &[TextFont] {
        &self.positioned.fonts
    }
    /// Original marked-content occurrences.
    pub fn marked_content(&self) -> &[MarkedSpan] {
        &self.positioned.marked
    }
    /// Plain-text export. Geometric lines use LF, paragraphs two LFs; replacement
    /// strings keep their literal whitespace.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Semantic text in content order, without inferred geometric separators.
    pub fn content_text(&self) -> &str {
        &self.content_text
    }
    /// Exported lines and their byte/word ranges.
    pub fn lines(&self) -> &[TextLine] {
        &self.lines
    }
    /// Word byte ranges under the documented boundary policy.
    pub fn words(&self) -> &[TextWord] {
        &self.words
    }
    /// Default search options, convenient for callers using inferred page types.
    pub fn search_options(&self) -> SearchOptions {
        SearchOptions::default()
    }
}

fn charge(total: &mut usize, amount: usize, maximum: usize) -> Result<()> {
    *total = total
        .checked_add(amount)
        .filter(|n| *n <= maximum)
        .ok_or_else(|| Error::limit("text extraction budget exceeded"))?;
    Ok(())
}
