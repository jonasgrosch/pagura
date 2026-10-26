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
            Diagnostic::information(TAG.as_str(), "more information").at(Position::File(offset)),
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
        assert_eq!(document.diagnostics(), std::slice::from_ref(&dependency));
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
    assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
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
    assert_eq!(strict.diagnostics(), [] as [Diagnostic; 0]);
}
