//! Bounded document diagnostics, independent of the number of decode attempts.

use crate::Document;
use crate::source::{ReadFault, ReadResult};
use pagura_core::{Diagnostic, DiagnosticKind, Diagnostics, ObjectId, Position, Recovery, Result};
use std::{cell::RefCell, collections::BTreeSet, sync::PoisonError};

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
    pending: RefCell<Log>,
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
        let batch = Self {
            document,
            pending: RefCell::new(Log::default()),
        };
        let result = read(&batch);
        if !matches!(&result, Err(ReadFault::Unavailable(_))) {
            let pending = batch.pending.into_inner();
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
mod tests {
    use super::{CAPACITY, DiagnosticBatch};
    use crate::repairs;
    use crate::{Document, OpenOptions};
    use pagura_core::{Diagnostic, ObjectId, Position, Repair};
    use pagura_testkit::fixture;

    const TAG: Repair = repairs::TRAILING_BYTES;

    #[test]
    fn repeated_decoding_retains_one_diagnostic_identity() {
        let document = Document::open(
            fixture("document", "diagnostics-repeated-decode.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let stream = document.object(ObjectId::new(4, 0)).unwrap();
        for _ in 0..100 {
            assert_eq!(
                document.stream_data(stream).unwrap().as_ref(),
                b"1 0 0 rg 8 8 48 32 re f\n"
            );
        }
        assert_eq!(document.diagnostics().len(), 1);
        assert_eq!(document.suppressed_diagnostics(), 0);
    }

    #[test]
    fn reports_deduplicate_identity_and_make_truncation_visible() {
        let document = Document::open(
            fixture("document", "xref-stream.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let diagnostic = Diagnostic::new(TAG, "first message").at(Position::File(1));
        document.report([
            diagnostic.clone(),
            Diagnostic::new(TAG, "second message").at(Position::File(1)),
        ]);
        // Builders keep the innermost context, so each identity is built afresh.
        document.report([
            diagnostic.clone().in_object(ObjectId::new(1, 0)),
            Diagnostic::new(TAG, "first message").at(Position::Buffer(1)),
            Diagnostic::new(TAG, "first message").at(Position::Stream {
                object: ObjectId::new(1, 0),
                offset: 1,
            }),
        ]);
        assert_eq!(document.diagnostics().len(), 4);
        assert_eq!(document.diagnostics()[0], diagnostic);
        document.report(
            (2..CAPACITY + 20)
                .map(|offset| Diagnostic::new(TAG, "more repairs").at(Position::File(offset))),
        );
        assert_eq!(document.diagnostics().len(), CAPACITY);
        assert_eq!(document.suppressed_diagnostics(), 22);
        document.report([diagnostic]);
        assert_eq!(document.suppressed_diagnostics(), 22);
        document.report([Diagnostic::new(TAG, "omitted").at(Position::File(CAPACITY + 100))]);
        assert_eq!(document.suppressed_diagnostics(), 23);
    }

    #[test]
    fn opening_reports_share_the_same_identity_set_and_suppression_saturates() {
        let document = Document::open(
            fixture("document", "header-text.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let initial = document.diagnostics();
        assert_eq!(initial.len(), 1);
        document.report(initial.clone());
        assert_eq!(document.diagnostics(), initial);
        let mut log = super::Log::default();
        for offset in 0..CAPACITY {
            log.push(Diagnostic::new(TAG, "damage").at(Position::File(offset)));
        }
        log.suppressed = usize::MAX;
        log.push(Diagnostic::new(TAG, "omitted").at(Position::File(CAPACITY)));
        assert_eq!(log.suppressed, usize::MAX);
    }

    #[test]
    fn reporting_iterators_can_reenter_the_document() {
        let document = Document::open(
            fixture("document", "xref-stream.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        document.report(std::iter::once_with(|| {
            document.report([Diagnostic::new(TAG, "inner")]);
            Diagnostic::new(TAG, "outer").at(Position::File(1))
        }));
        assert_eq!(document.diagnostics().len(), 2);
    }
    #[test]
    fn information_and_repairs_share_the_cap_but_have_distinct_identities() {
        let mut log = super::Log::default();
        let info = Diagnostic::information(TAG.as_str(), "information").at(Position::File(1));
        let repair = Diagnostic::new(TAG, "repair").at(Position::File(1));
        for diagnostic in [info.clone(), repair.clone(), info.clone(), repair.clone()] {
            log.push(diagnostic);
        }
        assert_eq!(log.items, [info, repair]);
        for offset in 2..CAPACITY {
            log.push(
                Diagnostic::information(TAG.as_str(), "more information")
                    .at(Position::File(offset)),
            );
        }
        assert_eq!(log.items.len(), CAPACITY);
        log.push(Diagnostic::new(TAG, "omitted repair").at(Position::File(CAPACITY)));
        assert_eq!(log.suppressed, 1);
    }
    #[test]
    fn provisional_reports_wait_without_rolling_back_completed_dependencies() {
        use crate::source::{ByteSource, SourceIdentity, SourceLimits, SparseBytes};
        let document = Document::open(
            fixture("document", "diagnostics-repeated-decode.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let identity = SourceIdentity::new(36, [17; 32]);
        let source =
            SparseBytes::new(identity, 4, &document.limits(), &SourceLimits::default()).unwrap();
        // The independently authored fixture is 24 content bytes in ASCIIHex,
        // followed by '>' and surplus data: the first trailing byte is 49.
        let dependency = Diagnostic::new(
            pagura_codec::tags::TRAILING_DATA,
            "data after filter end marker",
        )
        .at(Position::Buffer(49))
        .in_object(ObjectId::new(4, 0));
        let provisional = Diagnostic::new(TAG, "provisional semantic repair");
        let cached = document.object(ObjectId::new(4, 0)).unwrap();
        for _ in 0..3 {
            let waiting = DiagnosticBatch::run(&document, |batch| {
                batch.record(provisional.clone())?;
                let object = document.object(ObjectId::new(4, 0))?;
                assert!(std::ptr::eq(object, cached));
                assert_eq!(
                    document.stream_data(object)?.as_ref(),
                    b"1 0 0 rg 8 8 48 32 re f\n"
                );
                source.materialize()
            })
            .unwrap_err();
            assert!(matches!(waiting, crate::source::ReadFault::Unavailable(_)));
            assert_eq!(document.diagnostics(), [dependency.clone()]);
            assert_eq!(document.suppressed_diagnostics(), 0);
        }
        source.supply(identity, 0, b"done").unwrap();
        for _ in 0..3 {
            let complete = DiagnosticBatch::run(&document, |batch| {
                batch.record(provisional.clone())?;
                document.stream_data(document.object(ObjectId::new(4, 0))?)?;
                source.materialize()
            })
            .unwrap();
            assert_eq!(complete.bytes(), b"done");
            assert_eq!(
                document.diagnostics(),
                [dependency.clone(), provisional.clone()]
            );
            assert_eq!(document.suppressed_diagnostics(), 0);
        }
    }

    #[test]
    fn batches_share_strict_rejection_identity_and_bounded_suppression() {
        use crate::source::{ByteSource, SourceIdentity, SourceLimits, SparseBytes};
        use pagura_core::{Error, Recovery};
        let document = Document::open(
            fixture("document", "xref-stream.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let source = SparseBytes::new(
            SourceIdentity::new(37, [19; 32]),
            1,
            &document.limits(),
            &SourceLimits::default(),
        )
        .unwrap();
        DiagnosticBatch::run(&document, |batch| {
            batch.absorb(
                (0..CAPACITY + 3)
                    .map(|offset| Diagnostic::new(TAG, "damage").at(Position::File(offset))),
            )?;
            source.materialize()
        })
        .unwrap_err();
        assert!(document.diagnostics().is_empty());
        assert_eq!(document.suppressed_diagnostics(), 0);
        let terminal = Error::syntax("complete semantic rejection");
        let result: crate::source::ReadResult<()> = DiagnosticBatch::run(&document, |batch| {
            batch.absorb(
                (0..CAPACITY + 3)
                    .map(|offset| Diagnostic::new(TAG, "damage").at(Position::File(offset))),
            )?;
            Err(terminal.clone().into())
        });
        assert_eq!(result.unwrap_err().as_error(), Some(&terminal));
        let expected: Vec<_> = (0..CAPACITY)
            .map(|offset| Diagnostic::new(TAG, "damage").at(Position::File(offset)))
            .collect();
        assert_eq!(document.diagnostics(), expected);
        assert_eq!(document.suppressed_diagnostics(), 3);
        let strict = Document::open(
            fixture("document", "xref-stream.pdf"),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let diagnostic = Diagnostic::new(TAG, "refused repair").at(Position::File(9));
        let result: crate::source::ReadResult<()> = DiagnosticBatch::run(&strict, |batch| {
            batch.record(diagnostic.clone())?;
            Ok(())
        });
        assert_eq!(
            result.unwrap_err().as_error(),
            diagnostic.to_error().as_ref()
        );
        assert!(strict.diagnostics().is_empty());
    }
}
