//! Bounded document diagnostics, independent of the number of decode attempts.

use crate::Document;
use crate::source::{ReadFault, ReadResult};
use pagura_core::{Diagnostic, DiagnosticKind, Diagnostics, ObjectId, Position, Recovery, Result};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc, sync::PoisonError};

pub(crate) const CAPACITY: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct Log {
    pub(crate) items: Vec<Diagnostic>,
    keys: BTreeSet<(DiagnosticKind, Option<ObjectId>, Position)>,
    pub(crate) suppressed: usize,
}

impl Log {
    pub(crate) fn push(&mut self, diagnostic: Diagnostic) {
        let key = (diagnostic.kind, diagnostic.object, diagnostic.position);
        if self.keys.contains(&key) {
            return;
        }
        if self.items.len() == CAPACITY {
            self.suppressed = self.suppressed.saturating_add(1);
            return;
        }
        self.keys.insert(key);
        self.items.push(diagnostic);
    }
}

/// Provisional reports from one semantic read attempt.
///
/// The buffer uses the document log's same 1024-entry cap, identity deduplication
/// and suppression count. A missing dependency discards only this attempt's
/// reports; completed dependencies can still publish their own reports. Dropping
/// or panicking out of the callback publishes nothing. No global state or
/// document lock is held while the callback reads dependencies.
#[derive(Debug)]
pub struct DiagnosticBatch<'a> {
    document: &'a Document,
    pending: Rc<RefCell<Log>>,
}

impl<'a> DiagnosticBatch<'a> {
    /// Executes a semantic read and publishes its diagnostics on success or
    /// terminal rejection. `Unavailable` publishes none, so a later retry starts
    /// without reports from an incomplete interpretation. The result is unchanged.
    ///
    /// Fixed per-attempt storage is bounded by the existing document log cap;
    /// it is separate from source and caller-owned result storage budgets.
    ///
    /// # Errors
    /// Returns the callback's exact source wait or terminal error.
    pub fn run<T>(
        document: &'a Document,
        read: impl FnOnce(&Self) -> ReadResult<T>,
    ) -> ReadResult<T> {
        document.check_source()?;
        let batch = Self::new(document);
        let result = read(&batch).and_then(|value| {
            document.check_source()?;
            Ok(value)
        });
        if !matches!(&result, Err(ReadFault::Unavailable(_))) {
            let pending = std::mem::take(&mut *batch.pending.borrow_mut());
            let mut target = document
                .diagnostics
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for diagnostic in pending.items {
                target.push(diagnostic);
            }
            target.suppressed = target.suppressed.saturating_add(pending.suppressed);
        }
        result
    }

    pub(crate) fn new(document: &'a Document) -> Self {
        Self {
            document,
            pending: Rc::new(RefCell::new(Log::default())),
        }
    }

    /// One inspection can read historical documents under their own recovery
    /// policies. Only the initiating run publishes this shared pending buffer;
    /// a fork never owns a publication boundary or rolls back dependency logs.
    pub(crate) fn for_document<'b>(&self, document: &'b Document) -> DiagnosticBatch<'b> {
        DiagnosticBatch {
            document,
            pending: Rc::clone(&self.pending),
        }
    }

    /// The document's recovery policy for nested parsers.
    pub fn recovery(&self) -> Recovery {
        self.document.recovery()
    }

    /// Records one repair or information item under the existing recovery rule.
    ///
    /// # Errors
    /// Returns a refused repair in strict mode, before retaining that item.
    pub fn record(&self, diagnostic: Diagnostic) -> Result<()> {
        let mut checked = Diagnostics::new(self.recovery());
        checked.repair(diagnostic)?;
        for diagnostic in checked.take() {
            self.pending.borrow_mut().push(diagnostic);
        }
        Ok(())
    }

    /// Records nested parser reports one at a time without an unbounded copy.
    /// The iterator is evaluated outside the buffer borrow.
    ///
    /// # Errors
    /// As [`Self::record`].
    pub fn absorb(&self, diagnostics: impl IntoIterator<Item = Diagnostic>) -> Result<()> {
        for diagnostic in diagnostics {
            self.record(diagnostic)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
