//! Reported text repairs (ADR 0003). ToUnicode is extraction-only: rejecting a
//! repair discards that map, never the font's drawable glyphs (9.10.3).
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
    pub(crate) fn discarded(self, error: &Error, object: Option<ObjectId>) {
        let Some(document) = self.0 else {
            return;
        };
        let diagnostic = Diagnostic {
            repair: error.repair.unwrap_or(TOUNICODE_INVALID),
            message: "invalid ToUnicode data discarded; glyph selection is unchanged",
            position: error.position,
            object: error.object.or(object),
        };
        // Discarding optional extraction data is permitted even in Strict mode.
        // No rejected interpretation is applied; the tag records its rejection.
        document.report([diagnostic]);
    }
}
