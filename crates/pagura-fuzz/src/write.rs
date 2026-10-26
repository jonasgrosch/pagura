//! `write`: what the writer serializes reads back as the same objects,
//! parse(write(v)) == v, or the writer refuses with a structured error
//! (7.3, 7.5; `pagura::Builder`).
//!
//! Layout: parts split at [`super::wrapper::SEPARATOR`] become objects 3,
//! 4, … of a fixed document whose catalog (1) has an empty page tree (2); a
//! part `<< entries >> stream` and a line break followed by data becomes a
//! stream. Every object the document then holds is inserted into a new
//! [`Builder`], with its raw stream data, and the written file reopens
//! strictly with equal values.

use super::wrapper::Pdf;
use pagura::{
    Builder, Dictionary, Document, ErrorKind, Name, NewObject, ObjectId, OpenOptions, Recovery,
    Value,
};

/// How many objects are written and compared.
const OBJECTS: usize = 64;
/// The wrapper's catalog.
const ROOT: ObjectId = ObjectId::new(1, 0);

/// Writes the input's objects to a new file and compares them after
/// reopening it.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let mut pdf = Pdf::without_pages();
    pdf.parts(&super::wrapper::parts(data));
    let options = OpenOptions {
        limits: super::limits(),
        recovery: Recovery::Repair,
        ..OpenOptions::default()
    };
    let Some(source) = super::checked(Document::open(pdf.finish(), &options)) else {
        return;
    };
    let mut builder = Builder::new(options.limits);
    let mut written = Vec::new();
    let ids: Vec<_> = source.object_ids().take(OBJECTS).collect();
    for id in ids {
        let Some(object) = super::checked(source.object(id)) else {
            continue;
        };
        let original = match object.dictionary() {
            Some(dictionary) if object.is_stream() => {
                let Some(data) = super::checked(source.raw_stream_data(object)) else {
                    continue;
                };
                Original::Stream(dictionary.clone(), data.to_vec())
            }
            _ => Original::Value(object.value().clone()),
        };
        if super::checked(builder.insert(id, original.to_new())).is_some() {
            written.push((id, original));
        }
    }
    let Some(bytes) = super::checked(builder.finish(ROOT)) else {
        return;
    };
    let strict = OpenOptions {
        recovery: Recovery::Strict,
        ..options
    };
    let copy = match Document::open(bytes, &strict) {
        Ok(copy) => copy,
        Err(error) => {
            assert_eq!(
                error.kind,
                ErrorKind::Limit,
                "written file rejected: {error}"
            );
            return;
        }
    };
    for (id, expected) in written {
        let object = match copy.object(id) {
            Ok(object) => object,
            Err(error) => {
                assert_eq!(
                    error.as_error().map(|error| &error.kind),
                    Some(&ErrorKind::Limit),
                    "{id} does not read back: {error}"
                );
                continue;
            }
        };
        match &expected {
            Original::Value(value) => assert!(
                same(object.value(), value),
                "{id}: {value:?} read back as {:?}",
                object.value()
            ),
            Original::Stream(dictionary, data) => {
                assert!(object.is_stream(), "{id}: the stream read back as a value");
                let actual = object.dictionary();
                assert!(
                    actual.is_some_and(|actual| same_dictionary(actual, dictionary, b"Length")),
                    "{id}: stream dictionary {dictionary:?} read back as {actual:?}"
                );
                if let Some(read) = super::checked(copy.raw_stream_data(object)) {
                    assert_eq!(read, data.as_slice(), "{id}: stream data changed");
                }
            }
        }
    }
}

/// An object as the source document holds it: a value, or a stream's
/// dictionary and raw (still encoded) data.
enum Original {
    Value(Value),
    Stream(Dictionary, Vec<u8>),
}

impl Original {
    fn to_new(&self) -> NewObject {
        match self {
            Self::Value(value) => NewObject::Value(value.clone()),
            Self::Stream(dictionary, data) => NewObject::Stream {
                dictionary: dictionary.clone(),
                data: data.clone(),
            },
        }
    }
}

/// Values are equal, reals up to their last bits: the writer prints the
/// shortest decimal that names the same `f64`, and the parser need not
/// round it exactly.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Real(a), Value::Real(b)) => {
            (a - b).abs() <= 4.0 * f64::EPSILON * a.abs().max(b.abs())
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Dictionary(a), Value::Dictionary(b)) => same_dictionary(a, b, b""),
        _ => a == b,
    }
}

/// Dictionaries hold the same keys in the same order with the same values,
/// ignoring the key `except` (the `/Length` the writer sets).
fn same_dictionary(a: &Dictionary, b: &Dictionary, except: &[u8]) -> bool {
    let (a, b) = (entries(a, except), entries(b, except));
    a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|((ka, va), (kb, vb))| ka == kb && same(va, vb))
}

fn entries<'a>(dictionary: &'a Dictionary, except: &[u8]) -> Vec<(&'a Name, &'a Value)> {
    dictionary
        .iter()
        .filter(|(key, _)| key.as_bytes() != except)
        .collect()
}
