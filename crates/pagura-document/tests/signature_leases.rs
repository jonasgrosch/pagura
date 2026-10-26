//! Signature byte ranges retain the independently authored original source.

use pagura_core::{Limits, Recovery};
use pagura_document::source::{
    ByteSource, LoadingLimits, SourceIdentity, SourceLimits, SparseBytes,
};
use pagura_document::{OpenJob, OpenOptions, Opening};
use pagura_testkit::fixture;
use std::sync::Arc;

#[test]
fn checked_signature_ranges_outlive_source_close_and_document() {
    // The authored structure.expect fixes two ranges: 0+401 and 443+268.
    let expected = fixture("signatures", "structure.pdf");
    assert_eq!(expected.len(), 711);
    let identity = SourceIdentity::new(919, [31; 32]);
    let source = Arc::new(
        SparseBytes::new(identity, 711, &Limits::default(), &SourceLimits::default()).unwrap(),
    );
    source.supply(identity, 0, &expected).unwrap();
    let mut opening = OpenJob::new(
        source.clone(),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
        LoadingLimits::default(),
        pagura_document::Validation::CompleteDocument,
    )
    .unwrap();
    let mut ready = None;
    for _ in 0..16 {
        match opening.advance().unwrap() {
            Opening::Yielded => {}
            Opening::Ready(document) => {
                ready = Some(document);
                break;
            }
            state => panic!("unexpected opening state: {state:?}"),
        }
    }
    let document = ready.unwrap();
    let signatures = document.signatures().unwrap();
    let ranges = signatures.fields[0]
        .signature
        .as_ref()
        .unwrap()
        .byte_range
        .clone()
        .unwrap();
    assert_eq!(ranges.ranges(), &[0..401, 443..711]);
    drop(signatures);
    source.close();
    assert!(document.signatures().is_err());
    drop(document);
    drop(opening);
    let mut chunks = ranges.signed_bytes();
    assert_eq!(chunks.next(), Some(&expected[..401]));
    assert_eq!(chunks.next(), Some(&expected[443..711]));
    assert_eq!(chunks.next(), None);
    drop(chunks);
    assert!(source.retained_bytes() > 0);
    drop(ranges);
    assert_eq!(source.retained_bytes(), 0);
}
