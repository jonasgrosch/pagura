#![forbid(unsafe_code)]
//! Deterministic writing, verified by reopening and rendering.

use pagura::{Document, OpenOptions, Pixmap, Rgba8};
use pagura_testkit::{assert_bytes_eq, fixture};

fn open(bytes: impl Into<std::sync::Arc<[u8]>>) -> pagura::Result<Document> {
    Document::open(bytes, &OpenOptions::default())
}

fn ppm(pixmap: &Pixmap) -> Vec<u8> {
    pagura_testkit::ppm(
        pixmap.width(),
        pixmap.height(),
        &pixmap.to_rgb_over(Rgba8::WHITE),
    )
}
use pagura::{Builder, Dictionary, ErrorKind, Name, NewObject, ObjectId, RenderOptions, Value};

fn dictionary(entries: &[(&[u8], Value)]) -> Dictionary {
    entries
        .iter()
        .map(|(key, value)| (Name::new(key), value.clone()))
        .collect()
}

fn reference(number: u32) -> Value {
    Value::Reference(ObjectId::new(number, 0))
}

const ROOT: ObjectId = ObjectId::new(1, 0);

fn builder() -> pagura::Result<Builder> {
    let mut builder = Builder::default();
    let media_box = Value::Array([0, 0, 64, 64].map(Value::Integer).to_vec());
    let objects = [
        (
            1,
            dictionary(&[(b"Type", Value::name(b"Catalog")), (b"Pages", reference(3))]),
        ),
        (
            3,
            dictionary(&[
                (b"Type", Value::name(b"Pages")),
                (b"Kids", Value::Array(vec![reference(5)])),
                (b"Count", Value::Integer(1)),
            ]),
        ),
        (
            5,
            dictionary(&[
                (b"Type", Value::name(b"Page")),
                (b"Parent", reference(3)),
                (b"MediaBox", media_box),
                (b"Contents", reference(7)),
            ]),
        ),
    ];
    for (number, value) in objects {
        builder.insert(ObjectId::new(number, 0), NewObject::value(value.into()))?;
    }
    let content = NewObject::stream(Dictionary::new(), b"1 0 0 rg 8 8 48 32 re f\n".to_vec());
    builder.insert(ObjectId::new(7, 0), content)?;
    Ok(builder)
}

#[test]
fn deterministic_writer_handles_sparse_ids_and_reopens() {
    let builder = builder().unwrap();
    let first = builder.finish(ROOT).unwrap();
    assert_eq!(first, builder.finish(ROOT).unwrap());
    let document = open(first).unwrap();
    let pixmap = pagura::render_page(
        &document,
        document.page(0).unwrap(),
        &RenderOptions::default(),
    )
    .unwrap();
    assert_bytes_eq(
        &ppm(&pixmap),
        &fixture("classic", "rectangle.ppm"),
        "rewritten rectangle",
    );
}

#[test]
fn dictionary_entries_keep_insertion_order() {
    let output = builder().unwrap().finish(ROOT).unwrap();
    let text = String::from_utf8_lossy(&output);
    let page = text
        .find("/Type /Page /Parent 3 0 R /MediaBox [0 0 64 64] /Contents 7 0 R")
        .unwrap();
    assert!(page > 0);
}

#[test]
fn duplicate_ids_and_dangling_references_fail() {
    let mut builder = builder().unwrap();
    let error = builder
        .insert(ROOT, NewObject::value(Value::Null))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
    builder
        .insert(ObjectId::new(9, 0), NewObject::value(reference(99)))
        .unwrap();
    assert_eq!(
        builder.finish(ROOT).unwrap_err().kind,
        ErrorKind::MissingObject
    );
}

#[test]
fn invalid_numeric_values_fail_before_success() {
    let mut builder = builder().unwrap();
    builder
        .insert(ObjectId::new(9, 0), NewObject::value(Value::Real(f64::NAN)))
        .unwrap();
    assert_eq!(
        builder.finish(ROOT).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn oversized_value_graph_is_rejected_before_serialization() {
    let limits = pagura::Limits {
        value_nodes: 1000,
        ..pagura::Limits::default()
    };
    let mut builder = Builder::new(limits);
    builder
        .insert(
            ROOT,
            NewObject::value(Value::Array(vec![Value::Null; 1001])),
        )
        .unwrap();
    assert_eq!(
        builder.finish(ROOT).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn inserting_many_objects_is_linear() {
    let mut builder = Builder::default();
    for number in 1..=200_000 {
        builder
            .insert(ObjectId::new(number, 0), NewObject::value(Value::Null))
            .unwrap();
    }
}
