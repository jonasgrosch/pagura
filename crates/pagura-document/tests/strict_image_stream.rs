#![forbid(unsafe_code)]
//! Strict general-filter decoding is local to one requested image stream.

use pagura_core::{ErrorKind, Limits, ObjectId, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;

const IMAGE: ObjectId = ObjectId::new(3, 0);

#[test]
fn complete_general_filters_match_the_existing_entry_point() {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let document = Document::open(
            fixture("image", "samples-1-1-flate.pdf"),
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let object = document.object(IMAGE).unwrap();
        let strict = document
            .image_stream_with_strict_filters(object, 3)
            .unwrap();
        let ordinary = document.image_stream(object).unwrap();
        assert_eq!(strict.data.as_ref(), b"TTT");
        assert_eq!(strict.data, ordinary.data);
        assert!(strict.codec.is_none());
        assert_eq!(
            document.diagnostics(),
            Vec::<pagura_core::Diagnostic>::new()
        );
    }
}

#[test]
fn strict_filter_failure_does_not_report_or_enable_a_repair() {
    let mut bytes = fixture("image", "samples-1-1-flate.pdf");
    let original = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
    let range = original.object(IMAGE).unwrap().stream_range().unwrap();
    bytes[range.end - 1] ^= 1;
    let document = Document::open(
        bytes,
        &OpenOptions {
            recovery: Recovery::Repair,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let object = document.object(IMAGE).unwrap();
    let before = document.diagnostics();
    let error = document
        .image_stream_with_strict_filters(object, 3)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.object, Some(IMAGE));
    assert_eq!(error.repair, Some(pagura_codec::tags::CHECKSUM_MISMATCH));
    assert_eq!(document.diagnostics(), before);
    assert_eq!(document.image_stream(object).unwrap().data.as_ref(), b"TTT");
    assert_eq!(document.diagnostics().len(), before.len() + 1);
}

#[test]
fn strict_filters_keep_the_documents_real_output_cap() {
    let document = Document::open(
        fixture("image", "samples-1-1-flate.pdf"),
        &OpenOptions {
            limits: Limits {
                decoded_bytes: 2,
                ..Limits::default()
            },
            recovery: Recovery::Repair,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let object = document.object(IMAGE).unwrap();
    assert_eq!(
        document
            .image_stream_with_strict_filters(object, 3)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert_eq!(
        document.diagnostics(),
        Vec::<pagura_core::Diagnostic>::new()
    );
}

#[test]
fn the_explicit_output_bound_also_applies_to_unfiltered_rows() {
    let document = Document::open(
        fixture("image", "samples-1-1-raw.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let object = document.object(IMAGE).unwrap();
    assert_eq!(document.image_stream(object).unwrap().data.as_ref(), b"TTT");
    let error = document
        .image_stream_with_strict_filters(object, 2)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.object, Some(IMAGE));
    assert_eq!(
        document.diagnostics(),
        Vec::<pagura_core::Diagnostic>::new()
    );
}
