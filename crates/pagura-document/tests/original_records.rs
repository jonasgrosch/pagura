#![forbid(unsafe_code)]
//! Source-backed reconstruction, immutable identities and aggregate budget contracts.

use pagura_core::{ErrorKind, Limits, ObjectId, Position, Recovery, Value};
use pagura_document::{Document, ObjectSource, OpenOptions, repairs};
use pagura_testkit::fixture;

fn open_bytes(
    bytes: Vec<u8>,
    recovery: Recovery,
    document_bytes: usize,
) -> pagura_core::Result<Document> {
    Document::open(
        bytes,
        &OpenOptions {
            recovery,
            limits: Limits {
                document_bytes,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
}

fn open(name: &str) -> Document {
    pagura_testkit::open_fixture("original_records", &format!("{name}.pdf"), |bytes| {
        open_bytes(bytes, Recovery::Repair, Limits::default().document_bytes)
    })
}

fn has_integer_repair(document: &Document) -> bool {
    document
        .diagnostics()
        .iter()
        .any(|item| item.repair() == Some(repairs::INTEGER_SEPARATOR))
}

#[test]
fn visited_exact_original_scalars_preserve_values_ranges_and_repeated_borrows() {
    for (name, id) in [
        ("undersized-joined", ObjectId::new(5, 0)),
        ("positive-generation", ObjectId::new(5, 7)),
        ("sparse-original", ObjectId::new(1000, 0)),
        ("sparse-original-max", ObjectId::new(u32::MAX, 0)),
        ("omitted-content-joined", ObjectId::new(5, 0)),
        ("comment-suffix-newline", ObjectId::new(5, 0)),
    ] {
        let bytes = fixture("original_records", &format!("{name}.pdf"));
        let document = open(name);
        assert!(document.contains_object(id), "{name}");
        assert_eq!(document.revisions(), [], "{name}");
        let object = document.object(id).unwrap();
        assert_eq!(object.value(), &Value::Integer(22), "{name}");
        let ObjectSource::File(range) = object.source() else {
            panic!("physical source required")
        };
        let source = format!("{} {} obj\n22endobj", id.number, id.generation);
        assert_eq!(&bytes[range.clone()], source.as_bytes(), "{name}");
        let missing_separator = range.end - b"endobj".len();
        assert!(
            document
                .diagnostics()
                .iter()
                .any(|item| item.repair() == Some(repairs::INTEGER_SEPARATOR)
                    && item.object == Some(id)
                    && item.position == Position::File(missing_separator)),
            "{name}"
        );
        let diagnostics = document.diagnostics();
        for _ in 0..3 {
            assert!(std::ptr::eq(object, document.object(id).unwrap()));
            assert_eq!(document.diagnostics(), diagnostics);
            assert_eq!(document.original_bytes(), bytes);
        }
        let xref = bytes
            .windows(b"\nxref\n".len())
            .position(|part| part == b"\nxref\n")
            .unwrap()
            + 1;
        let strict =
            open_bytes(bytes, Recovery::Strict, Limits::default().document_bytes).unwrap_err();
        assert_eq!(strict.kind, ErrorKind::Syntax);
        assert_eq!(
            strict.message,
            "trailer Size does not cover cross-reference entries"
        );
        assert_eq!(strict.position, Position::File(xref));
        assert_eq!(strict.object, None);
        assert_eq!(strict.repair, None);
    }
    let generation = open("positive-generation");
    assert!(!generation.contains_object(ObjectId::new(5, 0)));
    assert_eq!(
        generation.object(ObjectId::new(5, 0)).unwrap_err().kind,
        ErrorKind::MissingObject
    );
}

#[test]
fn absent_nonexact_and_other_fault_records_never_supply_authority() {
    for name in [
        "omitted-scalar-joined",
        "wrong-scalar-offset",
        "generation-mismatch",
        "duplicate-original-record",
        "negative-scalar",
        "real-scalar",
        "reference-scalar",
        "integer-overflow",
        "comment-suffix-unterminated",
        "prev-present",
        "xrefstm-present",
        "encrypt-null",
        "header-extra-format",
        "leading-junk-physical-offsets",
        "leading-junk-relative-offsets",
        "all-free-size-coverage",
    ] {
        let document = open(name);
        let id = ObjectId::new(5, 0);
        assert!(!document.contains_object(id), "{name}");
        for _ in 0..3 {
            let error = document.object(id).unwrap_err();
            assert_eq!(error.kind, ErrorKind::MissingObject, "{name}");
            assert_eq!(error.object, Some(id), "{name}");
            assert_eq!(error.repair, None, "{name}");
        }
        assert!(!has_integer_repair(&document), "{name}");
        assert_eq!(
            document.original_bytes(),
            fixture("original_records", &format!("{name}.pdf"))
        );
    }
    for name in ["array-scalar", "name-scalar", "string-scalar"] {
        let document = open(name);
        let value = document.object(ObjectId::new(5, 0)).unwrap().value();
        match (name, value) {
            ("array-scalar", Value::Array(values)) => assert_eq!(values, &[Value::Integer(21)]),
            ("name-scalar", Value::Name(name)) => assert_eq!(name.as_bytes(), b"21endobj"),
            ("string-scalar", Value::String(bytes)) => assert_eq!(bytes.as_slice(), b"21"),
            _ => panic!("source type must be preserved"),
        }
        assert!(!has_integer_repair(&document));
    }
}

#[test]
fn raw_records_inside_skipped_values_are_not_an_alternate_traversal() {
    for kind in ["stream", "string", "comment", "name"] {
        let name = format!("fake-header-in-{kind}");
        let document = open(&name);
        let id = ObjectId::new(7, 0);
        assert!(!document.contains_object(id), "{kind}");
        assert!(
            !document.object_ids().any(|found| found.number == id.number),
            "{kind}"
        );
        assert_eq!(
            document.object(id).unwrap_err().kind,
            ErrorKind::MissingObject,
            "{kind}"
        );
        assert!(!has_integer_repair(&document), "{kind}");
    }
}

#[test]
fn ordinary_later_definitions_own_the_final_identity_and_source() {
    for (name, generation) in [
        ("joined-then-clean-override", 0),
        ("joined-then-generation-override", 1),
    ] {
        let document = open(name);
        let id = ObjectId::new(5, generation);
        let object = document.object(id).unwrap();
        assert_eq!(object.value(), &Value::Integer(99));
        let ObjectSource::File(range) = object.source() else {
            panic!("physical source required")
        };
        let marker = format!("5 {generation} obj\n99\nendobj");
        assert_eq!(&document.original_bytes()[range.clone()], marker.as_bytes());
        if generation != 0 {
            assert!(!document.contains_object(ObjectId::new(5, 0)));
            assert_eq!(
                document.object(ObjectId::new(5, 0)).unwrap_err().kind,
                ErrorKind::MissingObject
            );
        }
        assert_eq!(document.revisions(), []);
    }
}

#[test]
fn missing_or_nonpositive_noninteger_size_is_not_a_coverage_proof() {
    let source = fixture("original_records", "undersized-joined.pdf");
    let text = String::from_utf8(source).unwrap();
    for size in [
        "",
        "/Size 0",
        "/Size -1",
        "/Size 2.0",
        "/Size null",
        "/Size 5 0 R",
        "/Size /two",
    ] {
        let bytes = text.replace("/Size 2", size).into_bytes();
        let document = open_bytes(
            bytes.clone(),
            Recovery::Repair,
            Limits::default().document_bytes,
        )
        .unwrap();
        assert_eq!(
            document.object(ObjectId::new(5, 0)).unwrap_err().kind,
            ErrorKind::MissingObject,
            "{size}"
        );
        assert!(!has_integer_repair(&document), "{size}");
        assert_eq!(document.original_bytes(), bytes);
    }
}

#[test]
fn an_exact_large_length_retains_its_value_and_the_existing_resource_refusal() {
    let document = open("i64-max-scalar");
    assert_eq!(
        document.object(ObjectId::new(5, 0)).unwrap().value(),
        &Value::Integer(i64::MAX)
    );
    assert!(has_integer_repair(&document));
    let error = document
        .object(ObjectId::new(4, 0))
        .and_then(|object| document.stream_data(object))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.object, Some(ObjectId::new(4, 0)));
    assert_eq!(error.repair, None);
}

fn minimum_budget(bytes: &[u8]) -> usize {
    let mut low = 0;
    let mut high = 16 * 1024;
    assert!(open_bytes(bytes.to_vec(), Recovery::Repair, high).is_ok());
    while high - low > 1 {
        let middle = usize::midpoint(low, high);
        match open_bytes(bytes.to_vec(), Recovery::Repair, middle) {
            Ok(_) => high = middle,
            Err(error) => {
                assert_eq!(error.kind, ErrorKind::Limit);
                low = middle;
            }
        }
    }
    assert!(open_bytes(bytes.to_vec(), Recovery::Repair, high).is_ok());
    assert!(
        matches!(open_bytes(bytes.to_vec(), Recovery::Repair, low), Err(error) if error.kind == ErrorKind::Limit)
    );
    high
}

#[test]
fn useless_all_free_records_do_not_add_a_retained_metadata_fee() {
    let source = fixture("original_records", "all-free-size-coverage.pdf");
    // Correcting only Size leaves the catalog undefined in the primary all-free
    // index, so this independent control still takes the ordinary reconstruction
    // path, with no Size-coverage evidence. Input bytes are caller-owned copies.
    let control = String::from_utf8(source.clone())
        .unwrap()
        .replace("/Size 2", "/Size 16")
        .into_bytes();
    assert_eq!(minimum_budget(&source), minimum_budget(&control));
}

#[test]
fn source_evidence_never_fabricates_a_historical_revision() {
    let document = open("undersized-joined");
    assert_eq!(document.revisions(), []);
    for _ in 0..2 {
        assert_eq!(
            document
                .object_at_revision(0, ObjectId::new(5, 0))
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArgument
        );
    }
}

#[test]
fn retained_original_evidence_and_lazy_values_obey_one_aggregate_budget() {
    for (name, id) in [("undersized-joined", 5), ("sparse-original-max", u32::MAX)] {
        let bytes = fixture("original_records", &format!("{name}.pdf"));
        let read = |budget| -> pagura_core::Result<Document> {
            let document = open_bytes(bytes.clone(), Recovery::Repair, budget)?;
            assert_eq!(
                document.object(ObjectId::new(id, 0))?.value(),
                &Value::Integer(22)
            );
            let stream = document.object(ObjectId::new(4, 0))?;
            assert_eq!(
                document.stream_data(stream)?.as_ref(),
                b"1 0 0 rg 4 4 8 8 re f\n"
            );
            Ok(document)
        };
        let mut low = 0;
        let mut high = 16 * 1024;
        assert!(read(high).is_ok());
        while high - low > 1 {
            let middle = usize::midpoint(low, high);
            match read(middle) {
                Ok(_) => high = middle,
                Err(error) => {
                    assert_eq!(error.kind, ErrorKind::Limit);
                    low = middle;
                }
            }
        }
        let document = read(high).unwrap();
        assert_eq!(document.original_bytes(), bytes);
        assert!(has_integer_repair(&document));
        let error = read(low).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.repair, None);
    }
}
