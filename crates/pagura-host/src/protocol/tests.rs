//! Wire layouts against independently written byte oracles.

use super::*;
use pagura::{ErrorKind, Feature, Limits, ObjectId, Repair};

#[test]
fn hello_has_an_independent_literal_wire_oracle() {
    // host-wire-v1: length=9, response kind=129, id=7, version=1.
    assert_eq!(
        encode(7, &Response::Hello(Version::V1), 9).unwrap(),
        [9, 0, 0, 0, 129, 7, 0, 0, 0, 1, 0, 0, 0]
    );
    assert_eq!(
        decode(&[1, 7, 0, 0, 0, 1, 0, 0, 0], &Limits::default()).unwrap(),
        RequestFrame {
            id: 7,
            request: Request::Hello(Field::new(1, 5))
        }
    );
    assert_eq!(
        encode(7, &Response::Hello(Version::V1), 8)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn error_wire_preserves_feature_object_and_stream_position() {
    // host-structured-errors: independently constructed byte layout, not a roundtrip.
    let object = ObjectId::new(42, 3);
    let error = Error::unsupported(Feature::new("operator.Tj"), "text")
        .in_object(object)
        .at(Position::Stream {
            object: ObjectId::new(9, 2),
            offset: 17,
        });
    let actual = encode(5, &Response::Error(error), 1024).unwrap();
    let mut expected = vec![255, 5, 0, 0, 0, 3, 11, 0, 0, 0];
    expected.extend(b"operator.Tj");
    expected.extend([0; 4]); // No refused repair.
    expected.extend([4, 0, 0, 0]);
    expected.extend(b"text");
    expected.extend([
        1, 42, 0, 0, 0, 3, 0, 2, 17, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0,
    ]);
    assert_eq!(&actual[4..], expected);
    let diagnostics = Response::Diagnostics(vec![
        Diagnostic::new(Repair::new("syntax.duplicate-key"), "duplicate")
            .at(Position::File(10))
            .in_object(object),
    ]);
    let bytes = encode(6, &diagnostics, 1024).unwrap();
    assert!(bytes.windows(20).any(|s| s == b"syntax.duplicate-key"));
    for position in [Position::Unknown, Position::File(4), Position::Buffer(5)] {
        assert!(encode(8, &Response::Error(Error::syntax("bad").at(position)), 1024).is_ok());
    }
}

#[test]
fn strict_repair_has_an_independent_frozen_v1_wire_oracle() {
    let error = Diagnostic::new(Repair::new("file.trailing-bytes"), "trailing")
        .in_object(ObjectId::new(3, 0))
        .at(Position::File(17))
        .to_error()
        .unwrap();
    let actual = encode(2, &Response::Error(error), 1024).unwrap();
    // kind/id/category, empty feature tag, refused repair, message, context.
    let mut expected = vec![255, 2, 0, 0, 0, 1, 0, 0, 0, 0, 19, 0, 0, 0];
    expected.extend(b"file.trailing-bytes");
    expected.extend([8, 0, 0, 0]);
    expected.extend(b"trailing");
    expected.extend([1, 3, 0, 0, 0, 0, 0, 1, 17, 0, 0, 0, 0, 0, 0, 0]);
    let mut frame = u32::try_from(expected.len())
        .unwrap()
        .to_le_bytes()
        .to_vec();
    frame.extend(expected);
    assert_eq!(actual, frame);
    assert_eq!(
        encode(2, &Response::Error(Error::syntax("bad")), 1024).unwrap()[10..18],
        [0; 8]
    );
}

#[test]
fn every_error_category_and_context_has_a_fixed_wire_code() {
    let kinds = [
        ErrorKind::Syntax,
        ErrorKind::Limit,
        ErrorKind::Unsupported(Feature::new("operator.Tj")),
        ErrorKind::MissingObject,
        ErrorKind::Cycle,
        ErrorKind::Password,
        ErrorKind::InvalidArgument,
    ];
    for (index, kind) in kinds.into_iter().enumerate() {
        let response = Response::Error(Error::new(kind, "failure"));
        let bytes = encode(9, &response, 1024).unwrap();
        assert_eq!(usize::from(bytes[9]), index + 1);
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]); // No object, unknown position.
    }
    let bytes = encode(
        1,
        &Response::Error(Error::syntax("bad").at(Position::Buffer(17))),
        1024,
    )
    .unwrap();
    assert_eq!(&bytes[bytes.len() - 10..], &[0, 3, 17, 0, 0, 0, 0, 0, 0, 0]);
    let bytes = encode(
        1,
        &Response::Error(Error::syntax("bad").at(Position::File(5))),
        1024,
    )
    .unwrap();
    assert_eq!(&bytes[bytes.len() - 10..], &[0, 1, 5, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn generation_layouts_are_additive_and_truncations_fail() {
    assert_eq!(
        encode(9, &Response::Skipped(1), 1024).unwrap(),
        [10, 0, 0, 0, 136, 9, 0, 0, 0, 0, 1, 0, 0, 0]
    );
    assert_eq!(
        encode(8, &Response::Generation(2), 1024).unwrap(),
        [9, 0, 0, 0, 135, 8, 0, 0, 0, 2, 0, 0, 0]
    );
    let mut bodies = vec![vec![7, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]];
    let mut tile = vec![8, 3, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
    tile.extend(1_f64.to_le_bytes());
    for value in [0_u32, 0, 1, 1] {
        tile.extend(value.to_le_bytes());
    }
    bodies.push(tile);
    for body in bodies {
        assert!(decode(&body, &Limits::default()).is_ok());
        for end in 0..body.len() {
            assert!(decode(&body[..end], &Limits::default()).is_err());
        }
    }
}

#[test]
fn decoding_rejects_every_truncation_and_trailing_bytes() {
    let bodies = [
        vec![1, 1, 0, 0, 0, 1, 0, 0, 0],
        vec![2, 2, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, b'x'],
        vec![3, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
        vec![
            4, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0, 0,
            0, 1, 0, 0, 0, 1, 0, 0, 0,
        ],
        vec![5, 5, 0, 0, 0, 1, 0, 0, 0],
        vec![6, 6, 0, 0, 0, 1, 0, 0, 0],
    ];
    for body in bodies {
        assert!(decode(&body, &Limits::default()).is_ok());
        for cut in 0..body.len() {
            let error = decode(&body[..cut], &Limits::default()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax);
            assert!(matches!(error.position, Position::Buffer(_)));
        }
        let mut extra = body;
        extra.push(0);
        assert_eq!(
            decode(&extra, &Limits::default()).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }
    let invalid_mode = [2, 1, 0, 0, 0, 2];
    assert_eq!(
        decode(&invalid_mode, &Limits::default()).unwrap_err().kind,
        ErrorKind::Syntax
    );
    let too_long_password = [2, 1, 0, 0, 0, 0, 255, 255, 255, 255];
    assert_eq!(
        decode(&too_long_password, &Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let bad_tile = Response::Tile(Pixels::new(1, 1, vec![0; 3]));
    assert_eq!(
        encode(1, &bad_tile, 1024).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn diagnostic_kinds_have_independent_wire_oracles_and_legacy_filtering() {
    let information = Diagnostic::information("test.same", "info");
    let repair = Diagnostic::new(Repair::new("test.same"), "repair");
    let records = vec![information, repair];
    // Typed kind/id/count, then information=1 and repair=0, with Unknown contexts.
    let mut expected = vec![145, 9, 0, 0, 0, 2, 0, 0, 0, 1, 9, 0, 0, 0];
    expected.extend(b"test.same");
    expected.extend([4, 0, 0, 0]);
    expected.extend(b"info");
    expected.extend([0, 0]);
    let mut repair_bytes = vec![9, 0, 0, 0];
    repair_bytes.extend(b"test.same");
    repair_bytes.extend([6, 0, 0, 0]);
    repair_bytes.extend(b"repair");
    repair_bytes.extend([0, 0]);
    expected.push(0);
    expected.extend(&repair_bytes);
    let actual = encode(9, &Response::DiagnosticDetails(records.clone()), 1024).unwrap();
    assert_eq!(&actual[4..], expected);
    assert_eq!(
        encode(
            9,
            &Response::DiagnosticDetails(records.clone()),
            expected.len() - 1
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
    let mut legacy = vec![133, 9, 0, 0, 0, 1, 0, 0, 0];
    legacy.extend(repair_bytes);
    assert_eq!(
        &encode(9, &Response::Diagnostics(records), 1024).unwrap()[4..],
        legacy
    );
}

#[test]
fn code_tables_round_trip_and_reject_unassigned_codes() {
    fn round_trip<C>(all: &[C], code: impl Fn(C) -> u8)
    where
        C: Copy + std::fmt::Debug + PartialEq + TryFrom<u8, Error = Error>,
    {
        let mut assigned = [false; 256];
        for &value in all {
            assigned[usize::from(code(value))] = true;
            assert_eq!(C::try_from(code(value)).unwrap(), value);
        }
        for byte in 0..=u8::MAX {
            assert_eq!(
                C::try_from(byte).is_ok(),
                assigned[usize::from(byte)],
                "{byte}"
            );
        }
    }
    round_trip(RequestKind::ALL, RequestKind::code);
    round_trip(ResponseKind::ALL, ResponseKind::code);
    round_trip(ErrorCategory::ALL, ErrorCategory::code);
    round_trip(DiagnosticCode::ALL, DiagnosticCode::code);
    round_trip(PositionKind::ALL, PositionKind::code);
    round_trip(TileStatus::ALL, TileStatus::code);
    round_trip(FitCode::ALL, FitCode::code);
    round_trip(ActionCode::ALL, ActionCode::code);
    // Responses answer requests with the high bit set (PROTOCOL.md).
    for &kind in RequestKind::ALL {
        assert_eq!(kind.response().code(), kind.code() | 0x80);
    }
    assert_eq!(
        Version::ALL.iter().map(|v| v.code()).collect::<Vec<_>>(),
        [1, 0x0001_0001, 0x0001_0002, 0x0001_0003, 0x0001_0004]
    );
    assert!(Version::try_from(2).is_err());
    assert_eq!(Version::LATEST, *Version::ALL.last().unwrap());
    assert_eq!(SearchFlags::from_bits(7).map(SearchFlags::bits), Some(7));
    assert_eq!(SearchFlags::from_bits(8), None);
    assert_eq!(OutlineStyle::from_bits(4), None);
    assert_eq!(
        OpenFlags::from_bits(0).map(OpenFlags::recovery),
        Some(Recovery::Strict)
    );
    assert_eq!(
        OpenFlags::from_bits(1).map(OpenFlags::recovery),
        Some(Recovery::Repair)
    );
    assert_eq!(OpenFlags::from_bits(2), None);
}
