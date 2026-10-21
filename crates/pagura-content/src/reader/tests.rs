use super::*;
use pagura_core::{ErrorKind, ObjectId, Position};
use std::borrow::Cow;

fn meter() -> Meter {
    Meter::new(1_000, "test budget exceeded")
}

fn ops(bytes: &[u8]) -> Result<Vec<Operation>> {
    let content = Content::from_bytes(bytes);
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = meter();
    let mut out = Vec::new();
    while let Some(operation) = reader.next(&mut tokens)? {
        out.push(operation);
    }
    assert!(reader.is_balanced());
    Ok(out)
}

#[test]
fn operations_carry_their_operator_offsets() {
    let ops = ops(b"q 1 0 0 1 5 5 cm\n/F1 12 Tf [(a) -20 (b)] TJ Q").unwrap();
    assert_eq!(ops.len(), 5);
    assert_eq!(ops[1].offset, 14);
    assert_eq!(
        ops[1].op,
        Op::Concatenate([1.0, 0.0, 0.0, 1.0, 5.0, 5.0])
    );
    assert!(matches!(&ops[2].op, Op::Font { size, .. } if *size == 12.0));
    assert_eq!(ops[4].op, Op::Restore);
}

#[test]
fn unknown_operators_are_skipped_only_in_compatibility_sections() {
    let read = ops(b"BX 1 2 foo EX q").unwrap();
    let kinds: Vec<_> = read.iter().map(|o| o.op.operator()).collect();
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
    assert_eq!((error.kind, error.position), (ErrorKind::Limit, Position::Buffer(10)));
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
    assert_eq!(data, b"\x00\xffx");
    assert_eq!(reader.next(&mut tokens).unwrap().unwrap().op, Op::Restore);
    assert!(reader.next(&mut tokens).unwrap().is_none());

    let content = Content::from_bytes(b"BI /W 1 ID \x00 Q".as_slice());
    let mut reader = Reader::new(&content, &Limits::default());
    reader.next(&mut tokens).unwrap();
    let error = reader.inline_image_data(|_| Ok(1)).unwrap_err();
    assert_eq!(
        (error.message, error.position),
        ("inline image requires EI", Position::Buffer(0))
    );
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
