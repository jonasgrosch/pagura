use super::*;
use crate::op::tests::operator;
use pagura_core::{ErrorKind, ObjectId, Repair};
use std::borrow::Cow;

fn meter() -> Meter {
    Meter::new(1_000, "test budget exceeded")
}

/// The operators of `bytes` with their offsets, and each operation's debug
/// form.
fn ops(bytes: &[u8]) -> Result<Vec<(Operator, usize, String)>> {
    let content = Content::from_bytes(bytes);
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = meter();
    let mut out = Vec::new();
    while let Some(operation) = reader.next(&mut tokens)? {
        let op = &operation.op;
        out.push((operator(op), operation.offset, format!("{op:?}")));
    }
    assert!(reader.is_balanced());
    Ok(out)
}

#[test]
fn operations_carry_their_operator_offsets() {
    let ops = ops(b"q 1 0 0 1 5 5 cm\n/F1 12 Tf [(a) -20 (b)] TJ Q").unwrap();
    assert_eq!(ops.len(), 5);
    assert_eq!(ops[1].1, 14);
    assert_eq!(ops[1].2, "Concatenate([1.0, 0.0, 0.0, 1.0, 5.0, 5.0])");
    assert_eq!(ops[2].2, "Font { name: /F1, size: 12.0 }");
    assert_eq!(
        ops[3].2,
        "ShowTextAdjusted([Show([97]), Adjust(-20.0), Show([98])])"
    );
    assert_eq!(ops[4].0, Operator::Restore);
}

#[test]
fn operands_of_one_operation_do_not_reach_the_next() {
    let ops = ops(b"(ab) Tj <63> Tj [(d) 1] TJ 0.5 0.25 sc 1 sc").unwrap();
    let debug: Vec<_> = ops.iter().map(|(_, _, op)| op.as_str()).collect();
    assert_eq!(
        debug,
        [
            "ShowText([97, 98])",
            "ShowText([99])",
            "ShowTextAdjusted([Show([100]), Adjust(1.0)])",
            "FillColor(ColorOperands { components: [0.5, 0.25], pattern: None })",
            "FillColor(ColorOperands { components: [1.0], pattern: None })",
        ]
    );
}

#[test]
fn unknown_operators_are_skipped_only_in_compatibility_sections() {
    let read = ops(b"BX 1 2 foo EX q").unwrap();
    let kinds: Vec<_> = read.iter().map(|o| o.0).collect();
    assert_eq!(
        kinds,
        [
            Operator::BeginCompatibility,
            Operator::EndCompatibility,
            Operator::Save
        ]
    );
    let error = ops(b"q foo").unwrap_err();
    assert_eq!(error.message, "unknown content operator");
    assert_eq!(error.position, Position::Buffer(2));
    assert_eq!(ops(b"EX").unwrap_err().message, "EX without BX");
}

#[test]
fn operands_are_direct_and_bounded() {
    let error = ops(b"[1 0 R] 0 d").unwrap_err();
    assert_eq!(error.message, "indirect reference in content operand");
    let many = "0 ".repeat(33) + "sc";
    let error = ops(many.as_bytes()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.position, Position::Buffer(64));
}

#[test]
fn every_token_is_charged() {
    let content = Content::from_bytes(b"0 0 m 1 1 l".as_slice());
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = Meter::new(5, "test");
    reader.next(&mut tokens).unwrap();
    let error = reader.next(&mut tokens).unwrap_err();
    assert_eq!(
        (error.kind, error.position),
        (ErrorKind::Limit, Position::Buffer(10))
    );
}

#[test]
fn inline_images_are_framed_by_the_caller_and_end_with_ei() {
    let bytes = b"q BI /W 2 /H 1 /BPC 8 /CS /G ID \x00\xffx EI Q";
    let content = Content::from_bytes(bytes.as_slice());
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = meter();
    assert_eq!(reader.next(&mut tokens).unwrap().unwrap().op, Op::Save);
    let image = reader.next(&mut tokens).unwrap().unwrap();
    assert_eq!(image.offset, 2);
    let Op::InlineImage(dictionary) = &image.op else {
        panic!("{image:?}")
    };
    assert_eq!(dictionary.len(), 4);
    assert!(reader.next(&mut tokens).is_err());
    let data = reader
        .inline_image_data(|rest| {
            assert!(rest.starts_with(b"\x00\xff"));
            Ok(3)
        })
        .unwrap();
    assert_eq!(
        data,
        InlineData {
            data: b"\x00\xffx",
            unspaced: false
        }
    );
    assert_eq!(reader.next(&mut tokens).unwrap().unwrap().op, Op::Restore);
    assert!(reader.next(&mut tokens).unwrap().is_none());

    for bytes in [b"BI /W 1 ID \x00 Q".as_slice(), b"BI /W 1 ID \x00EIQ"] {
        let content = Content::from_bytes(bytes);
        let mut reader = Reader::new(&content, &Limits::default());
        reader.next(&mut tokens).unwrap();
        let error = reader.inline_image_data(|_| Ok(1)).unwrap_err();
        assert_eq!(
            (error.message, error.position),
            ("inline image requires EI", Position::Buffer(0))
        );
    }
}

#[test]
fn an_ei_directly_after_the_data_is_read_and_marked() {
    let content = Content::from_bytes(b"BI /W 1 ID \xffEI Q".as_slice());
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = meter();
    reader.next(&mut tokens).unwrap();
    let data = reader.inline_image_data(|_| Ok(1)).unwrap();
    assert_eq!(
        data,
        InlineData {
            data: b"\xff",
            unspaced: true
        }
    );
    assert_eq!(reader.next(&mut tokens).unwrap().unwrap().op, Op::Restore);
}

#[test]
fn errors_name_the_stream_holding_the_token() {
    let content = Content::from_streams(
        [
            (ObjectId::new(3, 0), Cow::Borrowed(b"q".as_slice())),
            (ObjectId::new(5, 0), Cow::Borrowed(b"Q bad".as_slice())),
        ],
        &Limits::default(),
    )
    .unwrap();
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = meter();
    reader.next(&mut tokens).unwrap();
    reader.next(&mut tokens).unwrap();
    let error = reader.next(&mut tokens).unwrap_err();
    assert_eq!(
        (error.position, error.object),
        (
            Position::Stream {
                object: ObjectId::new(5, 0),
                offset: 2
            },
            Some(ObjectId::new(5, 0))
        )
    );
}

#[test]
fn pending_operands_and_sections_leave_the_reader_unbalanced() {
    for bytes in [b"0 0".as_slice(), b"BX"] {
        let content = Content::from_bytes(bytes);
        let mut reader = Reader::new(&content, &Limits::default());
        let mut tokens = meter();
        while reader.next(&mut tokens).unwrap().is_some() {}
        assert!(!reader.is_balanced());
    }
}

#[test]
fn only_pending_operands_are_discarded() {
    for (bytes, pending) in [
        (b"0 0".as_slice(), true),
        (b"q 0 0 m", false),
        (b"BX", false),
    ] {
        let content = Content::from_bytes(bytes);
        let mut reader = Reader::new(&content, &Limits::default());
        let mut tokens = meter();
        while reader.next(&mut tokens).unwrap().is_some() {}
        assert_eq!(reader.discard_operands(), pending, "{bytes:?}");
        assert!(!reader.discard_operands());
        assert_eq!(reader.is_balanced(), bytes != b"BX");
    }
}

/// What an error is: kind, message, position and repair tag.
type Identity = (ErrorKind, &'static str, Position, Option<Repair>);

fn identity(error: &Error) -> Identity {
    (error.kind, error.message, error.position, error.repair)
}

/// The operators and offsets of `bytes` as a tokenizer that reads every
/// operand with the object parser reads them: an integer followed by
/// `G R` is one reference operand there. Operand types are not checked.
fn parser_tokens(
    bytes: &[u8],
    limits: &Limits,
    budget: usize,
) -> std::result::Result<Vec<(Operator, usize)>, Identity> {
    let mut parser = Parser::new(bytes, Origin::Buffer, *limits);
    let mut tokens = Meter::new(budget, "content token budget exceeded");
    let (mut operands, mut compatibility, mut out) = (0, 0_usize, Vec::new());
    while let Some(byte) = parser.peek() {
        let offset = parser.offset();
        let mut step = || -> Result<()> {
            tokens.charge(1)?;
            let operand = if matches!(
                byte,
                b'/' | b'(' | b'[' | b'<' | b'+' | b'-' | b'.' | b'0'..=b'9'
            ) {
                Some(parser.parse()?)
            } else {
                match parser.word()? {
                    b"true" | b"false" | b"null" => Some(Value::Null),
                    word => {
                        let Some(operator) = Operator::from_bytes(word) else {
                            if compatibility == 0 {
                                return Err(Error::syntax("unknown content operator"));
                            }
                            operands = 0;
                            return Ok(());
                        };
                        match operator {
                            Operator::BeginCompatibility => {
                                compatibility += 1;
                                if compatibility > limits.depth {
                                    return Err(Error::limit("content nesting budget exceeded"));
                                }
                            }
                            Operator::EndCompatibility => {
                                compatibility = compatibility
                                    .checked_sub(1)
                                    .ok_or(Error::syntax("EX without BX"))?;
                            }
                            _ => {}
                        }
                        operands = 0;
                        out.push((operator, offset));
                        None
                    }
                }
            };
            if let Some(value) = operand {
                if operands >= MAX_OPERANDS {
                    return Err(Error::limit("content operand budget exceeded"));
                }
                no_references(&value)?;
                operands += 1;
            }
            Ok(())
        };
        step().map_err(|error| {
            let position = match error.position {
                Position::Unknown => Position::Buffer(offset),
                position => position,
            };
            identity(&error.at(position))
        })?;
    }
    Ok(out)
}

/// The operators and offsets the reader yields.
fn reader_tokens(
    bytes: &[u8],
    limits: &Limits,
    budget: usize,
) -> std::result::Result<Vec<(Operator, usize)>, Identity> {
    let content = Content::from_bytes(bytes);
    let mut reader = Reader::new(&content, limits);
    let mut tokens = Meter::new(budget, "content token budget exceeded");
    let mut out = Vec::new();
    loop {
        match reader.next(&mut tokens) {
            Ok(Some(operation)) => out.push((operator(&operation.op), operation.offset)),
            Ok(None) => return Ok(out),
            Err(error) => return Err(identity(&error)),
        }
    }
}

#[test]
fn references_fail_as_the_object_parser_reads_them_under_every_budget() {
    let zeros = |n: usize| "0 ".repeat(n);
    let mut inputs: Vec<String> = [
        "1 0 R",
        "q 1 0 R Q",
        "01 0 R",
        "1 00 R",
        "0 0 R",
        "00 0 R",
        "4294967295 0 R",
        "4294967296 0 R",
        "1 65535 R",
        "1 65536 R",
        "1 +0 R",
        "+1 0 R",
        "1.0 0 R",
        "1 0.0 R",
        "5 1 0 R",
        "1 2 R",
        "1 0 %c\nR",
        "1 %c\r0 R",
        "1 0 Rx",
        "1 0 R1",
        "1 0 0 RG",
        "R",
        "1 R",
        "/a 0 R",
        "(a) 0 R",
        "BX 1 0 R EX",
        "BX 1 0 foo EX",
        "BX 1 R EX",
        "[1 0 R] TJ",
        "[(a) 1 0 R] TJ",
        "[01 0 R] TJ",
        "/T <</A 1 0 R>> BDC",
        "/T <</A 0 0 R>> BDC",
        "12345678901234567890 0 R",
        "0000000000000001 0 R",
        "1 0000000000000000 R",
        "[(a) 1 (b)] TJ (c) Tj <41> Tj /F1 12 Tf [3 1] 0 d",
        "/T <</MCID 0>> BDC EMC BX true false null foo EX",
        "(a\\(b\\)) Tj (ab) Tj <4142> Tj <41 4> Tj () Tj <> Tj",
        "(unterminated Tj",
        "<41x> Tj",
        "/A#20B 1 Tf /#41 1 Tf",
        "[(a) <62> 1 .5] TJ [] TJ",
        "1.2.3 0 m",
        "99999999999999999999 0 m",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for n in [30, 31, 32] {
        inputs.push(format!("{}1 0 R", zeros(n)));
        inputs.push(format!("{}1 R", zeros(n)));
        inputs.push(format!("{}01 0 R", zeros(n)));
        inputs.push(format!("{}0 0 R", zeros(n)));
        inputs.push(format!("{}1 0 0 R", zeros(n)));
    }
    let limits = [
        Limits::default(),
        Limits {
            value_nodes: 0,
            ..Limits::default()
        },
        Limits {
            value_nodes: 3,
            ..Limits::default()
        },
        Limits {
            depth: 0,
            ..Limits::default()
        },
        Limits {
            string_bytes: 1,
            ..Limits::default()
        },
    ];
    for input in &inputs {
        for limits in &limits {
            for budget in 0..=40 {
                assert_eq!(
                    reader_tokens(input.as_bytes(), limits, budget),
                    parser_tokens(input.as_bytes(), limits, budget),
                    "{input:?} under {limits:?} with {budget} tokens"
                );
            }
        }
    }
}
