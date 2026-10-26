//! The one reporting path for font loading (ADR 0003; CONTRIBUTING 4).
use crate::repairs::TOUNICODE_INVALID;
use pagura_core::{Diagnostic, Diagnostics, Error, ObjectId, Position, Recovery, Repair, Result};
use pagura_document::DiagnosticBatch;

/// Routes reports into the font read attempt's bounded provisional sink.
/// Standalone CMap parsing has no document and rejects every repair.
#[derive(Clone, Copy)]
pub(crate) struct Report<'a>(Option<&'a DiagnosticBatch<'a>>);

impl<'a> Report<'a> {
    pub(crate) fn new(batch: &'a DiagnosticBatch<'a>) -> Self {
        Self(Some(batch))
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
        match self.0 {
            Some(batch) => batch.absorb(items),
            None => Diagnostics::new(Recovery::Strict).absorb(items),
        }
    }

    /// Discards an unusable ToUnicode map in repair mode. Strict mode keeps
    /// the original error kind, position and refused repair.
    pub(crate) fn discarded(self, error: Error, object: Option<ObjectId>) -> Result<()> {
        if self
            .0
            .is_none_or(|batch| batch.recovery() == Recovery::Strict)
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
