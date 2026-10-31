#![forbid(unsafe_code)]
//! Exact caller-owned boundaries and reported inline separator repair.
use pagura_content::{Content, Op, Reader, repairs};
use pagura_core::{Diagnostics, Error, ErrorKind, Limits, Meter, ObjectId, Position, Recovery};
use std::borrow::Cow;

#[test]
fn repair_retains_every_sample_and_resumes_after_the_complete_token() {
    // The payload itself contains EI and a final whitespace-valued sample.
    let bytes = b"BI /W 4 ID EIx\0EI Q";
    let object = ObjectId::new(7, 2);
    let content = Content::from_streams(
        [(object, Cow::Borrowed(bytes.as_slice()))],
        &Limits::default(),
    )
    .unwrap();
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let mut reader = Reader::new(&content, &Limits::default());
        let mut tokens = Meter::new(64, "test tokens");
        reader.next(&mut tokens).unwrap();
        let mut diagnostics = Diagnostics::new(recovery);
        let result = reader.inline_image_data_with_diagnostics(|_| Ok(4), &mut diagnostics);
        let position = Position::Stream { object, offset: 15 };
        if recovery == Recovery::Strict {
            let error = result.unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax);
            assert_eq!(error.repair, Some(repairs::EI_SEPARATOR));
            assert_eq!((error.position, error.object), (position, Some(object)));
            assert_eq!(diagnostics.take(), Vec::<pagura_core::Diagnostic>::new());
        } else {
            assert_eq!(result.unwrap(), b"EIx\0");
            let reported = diagnostics.take();
            assert_eq!(reported.len(), 1);
            assert_eq!(reported[0].repair(), Some(repairs::EI_SEPARATOR));
            assert_eq!(
                (reported[0].position, reported[0].object),
                (position, Some(object))
            );
            assert_eq!(reader.next(&mut tokens).unwrap().unwrap().op, Op::Restore);
            assert!(reader.next(&mut tokens).unwrap().is_none());
        }
    }
}

#[test]
fn original_entry_point_stays_strict_without_reporting_arguments() {
    let content = Content::from_bytes(b"BI ID xEI Q".as_slice());
    let mut reader = Reader::new(&content, &Limits::default());
    reader.next(&mut Meter::new(16, "test tokens")).unwrap();
    let error = reader.inline_image_data(|_| Ok(1)).unwrap_err();
    assert_eq!(
        (error.kind, error.message),
        (ErrorKind::Syntax, "inline image requires EI")
    );
    assert!(error.repair.is_none());
}

#[test]
fn generated_content_reports_a_buffer_position_without_an_object() {
    let content = Content::from_bytes(b"BI ID xEI Q".as_slice());
    let mut reader = Reader::new(&content, &Limits::default());
    reader.next(&mut Meter::new(16, "test tokens")).unwrap();
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    assert_eq!(
        reader
            .inline_image_data_with_diagnostics(|_| Ok(1), &mut diagnostics)
            .unwrap(),
        b"x"
    );
    let diagnostics = diagnostics.take();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].position, Position::Buffer(7));
    assert_eq!(diagnostics[0].object, None);
}

#[test]
fn invalid_tokens_or_lengths_never_consume_repair_permission() {
    for (bytes, length) in [
        (b"BI ID xEIQ Q".as_slice(), 1),
        (b"BI ID xEIx Q", 1),
        (b"BI ID x Q", 1),
        (b"BI ID x%comment\nEI Q", 1),
        (b"BI ID x", 1),
        (b"BI ID xEI Q", 20),
    ] {
        let content = Content::from_bytes(bytes);
        let mut reader = Reader::new(&content, &Limits::default());
        reader.next(&mut Meter::new(16, "test tokens")).unwrap();
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        assert!(
            reader
                .inline_image_data_with_diagnostics(|_| Ok(length), &mut diagnostics)
                .is_err()
        );
        assert_eq!(diagnostics.take(), Vec::<pagura_core::Diagnostic>::new());
    }
}

#[test]
fn supplied_boundary_errors_and_overflow_remain_errors() {
    for length in [Err(Error::limit("framing work exhausted")), Ok(usize::MAX)] {
        let content = Content::from_bytes(b"BI ID xEI Q".as_slice());
        let mut reader = Reader::new(&content, &Limits::default());
        reader.next(&mut Meter::new(16, "test tokens")).unwrap();
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let error = reader
            .inline_image_data_with_diagnostics(|_| length, &mut diagnostics)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(diagnostics.take(), Vec::<pagura_core::Diagnostic>::new());
    }
}

#[test]
fn conforming_separator_needs_no_repair_in_either_mode() {
    for separator in [0, b'\t', b'\n', b'\x0c', b'\r', b' '] {
        let mut bytes = b"BI ID x".to_vec();
        bytes.extend([separator, b'E', b'I', b' ', b'Q']);
        let content = Content::from_bytes(bytes);
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let mut reader = Reader::new(&content, &Limits::default());
            reader.next(&mut Meter::new(16, "test tokens")).unwrap();
            let mut diagnostics = Diagnostics::new(recovery);
            assert_eq!(
                reader
                    .inline_image_data_with_diagnostics(|_| Ok(1), &mut diagnostics)
                    .unwrap(),
                b"x"
            );
            assert_eq!(diagnostics.take(), Vec::<pagura_core::Diagnostic>::new());
        }
    }
}
