#![forbid(unsafe_code)]
//! Source-bound scalar recovery through the public document API.

use pagura_core::{Diagnostic, ErrorKind, Limits, ObjectId, Position, Recovery, Value};
use pagura_document::{Document, ObjectSource, OpenOptions, repairs};
use pagura_testkit::fixture;

const CONTENT: &[u8] = b"0 0 1 rg 0 0 32 32 re f 1 0 0 rg 4 4 8 8 re f 0 1 0 rg 20 20 4 4 re f";

fn open(name: &str, recovery: Recovery) -> Document {
    pagura_testkit::open_fixture("integer_separator", &format!("{name}.pdf"), |bytes| {
        Document::open(
            bytes,
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )
    })
}

#[test]
fn exact_scalar_source_position_and_cache_are_preserved() {
    let bytes = fixture("integer_separator", "joined.pdf");
    let marker = bytes
        .windows(8)
        .position(|part| part == b"5 0 obj\n")
        .unwrap()
        + 8;
    let end = marker + CONTENT.len().to_string().len();
    let document = open("joined", Recovery::Repair);
    assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
    let scalar = document.object(ObjectId::new(5, 0)).unwrap();
    assert_eq!(
        scalar.value(),
        &Value::Integer(i64::try_from(CONTENT.len()).unwrap())
    );
    assert!(std::ptr::eq(scalar, document.object(scalar.id()).unwrap()));
    let ObjectSource::File(range) = scalar.source() else {
        panic!("physical scalar required")
    };
    assert_eq!(&bytes[range.clone()], &bytes[marker - 8..end + 6]);
    let stream = document.object(ObjectId::new(4, 0)).unwrap();
    for _ in 0..2 {
        assert_eq!(&*document.stream_data(stream).unwrap(), CONTENT);
        assert_eq!(document.original_bytes(), bytes);
        let diagnostics = document.diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].repair(), Some(repairs::INTEGER_SEPARATOR));
        assert_eq!(diagnostics[0].position, Position::File(end));
        assert_eq!(diagnostics[0].object, Some(scalar.id()));
    }
    let strict = open("joined", Recovery::Strict);
    for id in [ObjectId::new(5, 0), ObjectId::new(4, 0)] {
        let error = strict.object(id).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.repair, Some(repairs::INTEGER_SEPARATOR));
        assert_eq!(error.position, Position::File(end));
        assert_eq!(error.object, Some(ObjectId::new(5, 0)));
        assert_eq!(strict.diagnostics(), [] as [Diagnostic; 0]);
    }
}

#[test]
fn noninteger_values_and_fake_markers_are_not_reinterpreted() {
    let document = open("array", Recovery::Repair);
    assert!(matches!(
        document.object(ObjectId::new(5, 0)).unwrap().value(),
        Value::Array(_)
    ));
    assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
    for name in ["string", "hex-string", "name"] {
        let document = open(name, Recovery::Repair);
        // The name has no following mandatory endobj; strings remain strings.
        if name != "name" {
            let value = document.object(ObjectId::new(5, 0)).unwrap().value();
            assert!(matches!(value, Value::String(_)));
        }
        assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
        assert!(document.object(ObjectId::new(4, 0)).is_err());
    }
    for name in [
        "negative",
        "positive-sign",
        "real",
        "exponent",
        "reference",
        "comment",
        "overflow",
        "trailing-token",
        "corrected-offset",
        "overlapping-span",
        "unterminated-comment-boundary",
    ] {
        let document = open(name, Recovery::Repair);
        assert_eq!(
            document.object(ObjectId::new(5, 0)).unwrap_err().kind,
            ErrorKind::Syntax,
            "{name}"
        );
        assert_eq!(document.diagnostics(), [] as [Diagnostic; 0], "{name}");
    }
    for name in ["overlapping-span", "unterminated-comment-boundary"] {
        let bytes = fixture("integer_separator", &format!("{name}.pdf"));
        let start = bytes
            .windows(8)
            .position(|part| part == b"5 0 obj\n")
            .unwrap()
            + 8;
        let document = open(name, Recovery::Repair);
        let error = document.object(ObjectId::new(5, 0)).unwrap_err();
        assert_eq!(error.object, Some(ObjectId::new(5, 0)));
        assert_eq!(error.position, Position::File(start));
        assert_eq!(error.repair, None);
        assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
    }
}

#[test]
fn generation_and_revision_identity_cannot_be_rescued_by_other_values() {
    let document = open("joined-generation", Recovery::Repair);
    assert_eq!(
        document.object(ObjectId::new(5, 0)).unwrap_err().kind,
        ErrorKind::MissingObject
    );
    assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
    assert!(matches!(
        document.object(ObjectId::new(5, 2)).unwrap().value(),
        Value::Integer(_)
    ));
    let document = open("incremental-old-joined-current-clean", Recovery::Repair);
    assert_eq!(document.revisions().len(), 2);
    document.object(ObjectId::new(5, 0)).unwrap();
    assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
    let error = document
        .object_at_revision(1, ObjectId::new(5, 0))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.repair, Some(repairs::INTEGER_SEPARATOR));
    assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
    let document = open("incremental-current-joined", Recovery::Repair);
    document.object(ObjectId::new(5, 0)).unwrap();
    assert!(matches!(
        document
            .object_at_revision(1, ObjectId::new(5, 0))
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(_)
    ));
}

#[test]
fn integer_and_stream_resource_guards_remain_authoritative() {
    let document = open("integer-max", Recovery::Repair);
    assert_eq!(
        document.object(ObjectId::new(5, 0)).unwrap().value(),
        &Value::Integer(i64::MAX)
    );
    assert_eq!(
        document.object(ObjectId::new(4, 0)).unwrap_err().kind,
        ErrorKind::Limit
    );
    let zero = open("joined-zero", Recovery::Repair);
    assert_eq!(
        zero.object(ObjectId::new(5, 0)).unwrap().value(),
        &Value::Integer(0)
    );
    assert!(
        zero.stream_data(zero.object(ObjectId::new(4, 0)).unwrap())
            .unwrap()
            .is_empty()
    );
    for limits in [
        Limits {
            stream_bytes: CONTENT.len() - 1,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: CONTENT.len() - 1,
            ..Limits::default()
        },
    ] {
        let document = Document::open(
            fixture("integer_separator", "joined.pdf"),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let result = document
            .object(ObjectId::new(4, 0))
            .and_then(|object| document.stream_data(object));
        assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
    }
    let document = open("length-cycle", Recovery::Repair);
    assert_eq!(
        document.object(ObjectId::new(4, 0)).unwrap_err().kind,
        ErrorKind::Cycle
    );
    assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
}
