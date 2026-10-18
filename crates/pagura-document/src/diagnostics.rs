//! Bounded document diagnostics, independent of the number of decode attempts.

use pagura_core::{Diagnostic, DiagnosticKind, ObjectId, Position};
use std::collections::BTreeSet;

pub(crate) const CAPACITY: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum At {
    Unknown,
    File(usize),
    Stream(ObjectId, usize),
    Buffer(usize),
}

impl From<Position> for At {
    fn from(position: Position) -> Self {
        match position {
            Position::Unknown => Self::Unknown,
            Position::File(offset) => Self::File(offset),
            Position::Stream { object, offset } => Self::Stream(object, offset),
            Position::Buffer(offset) => Self::Buffer(offset),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Log {
    pub(crate) items: Vec<Diagnostic>,
    keys: BTreeSet<(DiagnosticKind, Option<ObjectId>, At)>,
    pub(crate) suppressed: usize,
}

impl Log {
    pub(crate) fn push(&mut self, diagnostic: Diagnostic) {
        let key = (
            diagnostic.kind,
            diagnostic.object,
            At::from(diagnostic.position),
        );
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

#[cfg(test)]
mod tests {
    use super::CAPACITY;
    use crate::{Document, OpenOptions};
    use pagura_core::{Diagnostic, ObjectId, Position, Repair};
    use pagura_testkit::fixture;

    const TAG: Repair = Repair::new("test.repair");

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
            Diagnostic {
                message: "second message",
                ..diagnostic.clone()
            },
        ]);
        document.report([
            diagnostic.clone().in_object(ObjectId::new(1, 0)),
            diagnostic.clone().at(Position::Buffer(1)),
            diagnostic.clone().at(Position::Stream {
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
}
