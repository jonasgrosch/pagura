#![forbid(unsafe_code)]
//! The builder's validation, budgets and file structure, through its public API.

use pagura_core::{Dictionary, ErrorKind, Limits, Name, ObjectId, Value};
use pagura_write::{Builder, NewObject};

fn catalog() -> NewObject {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(b"Type"), Value::name(b"Catalog"));
    NewObject::Value(Value::Dictionary(dictionary))
}

fn builder(limits: Limits) -> Builder {
    let mut builder = Builder::new(limits);
    let inserted = builder.insert(ObjectId::new(1, 0), catalog());
    assert_eq!(inserted, Ok(()));
    builder
}

#[test]
fn trailer_root_is_a_dictionary_not_a_stream() {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(b"Type"), Value::name(b"Catalog"));
    let mut output = Builder::default();
    output
        .insert(
            ObjectId::new(1, 0),
            NewObject::Stream {
                dictionary,
                data: vec![],
            },
        )
        .unwrap();
    assert_eq!(
        output.finish(ObjectId::new(1, 0)).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn new_documents_require_zero_generations() {
    let mut output = Builder::default();
    for generation in [1, 7, u16::MAX] {
        let id = ObjectId::new(1, generation);
        let error = output.insert(id, catalog()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArgument);
        assert_eq!(error.object, Some(id));
    }
}

#[test]
fn names_exclude_nul_in_values_and_keys() {
    for value in [Value::name(b"bad\0name"), {
        let mut dictionary = Dictionary::new();
        dictionary.insert(Name::new(b"bad\0key"), Value::Null);
        Value::Dictionary(dictionary)
    }] {
        let mut output = builder(Limits::default());
        output
            .insert(ObjectId::new(2, 0), NewObject::Value(value))
            .unwrap();
        let error = output.finish(ObjectId::new(1, 0)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArgument);
        assert_eq!(error.object, Some(ObjectId::new(2, 0)));
    }
}

#[test]
fn sparse_cross_reference_ranges_are_bounded_before_serializing() {
    for number in [u32::MAX, u32::MAX - 1, 4_000_000] {
        let mut output = builder(Limits::default());
        output
            .insert(ObjectId::new(number, 0), NewObject::Value(Value::Null))
            .unwrap();
        assert_eq!(
            output.finish(ObjectId::new(1, 0)).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn xref_entries_free_chain_and_trailer_match_independent_offsets() {
    let mut output = builder(Limits::default());
    output
        .insert(ObjectId::new(3, 0), NewObject::Value(Value::Null))
        .unwrap();
    let bytes = output.finish(ObjectId::new(1, 0)).unwrap();
    let header = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n";
    let first = b"1 0 obj\n<< /Type /Catalog >>\nendobj\n";
    let third = b"3 0 obj\nnull\nendobj\n";
    let prefix = [&header[..], first, third].concat();
    assert!(bytes.starts_with(&prefix));
    let expected = format!(
        concat!(
            "xref\n0 4\n",
            "0000000002 65535 f \n",
            "{:010} 00000 n \n",
            "0000000000 00000 f \n",
            "{:010} 00000 n \n",
            "trailer\n<< /Size 4 /Root 1 0 R >>\n",
            "startxref\n{}\n%%EOF\n",
        ),
        header.len(),
        header.len() + first.len(),
        prefix.len()
    );
    assert_eq!(&bytes[prefix.len()..], expected.as_bytes());
}

#[test]
fn streams_beyond_the_stream_budget_fail() {
    let mut output = builder(Limits {
        stream_bytes: 2,
        ..Limits::default()
    });
    output
        .insert(
            ObjectId::new(2, 0),
            NewObject::Stream {
                dictionary: Dictionary::new(),
                data: vec![0; 3],
            },
        )
        .unwrap();
    assert_eq!(
        output.finish(ObjectId::new(1, 0)).unwrap_err().kind,
        ErrorKind::Limit
    );
}

#[test]
fn the_file_budget_admits_exactly_the_written_size() {
    let finish = |file_bytes| {
        let mut output = builder(Limits {
            file_bytes,
            ..Limits::default()
        });
        let stream = NewObject::Stream {
            dictionary: Dictionary::new(),
            data: vec![7; 4096],
        };
        output.insert(ObjectId::new(2, 0), stream).unwrap();
        output.finish(ObjectId::new(1, 0))
    };
    let size = finish(Limits::default().file_bytes).unwrap().len();
    assert_eq!(finish(size).unwrap().len(), size);
    assert_eq!(finish(size - 1).unwrap_err().kind, ErrorKind::Limit);
}

#[test]
fn the_value_budget_applies_per_object() {
    // An array of 999 nulls is 1000 values, exactly the budget.
    let limits = Limits {
        value_nodes: 1000,
        ..Limits::default()
    };
    let mut output = builder(limits);
    for number in 2..=11 {
        let array = Value::Array(vec![Value::Null; 999]);
        output
            .insert(ObjectId::new(number, 0), NewObject::Value(array))
            .unwrap();
    }
    assert!(output.finish(ObjectId::new(1, 0)).is_ok());
    let over = ObjectId::new(12, 0);
    let array = Value::Array(vec![Value::Null; 1000]);
    output.insert(over, NewObject::Value(array)).unwrap();
    let error = output.finish(ObjectId::new(1, 0)).unwrap_err();
    assert_eq!((error.kind, error.object), (ErrorKind::Limit, Some(over)));
}

#[test]
fn nesting_beyond_the_depth_limit_fails() {
    let limits = Limits {
        depth: 2,
        ..Limits::default()
    };
    let nested = |levels| (0..levels).fold(Value::Null, |inner, _| Value::Array(vec![inner]));
    for (levels, fits) in [(2, true), (3, false)] {
        let mut output = builder(limits);
        output
            .insert(ObjectId::new(2, 0), NewObject::Value(nested(levels)))
            .unwrap();
        let result = output.finish(ObjectId::new(1, 0));
        assert_eq!(result.is_ok(), fits, "{levels} levels");
    }
}

#[test]
fn stream_dictionaries_are_checked_as_written() {
    // value_nodes 2 admits the dictionary and the /Length the writer adds.
    let limits = Limits {
        value_nodes: 2,
        ..Limits::default()
    };
    let stream = |entries: &[(&[u8], Value)]| {
        let mut output = builder(limits);
        let dictionary = entries
            .iter()
            .map(|(key, value)| (Name::new(key), value.clone()))
            .collect();
        let data = b"data".to_vec();
        let object = NewObject::Stream { dictionary, data };
        output.insert(ObjectId::new(2, 0), object).unwrap();
        output.finish(ObjectId::new(1, 0))
    };
    // A stale or dangling /Length is replaced, so it is neither checked nor counted.
    let stale = Value::Reference(ObjectId::new(9, 0));
    let bytes = stream(&[(b"Length", stale)]).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("<< /Length 4 >>\nstream\ndata\nendstream"),
        "{text}"
    );
    assert!(stream(&[]).is_ok());
    let error = stream(&[(b"Filter", Value::name(b"ASCIIHexDecode"))]).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}

#[test]
fn insert_and_root_errors_name_the_object() {
    let mut output = builder(Limits::default());
    for id in [ObjectId::new(0, 0), ObjectId::new(1, 0)] {
        let error = output
            .insert(id, NewObject::Value(Value::Null))
            .unwrap_err();
        assert_eq!(
            (error.kind, error.object),
            (ErrorKind::InvalidArgument, Some(id))
        );
    }
    let missing = ObjectId::new(2, 0);
    let error = output.finish(missing).unwrap_err();
    assert_eq!(
        (error.kind, error.object),
        (ErrorKind::MissingObject, Some(missing))
    );
    output
        .insert(missing, NewObject::Value(Value::Null))
        .unwrap();
    let error = output.finish(missing).unwrap_err();
    assert_eq!(
        (error.kind, error.object),
        (ErrorKind::InvalidArgument, Some(missing))
    );

    let mut full = Builder::new(Limits {
        objects: 1,
        ..Limits::default()
    });
    full.insert(ObjectId::new(1, 0), catalog()).unwrap();
    let error = full.insert(missing, catalog()).unwrap_err();
    assert_eq!(
        (error.kind, error.object),
        (ErrorKind::Limit, Some(missing))
    );
}

#[test]
fn generated_length_obeys_depth_limit_in_an_empty_stream_dictionary() {
    let object = NewObject::Stream {
        dictionary: Dictionary::new(),
        data: Vec::new(),
    };
    let limits = Limits {
        depth: 0,
        ..Limits::default()
    };
    assert_eq!(
        pagura_write::validate_object(&object, &limits, &|_| true)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert!(
        pagura_write::validate_object(&object, &Limits { depth: 1, ..limits }, &|_| true).is_ok()
    );
}

#[test]
fn generated_length_name_obeys_string_budget() {
    let object = NewObject::Stream {
        dictionary: Dictionary::new(),
        data: Vec::new(),
    };
    assert_eq!(
        pagura_write::validate_object(
            &object,
            &Limits {
                string_bytes: 5,
                ..Limits::default()
            },
            &|_| true
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
    assert!(
        pagura_write::validate_object(
            &object,
            &Limits {
                string_bytes: 6,
                ..Limits::default()
            },
            &|_| true
        )
        .is_ok()
    );
}
