//! The one reporting path for font loading (ADR 0003; CONTRIBUTING 4).
use crate::repairs::TOUNICODE_INVALID;
use pagura_core::{Diagnostic, Diagnostics, Error, ObjectId, Position, Recovery, Repair, Result};
use pagura_document::Document;

/// Routes diagnostics through a sink in the document's recovery mode, one call
/// at a time, so malformed maps cannot grow a temporary diagnostic vector
/// beyond the document's bounded, deduplicating sink. Without a document
/// (standalone CMap parsing) every repair is rejected as in strict mode.
#[derive(Clone, Copy)]
pub(crate) struct Report<'a>(Option<&'a Document>);

impl<'a> Report<'a> {
    pub(crate) fn new(document: &'a Document) -> Self {
        Self(Some(document))
    }

    pub(crate) fn strict() -> Self {
        Self(None)
    }

    /// Reports a repair at `position`, recording its stream as the object.
    pub(crate) fn repair(
        self,
        tag: Repair,
        message: &'static str,
        position: Position,
    ) -> Result<()> {
        self.diagnostic(
            Diagnostic::new(tag, message)
                .at(position)
                .in_object_opt(position.object()),
        )
    }

    /// Reports a repair or an informational diagnostic. Strict mode rejects
    /// repairs and records information.
    pub(crate) fn diagnostic(self, diagnostic: Diagnostic) -> Result<()> {
        self.diagnostics([diagnostic])
    }

    /// Reports diagnostics collected by a nested parser.
    pub(crate) fn diagnostics(self, items: impl IntoIterator<Item = Diagnostic>) -> Result<()> {
        let mut sink = Diagnostics::new(self.0.map_or(Recovery::Strict, Document::recovery));
        sink.absorb(items)?;
        if let Some(document) = self.0 {
            document.report(sink.take());
        }
        Ok(())
    }

    /// Discards an unusable ToUnicode map in repair mode. Strict mode keeps
    /// the original error kind, position and refused repair.
    pub(crate) fn discarded(self, error: Error, object: Option<ObjectId>) -> Result<()> {
        if self
            .0
            .is_none_or(|document| document.recovery() == Recovery::Strict)
        {
            return Err(error);
        }
        self.diagnostic(
            Diagnostic::new(
                error.repair.unwrap_or(TOUNICODE_INVALID),
                "invalid ToUnicode data discarded; glyph selection is unchanged",
            )
            .in_object_opt(error.object.or(object))
            .at(error.position),
        )
    }
}
