#![forbid(unsafe_code)]
//! Written values parse back to themselves, under the limits they were
//! written with (ADR 0029). The exact bytes are checked elsewhere against
//! the standard; this checks the writer and the parser agree.

use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Dictionary, ErrorKind, Limits, Name, ObjectId, Result, Value};
use pagura_write::Builder;

const ROOT: ObjectId = ObjectId::new(1, 0);

/// Writes `values` as objects 2, 3, … and parses each back from the file.
fn round_trip(values: &[Value], limits: Limits) -> Result<Vec<Value>> {
    let mut builder = Builder::new(limits);
    let catalog: Dictionary = [(Name::new(b"Type"), Value::name(b"Catalog"))]
        .into_iter()
        .collect();
    builder.insert(ROOT, Value::Dictionary(catalog).into())?;
    let numbers = 2..;
    for (number, value) in numbers.clone().zip(values) {
        builder.insert(ObjectId::new(number, 0), value.clone().into())?;
    }
    let file = builder.finish(ROOT)?;
    numbers
        .zip(values)
        .map(|(number, _)| {
            let header = format!("\n{number} 0 obj\n");
            let start = file
                .windows(header.len())
                .position(|window| window == header.as_bytes())
                .map(|offset| offset + header.len());
            let start = start.ok_or(pagura_core::Error::missing_object("object not written"))?;
            Parser::with_offset(&file, start, Origin::File, limits)?.parse()
        })
        .collect()
}

fn dictionary(entries: &[(&[u8], Value)]) -> Value {
    Value::Dictionary(
        entries
            .iter()
            .map(|(key, value)| (Name::new(key), value.clone()))
            .collect(),
    )
}

#[test]
fn every_kind_of_value_parses_back_unchanged() {
    let every_byte: Vec<u8> = (0..=u8::MAX).collect();
    let values = [
        Value::Null,
        Value::Boolean(true),
        Value::Boolean(false),
        Value::Integer(0),
        Value::Integer(i64::MIN),
        Value::Integer(i64::MAX),
        Value::Real(0.1),
        Value::Real(-2.5),
        Value::Real(3.0),
        Value::Real(1e-10),
        Value::Real(123_456_789.125),
        Value::Real(f64::from(u32::MAX) * 1e6),
        Value::name(b""),
        Value::Name(Name::new(&every_byte[1..])),
        Value::name(b"A B#()<>[]{}/%"),
        Value::String(vec![]),
        Value::String(every_byte.clone()),
        Value::String(every_byte.repeat(9)),
        Value::Reference(ROOT),
        Value::Array(vec![]),
        Value::Array(vec![
            Value::Array(vec![Value::Integer(1), Value::Real(-0.5)]),
            dictionary(&[]),
        ]),
        dictionary(&[
            (b"Type", Value::name(b"Example")),
            (b"Key with spaces", Value::String(b"(x)".to_vec())),
            (&every_byte[1..], Value::Array(vec![Value::Reference(ROOT)])),
            (b"Nested", dictionary(&[(b"A", Value::Null)])),
        ]),
    ];
    let parsed = round_trip(&values, Limits::default()).unwrap();
    assert_eq!(parsed, values);
}

#[test]
fn objects_at_every_limit_are_read_back_under_the_same_limits() {
    let limits = Limits {
        depth: 3,
        value_nodes: 40,
        string_bytes: 64,
        ..Limits::default()
    };
    let deepest = (0..3).fold(Value::Null, |inner, _| Value::Array(vec![inner]));
    let values = [
        // 1 array + 39 elements = 40 values.
        Value::Array(vec![Value::Integer(7); 39]),
        deepest,
        Value::String(vec![0xAB; 64]),
        Value::Name(Name::new(&[b'n'; 64])),
    ];
    let parsed = round_trip(&values, limits).unwrap();
    assert_eq!(parsed, values);

    // One more of each is refused when written, not left for the reader.
    let deeper = (0..4).fold(Value::Null, |inner, _| Value::Array(vec![inner]));
    let over = [
        Value::Array(vec![Value::Integer(7); 40]),
        deeper,
        Value::String(vec![0xAB; 65]),
        Value::Name(Name::new(&[b'n'; 65])),
    ];
    for value in over {
        let error = round_trip(std::slice::from_ref(&value), limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{value:?}");
    }
}
