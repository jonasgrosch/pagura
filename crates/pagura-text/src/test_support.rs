//! Fixture access and oracle parsing shared by unit tests.
use pagura_core::{
    Dictionary, Error, Limits, ObjectId, Result, Value,
    syntax::{Origin, Parser},
};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;

/// Opens `text/<name>.pdf` in repair mode.
pub(crate) fn document(name: &str) -> Result<Document> {
    Document::open(
        fixture("text", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
}

/// The array oracle `text/<name>.out`, in PDF syntax.
pub(crate) fn expected(name: &str) -> Result<Vec<Value>> {
    let data = fixture("text", &format!("{name}.out"));
    match Parser::new(&data, Origin::Buffer, Limits::default()).parse()? {
        Value::Array(values) => Ok(values),
        _ => Err(Error::syntax("expected array")),
    }
}

/// The font dictionary every vector stores as object 4.
pub(crate) fn dictionary(document: &Document) -> Result<&Dictionary> {
    document
        .object(ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("expected dictionary"))
}

pub(crate) fn rows(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("expected row"))
}

pub(crate) fn float(value: &Value) -> Result<f64> {
    value
        .as_number()
        .ok_or_else(|| Error::syntax("expected number"))
}

pub(crate) fn integer(value: &Value) -> Result<u32> {
    value
        .as_integer()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| Error::syntax("expected integer"))
}

pub(crate) fn text(value: &Value) -> Option<&str> {
    value.as_string().and_then(|v| std::str::from_utf8(v).ok())
}

pub(crate) fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}
