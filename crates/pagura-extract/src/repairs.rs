//! Reported page-text repairs (ADR 0003). They never change glyph painting.
use pagura_core::{Diagnostic, Diagnostics, Repair, Result};
use pagura_document::Document;

/// Unusable marked-content property lists or ActualText, Alt or E values
/// are ignored (14.6.2; 14.9.3–14.9.5).
/// This never supplies a guessed replacement or changes glyph painting.
pub const MARKED_PROPERTY: Repair = Repair::new("text.marked-property");

/// Logical structure is unusable: the page has no logical view, and its page
/// text is unaffected (14.7–14.8; reader recovery policy, ADR 0003 and ADR 0014).
pub const STRUCTURE_FALLBACK: Repair = Repair::new("text.structure-fallback");

/// Every repair tag defined by this crate.
pub const ALL: &[Repair] = &[MARKED_PROPERTY, STRUCTURE_FALLBACK];

/// Reports one repair under the document's recovery mode.
pub(crate) fn report(document: &Document, diagnostic: Diagnostic) -> Result<()> {
    let mut sink = Diagnostics::new(document.recovery());
    sink.repair(diagnostic)?;
    document.report(sink.take());
    Ok(())
}
