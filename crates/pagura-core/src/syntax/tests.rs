//! Unit tests of the bounded parser: budgets, positions and every reported
//! lexical repair in both recovery modes.

use super::{
    DICTIONARY_CLOSER, DICTIONARY_OPENER, KEY_SOLIDUS, LEADING_ZERO_REFERENCE, NAME_ESCAPE, Origin,
    Parser, ZERO_REFERENCE, hex_digit,
};
use crate::{ErrorKind, Limits, Name, ObjectId, Position, Recovery, Value};

fn parse(data: &[u8]) -> crate::Result<Value> {
    Parser::new(data, Origin::Buffer, Limits::default()).parse()
}

#[test]
fn value_budget_applies_per_object_not_per_parser() {
    let limits = Limits {
        value_nodes: 2,
        ..Limits::default()
    };
    let mut parser = Parser::new(b"1 2 3 4 5", Origin::Buffer, limits);
    for expected in 1..=5 {
        assert_eq!(parser.parse().unwrap(), Value::Integer(expected));
    }
}

/// The text conversion the short path replaces, for comparison.
fn text_value(token: &str) -> Value {
    if token.contains('.') {
        Value::Real(token.parse().unwrap())
    } else {
        Value::Integer(token.parse().unwrap())
    }
}

#[test]
fn short_numbers_match_text_conversion_bit_for_bit() {
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut tokens = vec![
        "0".to_owned(),
        "-0".into(),
        "+0".into(),
        "-0.0".into(),
        "+.000".into(),
        "-.5".into(),
        "3.".into(),
        "007".into(),
        "0.1".into(),
        "0.3".into(),
        "999999999999999".into(),
        "99999999999999.9".into(),
        ".99999999999999".into(),
        "9007199254740993".into(),
        "0.00000000000001".into(),
    ];
    for length in 1..=17 {
        for _ in 0..400 {
            let mut token = String::new();
            for _ in 0..length {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                token.push(char::from(b'0' + u8::try_from(seed % 10).unwrap()));
            }
            let point = usize::try_from(seed >> 32).unwrap() % (length + 2);
            if point <= length {
                token.insert(point, '.');
            }
            let sign = ["", "-", "+"][usize::try_from(seed >> 40).unwrap() % 3];
            tokens.push(format!("{sign}{token}"));
        }
    }
    for token in tokens {
        let actual = parse(token.as_bytes()).unwrap();
        let expected = text_value(&token);
        match (&actual, &expected) {
            (Value::Real(a), Value::Real(b)) => assert_eq!(a.to_bits(), b.to_bits(), "{token}"),
            _ => assert_eq!(actual, expected, "{token}"),
        }
    }
}

#[test]
fn name_prefixes_keep_decoded_limits_and_escape_positions() {
    let limits = Limits {
        string_bytes: 3,
        ..Limits::default()
    };
    for input in [b"/abc".as_slice(), b"/ab#63"] {
        let mut parser = Parser::new(input, Origin::Buffer, limits);
        assert_eq!(parser.parse().unwrap(), Value::Name(Name::new(b"abc")));
    }
    for input in [b"/abcd".as_slice(), b"/ab#63d"] {
        let mut parser = Parser::new(input, Origin::Buffer, limits);
        let error = parser.parse().unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.position, Position::Buffer(input.len()));
    }
    let mut parser = Parser::new(b"/ab#q", Origin::Buffer, limits);
    let error = parser.parse().unwrap_err();
    assert_eq!(error.repair, Some(NAME_ESCAPE));
    assert_eq!(error.position, Position::Buffer(3));
    let mut parser = Parser::new(b"/ab#00", Origin::Buffer, limits);
    let error = parser.parse().unwrap_err();
    assert_eq!(error.message, "name contains a null byte");
    assert_eq!(error.position, Position::Buffer(6));
}

#[test]
fn reference_lookahead_restores_every_unsuccessful_candidate() {
    for input in [
        b"1 /A".as_slice(),
        b"1 +0 R",
        b"1 65536 R",
        b"1 2x R",
        b"1 0 Rigid",
        b"1 % comment\n2 3",
    ] {
        let mut parser = Parser::new(input, Origin::Buffer, Limits::default());
        assert_eq!(parser.parse().unwrap(), Value::Integer(1));
        assert_eq!(parser.offset(), 1);
    }
    let mut parser = Parser::new(b"1 % comment\n65535 R", Origin::Buffer, Limits::default());
    assert_eq!(
        parser.parse().unwrap(),
        Value::Reference(ObjectId::new(1, 65535))
    );
}

#[test]
fn deep_literal_string_nesting_is_valid() {
    let data = [b"(".repeat(200), b")".repeat(200)].concat();
    let expected = [b"(".repeat(199), b")".repeat(199)].concat();
    assert_eq!(parse(&data).unwrap(), Value::String(expected));
}

#[test]
fn duplicate_keys_are_errors_or_reported_repairs() {
    let data = b"<< /A 1 /A 2 >>";
    let error = parse(data).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.position, Position::Buffer(8));
    let mut parser =
        Parser::new(data, Origin::Buffer, Limits::default()).with_recovery(Recovery::Repair);
    let Value::Dictionary(dictionary) = parser.parse().unwrap() else {
        panic!("expected dictionary");
    };
    assert_eq!(dictionary.get(b"A"), Some(&Value::Integer(2)));
    assert_eq!(parser.take_diagnostics().len(), 1);
}

#[test]
fn stream_origin_reports_stream_positions() {
    let object = ObjectId::new(4, 0);
    let error = Parser::new(b"  )", Origin::Stream(object), Limits::default())
        .parse()
        .unwrap_err();
    assert_eq!(error.position, Position::Stream { object, offset: 2 });
}

#[test]
fn lexical_repairs_are_positioned_and_strict_errors_keep_their_tags() {
    for (bytes, tag) in [
        (b"<< Bare 1 >>".as_slice(), KEY_SOLIDUS),
        (b"/Name#Q", NAME_ESCAPE),
        (b"0 0 R", ZERO_REFERENCE),
        (b"<< /A 1 ) >>", DICTIONARY_CLOSER),
        (b"<< << /A 1 >>", DICTIONARY_OPENER),
    ] {
        let mut strict = Parser::new(bytes, Origin::File, Limits::default());
        assert_eq!(strict.parse().unwrap_err().repair, Some(tag));
        let mut repair =
            Parser::new(bytes, Origin::File, Limits::default()).with_recovery(Recovery::Repair);
        repair.parse().unwrap();
        assert_eq!(repair.take_diagnostics()[0].repair(), Some(tag));
    }
    let mut parser = Parser::new(b"/A# /B#2 /C#23 0 0 R", Origin::File, Limits::default())
        .with_recovery(Recovery::Repair);
    for expected in [b"A#".as_slice(), b"B#2", b"C#"] {
        assert_eq!(
            parser.parse().unwrap().as_name().unwrap().as_bytes(),
            expected
        );
    }
    assert_eq!(parser.parse().unwrap(), Value::Null);
    for bytes in [
        b"<< 3 4 >>".as_slice(),
        b"<< true 4 >>",
        b"<< stream 4 >>",
        b"<< /A 1 ) /B 2 >>",
    ] {
        assert!(
            Parser::new(bytes, Origin::File, Limits::default())
                .with_recovery(Recovery::Repair)
                .parse()
                .is_err()
        );
    }
}

#[test]
fn identifier_zeroes_are_reported_only_after_a_complete_reference() {
    for bytes in [b"01 0 R".as_slice(), b"1 00 R", b"01 00 R"] {
        let error = Parser::new(bytes, Origin::File, Limits::default())
            .parse()
            .unwrap_err();
        assert_eq!(error.repair, Some(LEADING_ZERO_REFERENCE));
        assert_eq!(error.position, Position::File(0));
        let mut parser =
            Parser::new(bytes, Origin::File, Limits::default()).with_recovery(Recovery::Repair);
        assert_eq!(
            parser.parse().unwrap(),
            Value::Reference(ObjectId::new(1, 0))
        );
        assert_eq!(parser.take_diagnostics().len(), 1);
    }
    let mut parser = Parser::new(b"[01 00 1 0]", Origin::Buffer, Limits::default());
    assert_eq!(
        parser.parse().unwrap(),
        Value::Array(vec![
            Value::Integer(1),
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(0)
        ])
    );
    assert_eq!(parser.take_diagnostics(), []);
}

#[test]
fn braces_are_regular_characters_in_ordinary_pdf_names() {
    let mut parser = Parser::new(b"[/A{B /C}D /E#7bF]", Origin::Buffer, Limits::default());
    assert_eq!(
        parser.parse().unwrap(),
        Value::Array(vec![
            Value::name(b"A{B"),
            Value::name(b"C}D"),
            Value::name(b"E{F")
        ])
    );
    assert_eq!(parser.take_diagnostics(), []);
    assert!(!super::is_delimiter(b'{'));
    assert!(!super::is_delimiter(b'}'));
}

#[test]
fn hex_digits_accept_both_cases_only() {
    let digits: Vec<_> = b"09afAF".iter().map(|&b| hex_digit(b)).collect();
    assert_eq!(
        digits,
        [Some(0), Some(9), Some(10), Some(15), Some(10), Some(15)]
    );
    for byte in [b'g', b'G', b'/', b':', b'@', b'`', 0, 0xff] {
        assert_eq!(hex_digit(byte), None, "{byte}");
    }
}

#[test]
fn bare_keys_decode_like_names_and_count_against_the_value_budget() {
    let mut parser = Parser::new(b"<< Key#20A 1 >>", Origin::Buffer, Limits::default())
        .with_recovery(Recovery::Repair);
    let Value::Dictionary(dictionary) = parser.parse().unwrap() else {
        panic!("expected dictionary");
    };
    assert_eq!(dictionary.get(b"Key A"), Some(&Value::Integer(1)));
    assert_eq!(parser.take_diagnostics()[0].repair(), Some(KEY_SOLIDUS));
    let limits = Limits {
        value_nodes: 1,
        ..Limits::default()
    };
    let error = Parser::new(b"<< Key 1 >>", Origin::Buffer, limits)
        .with_recovery(Recovery::Repair)
        .parse()
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}

#[test]
fn offsets_beyond_the_input_are_rejected() {
    let parser = Parser::with_offset(b"12", 2, Origin::File, Limits::default()).unwrap();
    assert_eq!(parser.offset(), 2);
    let error = Parser::with_offset(b"12", 3, Origin::File, Limits::default()).unwrap_err();
    assert_eq!(
        (error.kind, error.position),
        (ErrorKind::Syntax, Position::File(3))
    );
}

#[test]
fn direct_stream_handlers_must_end_after_endstream() {
    let bytes = b"<< /A << /Length 1 >> stream\nZ\nendstream >>";
    let mut parser =
        Parser::new(bytes, Origin::File, Limits::default()).with_recovery(Recovery::Repair);
    let error = parser
        .parse_with_streams(&mut |parser, _| {
            assert!(parser.keyword(b"stream"));
            Ok(None)
        })
        .unwrap_err();
    assert_eq!(
        error.message,
        "direct stream handler did not consume endstream"
    );
}
