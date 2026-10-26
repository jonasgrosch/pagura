//! Frozen syntax controls for complete and extendable input boundaries.

use pagura_core::syntax::{InputEnd, Origin, Parser, WindowRead};
use pagura_core::{Error, ErrorKind, Limits, Name, Recovery, Result, Value};

fn parse(bytes: &[u8], end: InputEnd, recovery: Recovery) -> Result<WindowRead<Value>> {
    Parser::new(bytes, Origin::Buffer, Limits::default())
        .with_recovery(recovery)
        .read_window(end, Parser::parse)
}

#[test]
fn every_split_of_nested_object_waits_until_its_structure_completes() {
    let bytes = b"<< /A [true false null 17 0 R] /N /A#20B /S (x\\050y\\051) /H <4f0a> >>";
    for split in 0..bytes.len() {
        assert!(
            matches!(parse(&bytes[..split], InputEnd::Extendable, Recovery::Strict).unwrap(), WindowRead::Incomplete { minimum_length } if minimum_length == split + 1),
            "split {split}"
        );
    }
    let WindowRead::Complete(parsed) =
        parse(bytes, InputEnd::Extendable, Recovery::Strict).unwrap()
    else {
        panic!("complete dictionary")
    };
    let dictionary = parsed.value.as_dictionary().unwrap();
    assert_eq!(dictionary.get(b"N"), Some(&Value::Name(Name::new(b"A B"))));
    assert_eq!(dictionary.get(b"S"), Some(&Value::String(b"x(y)".to_vec())));
    assert_eq!(dictionary.get(b"H"), Some(&Value::String(vec![0x4f, 0x0a])));
    assert_eq!(parsed.offset, bytes.len());
    assert_eq!(parsed.diagnostics, []);
}

#[test]
fn open_boundary_does_not_turn_malformed_tokens_into_missing_input() {
    for bytes in [&b"@"[..], b"<z", b"[)", b"<< /A @"] {
        let error = parse(bytes, InputEnd::Extendable, Recovery::Strict).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
    }
    for bytes in [
        &b"tru"[..],
        b"12",
        b"12 0",
        b"12 0 R",
        b"/Name",
        b"<a",
        b"(x\\",
        b"<< /A 1 >",
    ] {
        assert!(matches!(
            parse(bytes, InputEnd::Extendable, Recovery::Strict).unwrap(),
            WindowRead::Incomplete { .. }
        ));
    }
}

#[test]
fn known_delimiters_finish_values_without_fetching_unrelated_bytes() {
    for (bytes, expected) in [
        (&b"true]"[..], Value::Boolean(true)),
        (&b"12]"[..], Value::Integer(12)),
        (&b"/Name]"[..], Value::Name(Name::new(b"Name"))),
        (&b"(text)"[..], Value::String(b"text".to_vec())),
    ] {
        let WindowRead::Complete(parsed) =
            parse(bytes, InputEnd::Extendable, Recovery::Strict).unwrap()
        else {
            panic!("delimited value")
        };
        assert_eq!(parsed.value, expected);
    }
}

#[test]
fn true_end_retains_existing_values_and_truncation_errors() {
    for (bytes, expected) in [
        (&b"true"[..], Value::Boolean(true)),
        (&b"12"[..], Value::Integer(12)),
        (&b"/Name"[..], Value::Name(Name::new(b"Name"))),
    ] {
        let WindowRead::Complete(parsed) =
            parse(bytes, InputEnd::Complete, Recovery::Strict).unwrap()
        else {
            panic!("complete input")
        };
        assert_eq!(parsed.value, expected);
    }
    for bytes in [&b"(text"[..], b"<< /A 1 >", b"[true", b"tru"] {
        assert_eq!(
            parse(bytes, InputEnd::Complete, Recovery::Strict)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}

#[test]
fn incomplete_name_escape_cannot_publish_a_repair_or_strict_refusal() {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        assert!(matches!(
            parse(b"/A#2", InputEnd::Extendable, recovery).unwrap(),
            WindowRead::Incomplete { .. }
        ));
    }
    let WindowRead::Complete(parsed) =
        parse(b"/A#20]", InputEnd::Extendable, Recovery::Strict).unwrap()
    else {
        panic!("complete escaped name")
    };
    assert_eq!(parsed.value, Value::Name(Name::new(b"A ")));
    assert_eq!(parsed.diagnostics, []);
    assert!(parse(b"/A#2", InputEnd::Complete, Recovery::Strict).is_err());
}

#[test]
fn provisional_diagnostics_are_published_only_with_a_completed_operation() {
    assert!(matches!(
        parse(b"<< /A 1 /A 2 >", InputEnd::Extendable, Recovery::Repair).unwrap(),
        WindowRead::Incomplete { .. }
    ));
    let WindowRead::Complete(parsed) =
        parse(b"<< /A 1 /A 2 >>", InputEnd::Extendable, Recovery::Repair).unwrap()
    else {
        panic!("complete repaired dictionary")
    };
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(
        parsed.value.as_dictionary().unwrap().get(b"A"),
        Some(&Value::Integer(2))
    );
}

#[test]
fn boundary_observation_never_defers_resource_or_non_syntax_failures() {
    let result = Parser::new(b"", Origin::Buffer, Limits::default()).read_window(
        InputEnd::Extendable,
        |parser| {
            assert!(parser.at_end());
            Err::<Value, _>(Error::limit("independent budget control"))
        },
    );
    assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
    let result = Parser::new(b"", Origin::Buffer, Limits::default()).read_window(
        InputEnd::Extendable,
        |parser| {
            assert!(parser.at_end());
            Err::<Value, _>(Error::missing_object("independent dependency control"))
        },
    );
    assert_eq!(result.unwrap_err().kind, ErrorKind::MissingObject);
}
