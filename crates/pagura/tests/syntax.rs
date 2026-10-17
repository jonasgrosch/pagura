#![forbid(unsafe_code)]
//! Object syntax (ISO 32000-2 clauses 7.2–7.3) through the public parser.

use pagura::{ErrorKind, Limits, Name, ObjectId, Recovery, Value};
use pagura_core::syntax::{Origin, Parser};

fn parse(data: &[u8]) -> pagura::Result<Value> {
    Parser::new(data, Origin::Buffer, Limits::default()).parse()
}

#[test]
fn names_are_bytes_and_escapes_are_decoded() {
    assert_eq!(
        parse(b"/A#20B#ff").unwrap(),
        Value::Name(Name::new(b"A B\xff"))
    );
    assert_eq!(parse(b"/").unwrap(), Value::Name(Name::new(b"")));
    for bad in [b"/#00".as_slice(), b"/#x1", b"/#1"] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn literal_strings_handle_nesting_octal_and_line_endings() {
    assert_eq!(
        parse(b"(a(b)c\\n\\101\\777\\\r\nd\r\ne)").unwrap(),
        Value::String(b"a(b)c\nA\xffd\ne".to_vec())
    );
    assert_eq!(parse(b"(\\(x\\))").unwrap(), Value::String(b"(x)".to_vec()));
    assert!(parse(b"(unfinished").is_err());
}

#[test]
fn hexadecimal_strings_pad_odd_nibble() {
    assert_eq!(
        parse(b"<61 6 2 f>").unwrap(),
        Value::String(vec![0x61, 0x62, 0xf0])
    );
    assert_eq!(parse(b"<>").unwrap(), Value::String(vec![]));
    assert!(parse(b"<gg>").is_err());
}

#[test]
fn numbers_reject_non_pdf_syntax_and_overflow() {
    for good in [b"+.5".as_slice(), b"-.5", b"12.", b"+17", b"-0", b"42"] {
        assert!(parse(good).is_ok(), "{good:?}");
    }
    for bad in [
        b".".as_slice(),
        b"+",
        b"1e3",
        b"1.2.3",
        b"NaN",
        b"9223372036854775808",
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn references_include_generations_and_number_lookahead_is_reversible() {
    assert_eq!(
        parse(b"42 7 R").unwrap(),
        Value::Reference(ObjectId::new(42, 7))
    );
    let mut parser = Parser::new(b"42 7 9", Origin::Buffer, Limits::default());
    for expected in [42, 7, 9] {
        assert_eq!(parser.parse().unwrap(), Value::Integer(expected));
    }
}

#[test]
fn comments_and_token_boundaries_matter() {
    assert_eq!(
        parse(b"% comment\r\n[true false null]").unwrap(),
        Value::Array(vec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Null
        ])
    );
    assert!(parse(b"trueish").is_err());
    assert!(parse(b"<< (A) 1 >>").is_err());
}

#[test]
fn duplicate_keys_are_strict_errors_and_reported_repairs() {
    assert_eq!(
        parse(b"<< /A 1 /A 2 >>").unwrap_err().kind,
        ErrorKind::Syntax
    );
    let mut parser = Parser::new(b"<< /A 1 /A 2 >>", Origin::Buffer, Limits::default())
        .with_recovery(Recovery::Repair);
    let value = parser.parse().unwrap();
    assert_eq!(
        value.as_dictionary().unwrap().get(b"A"),
        Some(&Value::Integer(2))
    );
    assert_eq!(
        parser.take_diagnostics()[0].repair.as_str(),
        "syntax.duplicate-key"
    );
}

#[test]
fn budgets_limit_nested_and_large_objects() {
    let limits = Limits {
        depth: 2,
        string_bytes: 3,
        value_nodes: 5,
        ..Limits::default()
    };
    for data in [b"[[[[0]]]]".as_slice(), b"(four)", b"[0 0 0 0 0 0]"] {
        let error = Parser::new(data, Origin::Buffer, limits)
            .parse()
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{data:?}");
    }
}

#[test]
fn deterministic_mutation_smoke_has_no_panics() {
    let seed = b"<< /Type /Page /Box [0 0 64 64] /Text (hello) /Other 9 0 R >>";
    let limits = Limits {
        depth: 8,
        value_nodes: 100,
        string_bytes: 1024,
        ..Limits::default()
    };
    for index in 0..seed.len() {
        for byte in [0, b'%', b'(', b')', b'<', b'>', b'[', b']', b'/', b'#', 255] {
            let mut data = seed.to_vec();
            data[index] = byte;
            let _ = Parser::new(&data, Origin::Buffer, limits).parse();
        }
        let _ = Parser::new(&seed[..index], Origin::Buffer, limits).parse();
    }
}
