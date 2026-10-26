//! Frozen syntax controls for complete and extendable input boundaries.

use pagura_core::syntax::{InputEnd, Origin, Parser, WindowRead};
use pagura_core::{Error, ErrorKind, Limits, Name, ObjectId, Position, Recovery, Result, Value};

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
            Err::<Value, _>(Error::limit("independent budget control").into())
        },
    );
    assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
    let result = Parser::new(b"", Origin::Buffer, Limits::default()).read_window(
        InputEnd::Extendable,
        |parser| {
            assert!(parser.at_end());
            Err::<Value, _>(Error::missing_object("independent dependency control").into())
        },
    );
    assert_eq!(result.unwrap_err().kind, ErrorKind::MissingObject);
}

#[test]
fn terminal_unsigned_and_numeric_tokens_do_not_wait_for_unrelated_bytes() {
    for bytes in [
        &b"12x"[..],
        b"18446744073709551616",
        b"999999999999999999999999999999999999",
    ] {
        let error = Parser::new(bytes, Origin::Buffer, Limits::default())
            .read_window(InputEnd::Extendable, Parser::unsigned)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
    }
    for bytes in [&b"12x"[..], b"1..2", b"+x", b"1e3"] {
        assert_eq!(
            parse(bytes, InputEnd::Extendable, Recovery::Strict)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    assert!(matches!(
        parse(
            b"9223372036854775808",
            InputEnd::Extendable,
            Recovery::Strict
        )
        .unwrap(),
        WindowRead::Incomplete { .. }
    ));
    let WindowRead::Complete(value) = parse(
        b"9223372036854775808.0]",
        InputEnd::Extendable,
        Recovery::Strict,
    )
    .unwrap() else {
        panic!("real suffix completes a valid number")
    };
    assert_eq!(value.value, Value::Real(9_223_372_036_854_775_808.0));
}

#[test]
fn callback_syntax_failure_is_terminal_after_any_boundary_observation() {
    for bytes in [&b""[..], b"12"] {
        let error = Parser::new(bytes, Origin::Buffer, Limits::default())
            .read_window(InputEnd::Extendable, |parser| {
                if !bytes.is_empty() {
                    let _ = parser.word()?;
                }
                assert!(parser.at_end());
                Err::<Value, _>(Error::syntax("independent callback failure").into())
            })
            .unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Syntax, "independent callback failure")
        );
    }
}

#[test]
fn window_errors_and_repairs_keep_absolute_origin_positions() {
    let object = ObjectId::new(17, 2);
    for origin in [Origin::File, Origin::Stream(object), Origin::Buffer] {
        let error = Parser::new(b"[true @]", origin, Limits::default())
            .with_origin_offset(101)
            .unwrap()
            .read_window(InputEnd::Extendable, Parser::parse)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.position, origin.position(107));

        let WindowRead::Complete(parsed) =
            Parser::new(b"<< /A 1 /A 2 >>", origin, Limits::default())
                .with_origin_offset(211)
                .unwrap()
                .with_recovery(Recovery::Repair)
                .read_window(InputEnd::Extendable, Parser::parse)
                .unwrap()
        else {
            panic!("complete window")
        };
        assert_eq!(parsed.offset, 15);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].position, origin.position(219));
    }
}

#[test]
fn origin_admission_and_seek_check_overflow_without_changing_local_offsets() {
    let mut parser = Parser::new(b"a 17]", Origin::File, Limits::default())
        .with_origin_offset(301)
        .unwrap();
    parser.seek(2).unwrap();
    assert_eq!(parser.offset(), 2);
    assert_eq!(parser.position(), Position::File(303));
    assert_eq!(parser.parse().unwrap(), Value::Integer(17));
    assert_eq!(parser.offset(), 4);
    assert_eq!(parser.position(), Position::File(305));
    assert_eq!(parser.seek(9).unwrap_err().position, Position::File(310));

    assert_eq!(
        Parser::new(b"x", Origin::File, Limits::default())
            .with_origin_offset(usize::MAX)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let mut parser = Parser::new(b"", Origin::File, Limits::default())
        .with_origin_offset(usize::MAX)
        .unwrap();
    assert_eq!(parser.seek(1).unwrap_err().kind, ErrorKind::Limit);
    assert_eq!(parser.offset(), 0);
    assert_eq!(parser.position(), Position::File(usize::MAX));
}

#[test]
fn token_start_errors_and_repairs_keep_the_window_origin() {
    for origin in [Origin::File, Origin::Stream(ObjectId::new(5, 1))] {
        let error = Parser::new(b" 1..2]", origin, Limits::default())
            .with_origin_offset(307)
            .unwrap()
            .read_window(InputEnd::Extendable, Parser::parse)
            .unwrap_err();
        assert_eq!(error.position, origin.position(308));
        for (bytes, expected) in [
            (&b"/A#x1]"[..], 309),
            (&b" 01 0 R]"[..], 308),
            (&b" 0 0 R]"[..], 308),
        ] {
            let WindowRead::Complete(parsed) = Parser::new(bytes, origin, Limits::default())
                .with_origin_offset(307)
                .unwrap()
                .with_recovery(Recovery::Repair)
                .read_window(InputEnd::Extendable, Parser::parse)
                .unwrap()
            else {
                panic!("completed repaired token")
            };
            assert_eq!(parsed.diagnostics.len(), 1);
            assert_eq!(parsed.diagnostics[0].position, origin.position(expected));
        }
    }
}

#[test]
fn grammar_branches_wait_only_for_undecided_keyword_prefixes() {
    let bytes = b"endstream";
    for split in 0..=bytes.len() {
        let result = Parser::new(&bytes[..split], Origin::Buffer, Limits::default())
            .read_window(InputEnd::Extendable, |parser| {
                parser.keyword_with_boundary(bytes)
            })
            .unwrap();
        assert!(matches!(result, WindowRead::Incomplete { .. }));
    }
    for (bytes, expected) in [(&b"endstream]"[..], true), (b"endstX", false)] {
        let WindowRead::Complete(parsed) = Parser::new(bytes, Origin::Buffer, Limits::default())
            .read_window(InputEnd::Extendable, |parser| {
                parser.keyword_with_boundary(b"endstream")
            })
            .unwrap()
        else {
            panic!("decided keyword")
        };
        assert_eq!(parsed.value, expected);
    }
    let mut complete = Parser::new(b"endstr", Origin::Buffer, Limits::default());
    assert!(!complete.keyword_with_boundary(b"endstream").unwrap());
    assert_eq!(complete.offset(), 0);
}

#[test]
fn declared_byte_extents_separate_window_extension_from_real_truncation() {
    let WindowRead::Complete(parsed) = Parser::new(b"ab", Origin::Buffer, Limits::default())
        .read_window(InputEnd::Complete, |parser| parser.has_bytes(3))
        .unwrap()
    else {
        panic!("true EOF is known")
    };
    assert!(!parsed.value);
    assert_eq!(parsed.offset, 0);
    assert!(matches!(
        Parser::new(b"ab", Origin::Buffer, Limits::default())
            .read_window(InputEnd::Extendable, |parser| parser.has_bytes(3))
            .unwrap(),
        WindowRead::Incomplete { .. }
    ));
    let WindowRead::Complete(parsed) = Parser::new(b"ab", Origin::Buffer, Limits::default())
        .read_window(InputEnd::Extendable, |parser| parser.has_bytes(2))
        .unwrap()
    else {
        panic!("extent already present")
    };
    assert!(parsed.value);
}

#[test]
fn incomplete_reference_lookahead_never_becomes_an_invalid_dictionary_key() {
    let bytes = b"<< /Root 1 0 R >>";
    for split in 0..bytes.len() {
        assert!(
            matches!(
                parse(&bytes[..split], InputEnd::Extendable, Recovery::Strict).unwrap(),
                WindowRead::Incomplete { .. }
            ),
            "split {split}"
        );
    }
    let WindowRead::Complete(parsed) =
        parse(bytes, InputEnd::Extendable, Recovery::Strict).unwrap()
    else {
        panic!("complete reference dictionary")
    };
    assert_eq!(
        parsed.value.as_dictionary().unwrap().get(b"Root"),
        Some(&Value::Reference(ObjectId::new(1, 0)))
    );
}

#[test]
fn reference_repairs_wait_until_the_r_keyword_boundary_is_known() {
    for (bytes, repair) in [
        (&b"01 0 R"[..], pagura_core::repairs::LEADING_ZERO_REFERENCE),
        (&b"1 00 R"[..], pagura_core::repairs::LEADING_ZERO_REFERENCE),
        (&b"0 0 R"[..], pagura_core::repairs::ZERO_REFERENCE),
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            assert!(matches!(
                parse(bytes, InputEnd::Extendable, recovery).unwrap(),
                WindowRead::Incomplete { .. }
            ));
        }
        let mut delimited = bytes.to_vec();
        delimited.push(b']');
        for (input, end) in [
            (bytes, InputEnd::Complete),
            (delimited.as_slice(), InputEnd::Extendable),
        ] {
            assert_eq!(
                parse(input, end, Recovery::Strict).unwrap_err().repair,
                Some(repair)
            );
            let WindowRead::Complete(parsed) = parse(input, end, Recovery::Repair).unwrap() else {
                panic!("confirmed reference repair")
            };
            assert_eq!(parsed.diagnostics.len(), 1);
            assert_eq!(parsed.diagnostics[0].repair(), Some(repair));
        }
    }
}

#[test]
fn impossible_generations_do_not_wait_and_adjacent_integers_remain_distinct() {
    for bytes in [&b"1 65536"[..], b"1 12x", b"1 18446744073709551616"] {
        let WindowRead::Complete(parsed) =
            parse(bytes, InputEnd::Extendable, Recovery::Strict).unwrap()
        else {
            panic!("candidate cannot become a reference")
        };
        assert_eq!(parsed.value, Value::Integer(1));
        assert_eq!(parsed.offset, 1);
    }
    for bytes in [&b"<< /Root 1 65536"[..], b"<< /Root 1 12x"] {
        assert_eq!(
            parse(bytes, InputEnd::Extendable, Recovery::Strict)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    let WindowRead::Complete(parsed) = parse(b"1 0", InputEnd::Complete, Recovery::Strict).unwrap()
    else {
        panic!("EOF settles ordinary integer lookahead")
    };
    assert_eq!(parsed.value, Value::Integer(1));
    assert_eq!(parsed.offset, 1);
    assert!(matches!(
        parse(b"1 0", InputEnd::Extendable, Recovery::Strict).unwrap(),
        WindowRead::Incomplete { .. }
    ));

    let bytes = b"[1 2 65536 0.5]";
    for split in 0..bytes.len() {
        assert!(
            matches!(
                parse(&bytes[..split], InputEnd::Extendable, Recovery::Strict).unwrap(),
                WindowRead::Incomplete { .. }
            ),
            "split {split}"
        );
    }
    let WindowRead::Complete(parsed) =
        parse(bytes, InputEnd::Extendable, Recovery::Strict).unwrap()
    else {
        panic!("complete ordinary operands")
    };
    assert_eq!(
        parsed.value,
        Value::Array(vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(65536),
            Value::Real(0.5)
        ])
    );
}

#[test]
fn dictionary_repairs_wait_for_a_decided_key_or_opener() {
    for bytes in [&b"<< <<"[..], b"<< << ", b"<< tru", b"<< true"] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            assert!(matches!(
                parse(bytes, InputEnd::Extendable, recovery).unwrap(),
                WindowRead::Incomplete { .. }
            ));
            assert_eq!(
                parse(bytes, InputEnd::Complete, recovery).unwrap_err().kind,
                ErrorKind::Syntax
            );
        }
    }
    for (bytes, repair) in [
        (
            &b"<< << /A 1 >>"[..],
            pagura_core::repairs::DICTIONARY_OPENER,
        ),
        (&b"<< trueX 1 >>"[..], pagura_core::repairs::KEY_SOLIDUS),
    ] {
        assert_eq!(
            parse(bytes, InputEnd::Extendable, Recovery::Strict)
                .unwrap_err()
                .repair,
            Some(repair)
        );
        let WindowRead::Complete(parsed) =
            parse(bytes, InputEnd::Extendable, Recovery::Repair).unwrap()
        else {
            panic!("confirmed dictionary repair")
        };
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].repair(), Some(repair));
    }
    for bytes in [&b"<< << @"[..], b"<< 1"] {
        assert_eq!(
            parse(bytes, InputEnd::Extendable, Recovery::Repair)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}

#[test]
fn optional_direct_stream_lookahead_waits_before_calling_or_refusing_the_handler() {
    let prefix = b"<< /S << /Length 0 >> ";
    let keyword = b"stream";
    for split in 0..=keyword.len() {
        let mut bytes = prefix.to_vec();
        bytes.extend_from_slice(&keyword[..split]);
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let result = Parser::new(&bytes, Origin::Buffer, Limits::default())
                .with_recovery(recovery)
                .read_window(InputEnd::Extendable, |parser| {
                    parser.parse_with_streams(&mut |_, _| panic!("handler before keyword boundary"))
                })
                .unwrap();
            assert!(matches!(result, WindowRead::Incomplete { .. }));
        }
    }
    let mut complete_keyword = prefix.to_vec();
    complete_keyword.extend_from_slice(keyword);
    let error = Parser::new(&complete_keyword, Origin::Buffer, Limits::default())
        .read_window(InputEnd::Complete, |parser| {
            parser.parse_with_streams(&mut |_, _| panic!("strict refusal before handler"))
        })
        .unwrap_err();
    assert_eq!(error.repair, Some(pagura_core::repairs::DIRECT_STREAM));

    let WindowRead::Complete(ordinary) = Parser::new(
        b"<< /S << /Length 0 >> /A 1 >>",
        Origin::Buffer,
        Limits::default(),
    )
    .read_window(InputEnd::Extendable, |parser| {
        parser.parse_with_streams(&mut |_, _| panic!("ordinary dictionary is not a stream"))
    })
    .unwrap() else {
        panic!("known non-stream continuation")
    };
    assert_eq!(ordinary.value.1, []);
    assert_eq!(ordinary.diagnostics, []);

    let bytes = b"<< /S << /Length 0 >> stream\nendstream >>";
    let error = Parser::new(bytes, Origin::Buffer, Limits::default())
        .read_window(InputEnd::Extendable, |parser| {
            parser.parse_with_streams(&mut |_, _| panic!("strict refusal before handler"))
        })
        .unwrap_err();
    assert_eq!(error.repair, Some(pagura_core::repairs::DIRECT_STREAM));
    let WindowRead::Complete(parsed) = Parser::new(bytes, Origin::Buffer, Limits::default())
        .with_recovery(Recovery::Repair)
        .read_window(InputEnd::Extendable, |parser| {
            parser.parse_with_streams(&mut |parser, _| {
                assert!(parser.keyword_with_boundary(b"stream")?);
                let start = parser.offset() + 1;
                parser.seek(start)?;
                assert!(parser.keyword_with_boundary(b"endstream")?);
                Ok(Some(start..start))
            })
        })
        .unwrap()
    else {
        panic!("complete direct-stream repair")
    };
    assert_eq!(parsed.value.1.len(), 1);
    assert_eq!(parsed.diagnostics.len(), 1);
    assert_eq!(
        parsed.diagnostics[0].repair(),
        Some(pagura_core::repairs::DIRECT_STREAM)
    );
}
