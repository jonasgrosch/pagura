#![forbid(unsafe_code)]
//! Independently authored input syntax and saved revision/form contracts.

use pagura::editing::{SaveOptions, SignaturePolicy, Transaction, features, save_filled_form};
use pagura::forms::{ButtonValue, ChoiceSelection, FieldValue, FillValue};
use pagura::{
    Document, Error, ErrorKind, Limits, Name, NewObject, ObjectId, OpenOptions, Recovery,
    RenderOptions, Rgba8, Value,
};
use pagura_testkit::fixture;

fn open(bytes: Vec<u8>) -> Document {
    Document::open(
        bytes,
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap()
}

// Source syntax is independent of pagura-write. A nonzero generation, custom
// trailer entry and Info/ID exercise preservation that a new-file builder lacks.
fn source() -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (number, generation, value) in [
        (1, 0, "<< /Type /Catalog /Pages 2 0 R >>"),
        (2, 0, "<< /Type /Pages /Kids [3 7 R] /Count 1 >>"),
        (
            3,
            7,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 60] >>",
        ),
        (4, 0, "<< /Producer (independent) >>"),
    ] {
        offsets.push((bytes.len(), generation));
        bytes.extend_from_slice(format!("{number} {generation} obj\n{value}\nendobj\n").as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for (offset, generation) in offsets {
        bytes.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R /Info 4 0 R /ID [<0102><0304>] /Private (keep) >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn exported(name: &str, bytes: &[u8]) {
    if let Some(directory) = std::env::var_os("PAGURA_EDIT_OUTPUT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{name}.pdf")), bytes).unwrap();
    }
}

#[test]
fn original_prefix_generations_trailer_and_repeated_revisions_survive() {
    let original = source();
    let document = open(original.clone());
    let mut transaction = Transaction::new(&document);
    transaction
        .edit_dictionary(ObjectId::new(3, 7), |dictionary| {
            dictionary.insert(Name::new(b"Rotate"), Value::Integer(90));
            Ok(())
        })
        .unwrap();
    let added = transaction
        .add(NewObject::Value(Value::String(b"new".to_vec())))
        .unwrap();
    assert_eq!(added, ObjectId::new(5, 0));
    let saved = transaction.commit(&SaveOptions::default()).unwrap();
    assert_eq!(saved, transaction.commit(&SaveOptions::default()).unwrap());
    assert!(saved.starts_with(&original));
    assert_eq!(document.revisions().len(), 1);
    let reopened = open(saved.clone());
    assert_eq!(reopened.revisions().len(), 2);
    assert_eq!(
        reopened.trailer().get(b"Private"),
        document.trailer().get(b"Private")
    );
    assert_eq!(
        reopened.trailer().get(b"Info"),
        document.trailer().get(b"Info")
    );
    assert_eq!(
        reopened.trailer().get(b"Prev").and_then(Value::as_integer),
        Some(i64::try_from(document.revisions()[0].offset()).unwrap())
    );
    let ids = reopened.trailer().get(b"ID").unwrap().as_array().unwrap();
    assert_eq!(ids[0], Value::String(vec![1, 2]));
    assert_ne!(ids[1], Value::String(vec![3, 4]));
    assert_eq!(
        reopened
            .object(ObjectId::new(3, 7))
            .unwrap()
            .dictionary()
            .unwrap()
            .get(b"Rotate"),
        Some(&Value::Integer(90))
    );
    assert!(reopened.object(ObjectId::new(3, 0)).is_err());
    assert_eq!(
        reopened
            .object_at_revision(1, ObjectId::new(3, 7))
            .unwrap()
            .dictionary()
            .unwrap()
            .get(b"Rotate"),
        None
    );
    let mut second = Transaction::new(&reopened);
    assert_eq!(
        second.add(Value::Integer(42).into()).unwrap(),
        ObjectId::new(6, 0)
    );
    let twice = second.commit(&SaveOptions::default()).unwrap();
    assert!(twice.starts_with(&saved));
    assert_eq!(open(twice.clone()).revisions().len(), 3);
    exported("incremental", &twice);
}

#[test]
fn rollback_failed_sets_and_dangling_references_are_atomic() {
    let original = source();
    let document = open(original.clone());
    let mut transaction = Transaction::new(&document);
    assert_eq!(
        transaction.commit(&SaveOptions::default()).unwrap(),
        original
    );
    let id = transaction.add(Value::Integer(42).into()).unwrap();
    assert_eq!(
        transaction
            .replace(id, Value::Real(f64::NAN).into())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        transaction.get(id),
        Some(&NewObject::Value(Value::Integer(42)))
    );
    assert_eq!(
        transaction
            .replace(ObjectId::new(3, 0), Value::Null.into())
            .unwrap_err()
            .kind,
        ErrorKind::MissingObject
    );
    transaction
        .edit_dictionary(ObjectId::new(4, 0), |dictionary| {
            dictionary.insert(Name::new(b"Producer"), Value::Null);
            Err(Error::invalid_argument("intentional rollback"))
        })
        .unwrap_err();
    assert!(transaction.get(ObjectId::new(4, 0)).is_none());
    let reference = transaction.add(Value::Reference(id).into()).unwrap();
    transaction.reset(id);
    assert_eq!(
        transaction
            .commit(&SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::MissingObject
    );
    transaction.reset(reference);
    assert!(transaction.is_empty());
    assert_eq!(transaction.rollback().original_bytes(), original);
}

#[test]
fn stream_and_hybrid_sources_keep_untouched_compressed_objects() {
    for name in [
        "xref-stream",
        "object-stream",
        "object-stream-flate",
        "hybrid-precedence",
    ] {
        let original = fixture("document", &format!("{name}.pdf"));
        if let Some(directory) = std::env::var_os("PAGURA_EDIT_OUTPUT") {
            let directory = std::path::PathBuf::from(directory).join("originals");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(format!("{name}.pdf")), &original).unwrap();
        }
        let document = open(original.clone());
        let root = document
            .trailer()
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();
        let mut transaction = Transaction::new(&document);
        transaction
            .edit_dictionary(root, |dictionary| {
                dictionary.insert(Name::new(b"Lang"), Value::String(b"en-US".to_vec()));
                Ok(())
            })
            .unwrap();
        let saved = transaction.commit(&SaveOptions::default()).unwrap();
        assert!(saved.starts_with(&original));
        let reopened = open(saved.clone());
        assert_eq!(
            reopened.catalog().unwrap().get(b"Lang"),
            Some(&Value::String(b"en-US".to_vec()))
        );
        assert_eq!(
            reopened.pages().unwrap().len(),
            document.pages().unwrap().len()
        );
        assert!(!reopened.revisions()[0].trailer().contains_key(b"XRefStm"));
        assert!(reopened.revisions()[0].trailer().contains_key(b"W"));
        assert_eq!(reopened.revisions()[0].kind(), pagura_document::RevisionKind::Stream);
        exported(name, &saved);
    }
}

#[test]
fn modified_repair_mode_and_encrypted_sources_are_refused_but_noop_is_exact() {
    let original = source();
    let repaired = Document::open(
        original.clone(),
        &OpenOptions {
            recovery: Recovery::Repair,
            ..Default::default()
        },
    )
    .unwrap();
    let mut transaction = Transaction::new(&repaired);
    assert_eq!(
        transaction.commit(&SaveOptions::default()).unwrap(),
        original
    );
    transaction.add(Value::Null.into()).unwrap();
    assert_eq!(
        transaction
            .commit(&SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::REPAIRED)
    );
    let original = fixture("crypto", "r3-rc4-128-password-user.pdf");
    let document = Document::open(
        original.clone(),
        &OpenOptions {
            password: b"user-secret".to_vec(),
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let mut transaction = Transaction::new(&document);
    assert_eq!(
        transaction.commit(&SaveOptions::default()).unwrap(),
        original
    );
    transaction.add(Value::Null.into()).unwrap();
    assert_eq!(
        transaction
            .commit(&SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::ENCRYPTED)
    );
}

#[test]
fn output_and_revision_limits_do_not_publish_partial_saves() {
    let original = source();
    let document = Document::open(
        original.clone(),
        &OpenOptions {
            recovery: Recovery::Strict,
            limits: Limits {
                file_bytes: original.len(),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let mut transaction = Transaction::new(&document);
    let id = transaction.add(Value::Integer(42).into()).unwrap();
    assert_eq!(
        transaction
            .commit(&SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert!(transaction.get(id).is_some());
    assert_eq!(document.original_bytes(), original);
    let document = Document::open(
        original,
        &OpenOptions {
            recovery: Recovery::Strict,
            limits: Limits {
                revisions: 1,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let mut transaction = Transaction::new(&document);
    transaction.add(Value::Null.into()).unwrap();
    assert_eq!(
        transaction
            .commit(&SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn informational_diagnostics_do_not_change_save_permission() {
    let document = open(source());
    document.report([pagura::Diagnostic::information(
        "font.substituted",
        "test information",
    )]);
    let mut transaction = Transaction::new(&document);
    transaction.add(Value::Integer(42).into()).unwrap();
    let saved = transaction.commit(&SaveOptions::default()).unwrap();
    assert_eq!(open(saved).revisions().len(), 2);
}

#[test]
fn signatures_require_explicit_policy_and_original_signed_bytes_survive() {
    let original = fixture("cms-signatures", "rsa-sha256-ski.pdf");
    let document = open(original.clone());
    let mut transaction = Transaction::new(&document);
    transaction.add(Value::Integer(123).into()).unwrap();
    assert_eq!(
        transaction
            .commit(&SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::SIGNED)
    );
    let saved = transaction
        .commit(&SaveOptions {
            signatures: SignaturePolicy::PreserveSignedBytes,
        })
        .unwrap();
    assert!(saved.starts_with(&original));
    let reopened = open(saved);
    let signatures = pagura::signatures(&reopened).unwrap();
    assert_eq!(
        signatures
            .fields
            .iter()
            .filter(|field| field.signature.is_some())
            .count(),
        1
    );
    assert!(signatures.fields.iter().filter_map(|field| field.signature.as_ref()).all(|signature| signature.results.integrity == pagura::signatures::Integrity::Verified));
}

fn assert_form_roundtrip(name: &str, input: FillValue<'_>) -> (Vec<u8>, Document) {
    let original = fixture("forms", &format!("{name}.pdf"));
    let document = open(original.clone());
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, input).unwrap();
    let before = pagura::compile_filled_page(&filler, document.page(0).unwrap())
        .unwrap()
        .render(&RenderOptions::default())
        .unwrap()
        .to_rgb_over(Rgba8::WHITE);
    let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
    assert!(saved.starts_with(&original));
    assert!(filler.get(0).is_some());
    let reopened = open(saved.clone());
    let after = pagura::render_page(
        &reopened,
        reopened.page(0).unwrap(),
        &RenderOptions::default(),
    )
    .unwrap()
    .to_rgb_over(Rgba8::WHITE);
    assert_eq!(before, after, "{name}: persisted appearance differs");
    exported(name, &saved);
    (saved, reopened)
}

#[test]
fn text_merged_widgets_existing_appearances_and_need_appearances_roundtrip() {
    for name in [
        "render-text",
        "render-text-update",
        "render-text-need-appearances",
    ] {
        let (_, document) = assert_form_roundtrip(name, FillValue::Text("ABC"));
        let form = document.acro_form().unwrap().unwrap();
        assert!(!form.need_appearances);
        assert!(matches!(&form.fields[0].value, Some(FieldValue::Text(value)) if value == "ABC"));
    }
}

#[test]
fn choices_use_display_values_and_sorted_indices() {
    let (_, document) = assert_form_roundtrip(
        "render-choice-list-multi",
        FillValue::Choice(ChoiceSelection::Indices(&[2, 0])),
    );
    let form = document.acro_form().unwrap().unwrap();
    let field = &form.fields[0];
    assert_eq!(
        field.dictionary.get(b"I"),
        Some(&Value::Array(vec![Value::Integer(0), Value::Integer(2)]))
    );
    assert!(matches!(field.dictionary.get(b"V"), Some(Value::Array(values)) if values.len() == 2));
}

#[test]
fn checkbox_and_radio_states_and_appearances_survive() {
    for name in ["render-checkbox", "render-radio"] {
        let (_, document) = assert_form_roundtrip(
            name,
            FillValue::Button(ButtonValue::Widget {
                index: 0,
                state: None,
            }),
        );
        let form = document.acro_form().unwrap().unwrap();
        assert!(
            form.widgets[0]
                .appearance_state
                .is_some_and(|name| name.as_bytes() != b"Off")
        );
    }
}

#[test]
fn password_and_missing_button_state_appearances_fail_atomically() {
    let document = open(fixture("forms", "text-password.pdf"));
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("secret")).unwrap();
    assert_eq!(
        save_filled_form(&filler, &SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::PASSWORD)
    );
    assert!(filler.get(0).is_some());
    let document = open(fixture("forms", "render-checkbox-generated.pdf"));
    let mut filler = document.form_filler().unwrap().unwrap();
    filler
        .set(
            0,
            FillValue::Button(ButtonValue::Widget {
                index: 0,
                state: None,
            }),
        )
        .unwrap();
    assert_eq!(
        save_filled_form(&filler, &SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::BUTTON_APPEARANCE)
    );
}
