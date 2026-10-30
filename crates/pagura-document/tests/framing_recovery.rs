#![forbid(unsafe_code)]
//! Framing repairs identify bounded structure, never arbitrary scan prefixes.
use pagura_core::{ErrorKind, Limits, ObjectId, Position, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;

#[test]
fn absent_header_keeps_bytes_and_reports_strict_refusal() {
    for name in ["header-absent", "header-absent-binary-comment"] {
        let bytes = fixture("recovery_document", &format!("{name}.pdf"));
        let error = Document::open(
            bytes.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.repair.unwrap().as_str(), "file.header-absent");
        assert_eq!(error.position, Position::File(0));
        let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
        assert_eq!(document.page_count().unwrap(), 1);
        assert_eq!(document.original_bytes(), bytes);
        assert_eq!(document.diagnostics().len(), 1);
    }
}

#[test]
fn a_missing_header_requires_a_complete_leading_catalog_and_respects_limits() {
    let bytes = fixture("recovery_document", "header-absent.pdf");
    for prefix in ["%PDF-9.0\n", "%PDF-", "1 0 obj null endobj\n", "garbage\n"] {
        let data = [prefix.as_bytes(), &bytes].concat();
        assert!(
            Document::open(data, &OpenOptions::default()).is_err(),
            "{prefix}"
        );
    }
    let error = Document::open(
        bytes,
        &OpenOptions {
            limits: Limits {
                value_nodes: 0,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}

#[test]
fn an_overlong_length_recovers_only_the_streams_own_terminator() {
    for name in [
        "stream-length-crosses-next-object",
        "stream-length-crosses-xref",
        "stream-length-crosses-EOF",
    ] {
        let bytes = fixture("recovery_document", &format!("{name}.pdf"));
        let strict = Document::open(
            bytes.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..Default::default()
            },
        )
        .unwrap();
        let error = strict.object(ObjectId::new(4, 0)).unwrap_err();
        assert_eq!(error.repair.unwrap().as_str(), "stream.length-corrected");
        let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
        let object = document.object(ObjectId::new(4, 0)).unwrap();
        assert_eq!(
            document.stream_data(object).unwrap().as_ref(),
            b"1 0 0 rg 8 8 48 32 re f\n"
        );
        assert!(
            document
                .object(ObjectId::new(5, 0))
                .unwrap()
                .value()
                .is_null()
        );
        assert_eq!(document.original_bytes(), bytes);
    }
}
