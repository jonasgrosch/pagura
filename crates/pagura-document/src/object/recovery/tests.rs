use super::end;
use crate::{Document, OpenOptions, repairs};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Diagnostics, ErrorKind, Limits, ObjectId, Recovery};
use pagura_testkit::fixture;

#[test]
fn reserved_zero_root_needs_its_exact_positive_plaintext_identity() {
    let bytes = fixture("document", "catalog-number-zero.pdf");
    let strict = Document::open(
        bytes.clone(),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(strict.repair, Some(repairs::CATALOG_OBJECT_NUMBER));
    assert_eq!(strict.object, Some(ObjectId::new(1, 0)));
    let document = Document::open(bytes, &OpenOptions::default()).unwrap();
    assert_eq!(document.page_count().unwrap(), 1);
    assert_eq!(
        document.object(ObjectId::new(1, 0)).unwrap().id(),
        ObjectId::new(1, 0)
    );
    assert_eq!(
        document
            .object(ObjectId::new(0, 0))
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::MissingObject
    );
    for name in [
        "catalog-number-zero-conflict",
        "catalog-number-zero-encrypted",
    ] {
        let bytes = fixture("document", &format!("{name}.pdf"));
        assert!(Document::open(bytes, &OpenOptions::default()).is_err());
    }
}

#[test]
fn missing_object_terminators_require_a_following_structural_token() {
    let limits = Limits::default();
    for next in [b"2 0 obj".as_slice(), b"xref", b"trailer << >>"] {
        let bytes = [b"null\n".as_slice(), next].concat();
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let mut parser = Parser::new(&bytes, Origin::File, limits);
            parser.parse().unwrap();
            let mut diagnostics = Diagnostics::new(recovery);
            let result = end(&mut parser, ObjectId::new(1, 0), &mut diagnostics);
            if recovery == Recovery::Strict {
                let error = result.unwrap_err();
                assert_eq!(error.repair, Some(repairs::OBJECT_END));
                assert_eq!(error.object, Some(ObjectId::new(1, 0)));
            } else {
                result.unwrap();
                assert_eq!(&bytes[parser.offset()..], next);
                assert_eq!(
                    diagnostics.as_slice()[0].repair(),
                    Some(repairs::OBJECT_END)
                );
            }
        }
    }
    for bytes in [
        b"null".as_slice(),
        b"null\nunknown",
        b"null\nendobject",
        b"null\n0 0 obj",
    ] {
        let mut parser = Parser::new(bytes, Origin::File, limits);
        parser.parse().unwrap();
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        assert_eq!(
            end(&mut parser, ObjectId::new(1, 0), &mut diagnostics)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
        assert!(diagnostics.is_empty());
    }
}

#[test]
fn ambiguous_stream_boundaries_are_not_repaired() {
    let mut bytes = fixture("document", "stream-length-missing.pdf");
    let start = bytes
        .windows(8)
        .position(|part| part == b"\nstream\n")
        .unwrap()
        + 8;
    bytes.splice(start..start, b"\nendstream\nendobj\n".iter().copied());
    let document = Document::open(bytes, &OpenOptions::default()).unwrap();
    let error = document.object(ObjectId::new(4, 0)).unwrap_err();
    assert_eq!(error.as_error().unwrap().kind, ErrorKind::Syntax);
    assert_eq!(
        error.as_error().unwrap().message,
        "ambiguous stream terminators"
    );
}

#[test]
fn unusable_lengths_are_repaired_lazily_and_keep_exact_bytes() {
    for name in [
        "stream-length-missing",
        "stream-length-negative",
        "stream-length-undefined",
        "stream-length-outside",
    ] {
        let bytes = fixture("document", &format!("{name}.pdf"));
        let strict = Document::open(
            bytes.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let error = strict.object(ObjectId::new(4, 0)).unwrap_err();
        assert_eq!(
            error.as_error().unwrap().repair,
            Some(repairs::LENGTH_CORRECTED),
            "{name}"
        );
        let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
        let object = document.object(ObjectId::new(4, 0)).unwrap();
        assert_eq!(
            document.stream_data(object).unwrap().as_ref(),
            b"1 0 0 rg 8 8 48 32 re f\n",
            "{name}"
        );
        assert_eq!(document.materialize().unwrap().bytes(), bytes);
        assert_eq!(
            document
                .diagnostics()
                .iter()
                .filter(|item| item.repair() == Some(repairs::LENGTH_CORRECTED))
                .count(),
            1
        );
    }
    let bytes = fixture("document", "stream-length-outside.pdf");
    let document = Document::open(
        bytes,
        &OpenOptions {
            limits: Limits {
                stream_bytes: 32,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        document
            .object(ObjectId::new(4, 0))
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
}

mod length_dependencies {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn cyclic_and_deep_lengths_remain_structured_errors_in_both_modes() {
        for (name, expected) in [
            ("length-self-cycle", ErrorKind::Cycle),
            ("length-mutual-cycle", ErrorKind::Cycle),
            ("length-chain-depth", ErrorKind::Limit),
        ] {
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let document = Document::open(
                    fixture("document", &format!("{name}.pdf")),
                    &OpenOptions {
                        recovery,
                        ..OpenOptions::default()
                    },
                )
                .unwrap();
                assert_eq!(document.page_count().unwrap(), 1);
                let error = document
                    .content_streams(document.page(0).unwrap())
                    .unwrap_err();
                assert_eq!(error.as_error().unwrap().kind, expected);
                let number = if expected == ErrorKind::Limit { 69 } else { 4 };
                assert_eq!(
                    error.as_error().unwrap().object,
                    Some(ObjectId::new(number, 0))
                );
            }
        }
    }
}
