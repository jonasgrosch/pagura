//! Reported text repairs (ADR 0003). ToUnicode only controls extraction
//! (9.10.3); malformed maps still require permission to repair or discard.
use pagura_core::{Diagnostic, Diagnostics, Error, ObjectId, Position, Recovery, Repair, Result};
use pagura_document::Document;

/// Missing embedded CIDFontType2 CIDToGIDMap uses Identity (9.7.4.1, Table 115).
pub const CID_TO_GID_IDENTITY: Repair = Repair::new("font.cid-to-gid-identity");
/// Faulty extraction-only map is discarded (9.10.3).
pub const TOUNICODE_INVALID: Repair = Repair::new("font.tounicode-invalid");
/// Simple-font two-byte ToUnicode sources use their low byte (9.10.3).
pub const TOUNICODE_SIMPLE_CODES: Repair = Repair::new("font.tounicode-simple-codes");
/// Invalid UTF-16BE destinations are omitted (9.10.3).
pub const TOUNICODE_UTF16: Repair = Repair::new("font.tounicode-utf16");
/// Overflowing bfrange destinations increment with byte carry (9.10.3).
pub const TOUNICODE_RANGE: Repair = Repair::new("font.tounicode-range");
/// CMap blocks above 100 entries are accepted within budgets (9.7.5.4; TN5014 7.4).
pub const CMAP_BLOCK_COUNT: Repair = Repair::new("font.cmap-block-count");
/// Malformed byte rectangles use disjoint integer-interval pieces (9.7.6.2).
pub const CMAP_CODESPACE: Repair = Repair::new("font.cmap-codespace");
/// Invalid optional FontWeight is ignored when selecting a substitute
/// (9.8.1, Table 120); normal weight and the independent ForceBold flag apply.
pub const DESCRIPTOR_WEIGHT: Repair = Repair::new("font.descriptor-weight");

/// Unusable marked-content property lists or ActualText, Alt or E values
/// are ignored (14.6.2; 14.9.3–14.9.5).
/// This never supplies a guessed replacement or changes glyph painting.
pub const MARKED_PROPERTY: Repair = Repair::new("text.marked-property");

/// Logical structure is unusable: the page has no logical view, and its page
/// text is unaffected (14.7–14.8; reader recovery policy, ADR 0003 and ADR 0014).
pub const STRUCTURE_FALLBACK: Repair = Repair::new("text.structure-fallback");

/// Reports one item at a time so malformed maps cannot grow a temporary
/// diagnostic vector beyond Document's bounded/deduplicating sink.
#[derive(Clone, Copy)]
pub(crate) struct Report<'a>(Option<&'a Document>);

impl<'a> Report<'a> {
    pub(crate) fn new(document: &'a Document) -> Self {
        Self(Some(document))
    }
    pub(crate) fn strict() -> Self {
        Self(None)
    }
    pub(crate) fn repair(
        self,
        tag: Repair,
        message: &'static str,
        position: Position,
    ) -> Result<()> {
        let mut diagnostic = Diagnostic::new(tag, message).at(position);
        if let Position::Stream { object, .. } = position {
            diagnostic = diagnostic.in_object(object);
        }
        self.diagnostic(diagnostic)
    }
    pub(crate) fn diagnostic(self, diagnostic: Diagnostic) -> Result<()> {
        let mut sink = Diagnostics::new(self.0.map_or(Recovery::Strict, Document::recovery));
        sink.repair(diagnostic)?;
        if let Some(document) = self.0 {
            document.report(sink.take());
        }
        Ok(())
    }
    pub(crate) fn discarded(self, error: Error, object: Option<ObjectId>) -> Result<()> {
        // Preserve the original error kind, position and refused repair in Strict.
        if self
            .0
            .is_none_or(|document| document.recovery() == Recovery::Strict)
        {
            return Err(error);
        }
        let diagnostic = Diagnostic::new(
            error.repair.unwrap_or(TOUNICODE_INVALID),
            "invalid ToUnicode data discarded; glyph selection is unchanged",
        )
        .in_object_opt(error.object.or(object))
        .at(error.position);
        self.diagnostic(diagnostic)
    }
}
