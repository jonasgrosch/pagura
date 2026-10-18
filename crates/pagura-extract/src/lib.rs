#![forbid(unsafe_code)]
//! PDF page text: positioned text, geometric layout policy, search and the
//! logical structure view (9.10, 14.7–14.9; ADR 0014, ADR 0025).
//!
//! Content interpretation belongs exclusively to `pagura-render`. These types
//! consume its records, never content streams. Quads are selection cells, not
//! exact ink outlines. Page layout is reader policy; logical structure is a
//! separate assistive-technology view and never changes page search or selection.
use pagura_core::{Error, Limits, Name, ObjectId, Result};
use pagura_raster::Point;
use pagura_text::{FontKind, WritingMode};
use std::{ops::Range, sync::Arc};

mod layout;
mod memory;
pub mod repairs;
mod structure;
pub use structure::{StructureElement, StructureNamespace};
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
    /// Page or form whose StructParents indexes this MCID (14.7.5.4).
    pub container: Option<ObjectId>,
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

/// One XObject occurrence or annotation, for structure OBJR content (14.7.5.3).
#[derive(Debug, Clone)]
pub struct TextObject {
    /// Marked-content context of the invocation.
    pub marked_content: Arc<[Arc<MarkedContent>]>,
    /// Referenced object.
    pub object: ObjectId,
    /// Source glyphs from this invocation.
    pub glyphs: Range<usize>,
    /// Whether optional content hides this invocation.
    pub hidden: bool,
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
    /// Object occurrences, including non-text graphics and annotations.
    pub objects: Vec<TextObject>,
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
    /// UTF-8 byte range in the corresponding content, page or logical string.
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

/// Assistive-technology text in logical /K order (14.7–14.8).
///
/// Independent of page layout options: inline separators use the default
/// geometric thresholds and optional-content-hidden items remain excluded.
/// Structure artifacts are excluded and structure ActualText/Alt applies here,
/// leaving page search and selection unchanged (14.8.2.2, 14.9.3–14.9.4).
#[derive(Debug)]
pub struct LogicalText {
    text: String,
    characters: Vec<Character>,
    structure: Vec<StructureElement>,
    retained: usize,
}
impl LogicalText {
    /// Logical text, with policy separators between structure blocks.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Logical characters, with ranges into this text and the page's glyphs.
    pub fn characters(&self) -> &[Character] {
        &self.characters
    }
    /// Structure elements in preorder, with ranges into this logical text.
    pub fn structure(&self) -> &[StructureElement] {
        &self.structure
    }
}

/// Text in content order and policy reading order, with highlight geometry.
///
/// Page layout is reader policy (9.10; 14.8), including artifacts.
/// [`Self::with_structure`] adds a separate logical view for tagged documents;
/// it never changes page text, search or selection.
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
/// Outermost marked-content ActualText/Alt replaces its text; ActualText wins over
/// Alt, including empty values (14.9.3–14.9.4). Alt supplies word breaks.
/// ReversedChars reverses individual shown strings (14.8.2.5.3); that sequence
/// relies on explicit whitespace. Replacement characters share all associated
/// source quads; graphics-only replacements have none. Unknown Unicode exports
/// U+FFFD while retaining `None`, a search barrier. Hidden optional content is
/// excluded by default; OCR, clip-only and clipped text remain extractable.
#[derive(Debug)]
pub struct TextPage {
    positioned: Arc<PositionedText>,
    content: Vec<Character>,
    content_text: String,
    reading: Vec<Character>,
    text: String,
    lines: Vec<TextLine>,
    words: Vec<TextWord>,
    limits: Limits,
    retained: usize,
    source_retained: usize,
    options: LayoutOptions,
    logical: Option<Arc<LogicalText>>,
    structure_fallback: Option<Error>,
}

impl TextPage {
    /// Estimated retained heap, computed once from capacities. Includes inline
    /// quads in glyph records, all strings and vectors, and Arc allocation
    /// headers. Shared paths and marked occurrences are counted once per page.
    /// Allocator bookkeeping is excluded. Independently cached views each charge
    /// their complete shared positioned source and logical allocation; it never depends on
    /// a changing Arc reference count. The inline TextPage itself is excluded.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// Assistive-technology view, independent of page layout options.
    /// None means no structure tree, or a reported structure fallback.
    pub fn logical(&self) -> Option<&LogicalText> {
        self.logical.as_deref()
    }

    /// Convenience access to the logical view's structure elements.
    /// Element ranges refer to [`LogicalText::text`], never page copy text.
    pub fn structure(&self) -> Option<&[StructureElement]> {
        self.logical().map(LogicalText::structure)
    }

    /// Original structure failure when repair mode produced no logical view.
    /// The document also reports `text.structure-fallback`, preserving the
    /// original reason and any unsupported-feature tag. No heap is retained.
    pub fn structure_fallback(&self) -> Option<&Error> {
        self.structure_fallback.as_ref()
    }

    /// Adds logical /K order, cross-checking content references against the
    /// ParentTree (14.7.5). Artifacts are excluded; structure ActualText/Alt
    /// replaces logical descendants (14.9.3–14.9.4). The entire page view
    /// remains intact, including search, selection and artifacts.
    ///
    /// # Errors
    /// Malformed or unsupported structure reports `text.structure-fallback`:
    /// repair mode returns no logical view; strict mode rejects the repair.
    /// Optional hierarchy traversal stops at the depth cap with the same
    /// reported fallback. Parser, capture, reference-tree and work/storage
    /// budgets and caller errors remain errors in both modes. No
    /// partial logical page is returned.
    pub fn with_structure(
        mut self,
        document: &pagura_document::Document,
        page: &pagura_document::Page,
    ) -> Result<Self> {
        let owned = document.page(page.index())?;
        if owned.id() != page.id()
            || !std::ptr::eq(owned.resources(), page.resources())
            || owned != page
        {
            return Err(Error::invalid_argument(
                "page does not belong to structure document",
            ));
        }
        self.logical = None;
        self.structure_fallback = None;
        match structure::apply(&self, document, page) {
            Ok(logical) => self.logical = logical.map(Arc::new),
            Err(failure) => {
                let error = failure.error;
                if !failure.recoverable {
                    return Err(error);
                }
                repairs::report(
                    document,
                    pagura_core::Diagnostic::new(repairs::STRUCTURE_FALLBACK, error.message)
                        .in_object_opt(error.object)
                        .at(error.position),
                )?;
                self.structure_fallback = Some(error);
            }
        }
        self.retained = memory::estimate(&self);
        if self.retained > self.limits.document_bytes {
            return Err(Error::limit("logical text retained budget exceeded"));
        }
        Ok(self)
    }

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
        layout::build(Arc::new(text), options, limits, None)
    }
    /// Explicitly chooses geometric layout, sharing the original records.
    /// The logical view and fallback cause are preserved. Logical separators
    /// always use default thresholds and hidden items stay excluded, regardless
    /// of the options of the page view.
    ///
    /// # Errors
    /// As [`Self::from_positioned`].
    pub fn with_layout(&self, options: LayoutOptions) -> Result<Self> {
        let mut page = layout::build(
            Arc::clone(&self.positioned),
            options,
            self.limits,
            Some(self.source_retained),
        )?;
        page.logical.clone_from(&self.logical);
        page.structure_fallback.clone_from(&self.structure_fallback);
        page.retained = memory::estimate(&page);
        if page.retained > page.limits.document_bytes {
            return Err(Error::limit("text page retained budget exceeded"));
        }
        Ok(page)
    }
    /// Characters in content order, after marked-content replacements and
    /// ReversedChars. Structure-level replacements affect only the logical view.
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
    /// Content-order text after marked-content replacements, without inferred
    /// separators. Structure-level replacements affect only the logical view.
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
