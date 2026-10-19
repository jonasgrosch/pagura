#![forbid(unsafe_code)]
//! The DER reader, the reported BER subset, budgets and primitive views
//! (X.690 8, 10 and 11).

use pagura_core::{
    Diagnostic, Diagnostics, ErrorKind, Limits, Position, Recovery, SignatureLimits,
};
use pagura_pki::cms::OctetString;
use pagura_pki::der::{self, Class, Document, Form, Tag};
use pagura_pki::types::{EcdsaSignature, StringValue};
use pagura_pki::{BitString, Integer, ObjectIdentifier, Time, repairs};
use pagura_testkit::unhex;

#[test]
fn malformed_der_has_precise_errors() {
    // X.690 8.1.2, 8.3, 8.6, 8.19, 10.1, 11.1-11.2.
    let cases = [
        ("0200", 2),
        ("02020001", 2),
        ("0202ffff", 2),
        ("010101", 2),
        ("010200ff", 2),
        ("050100", 2),
        ("030108", 2),
        ("03020301", 2),
        ("030101", 2),
        ("0600", 2),
        ("060180", 2),
        ("06022a80", 3),
        ("048100", 1),
        ("04820080", 1),
        ("0480", 1),
        ("04ff", 1),
        ("1f020100", 0),
        ("1f80020100", 0),
        ("3003040200", 4),
        ("0000", 2),
        ("05000000", 2),
        ("3001", 2),
        ("300105", 3),
    ];
    for (encoded, offset) in cases {
        let bytes = unhex(encoded);
        let error = Document::parse(&bytes, &Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{encoded}: {error}");
        assert_eq!(error.position, Position::Buffer(offset), "{encoded}");
    }
}

#[test]
fn der_deviations_name_the_repair_ber_would_apply() {
    for (encoded, repair) in [
        ("0480", repairs::BER_INDEFINITE),
        ("048100", repairs::BER_LENGTH),
        ("2403040161", repairs::BER_OCTETS),
    ] {
        let error = Document::parse(&unhex(encoded), &Limits::default()).unwrap_err();
        assert_eq!(error.repair, Some(repair), "{encoded}");
    }
}

#[test]
fn tag_classes_large_tags_and_length_overflow() {
    let limits = Limits::default();
    for (encoded, class, number) in [
        ("5f81000100", Class::Application, 128),
        ("9f1f0100", Class::Context, 31),
        ("df7f0100", Class::Private, 127),
    ] {
        let bytes = unhex(encoded);
        let doc = Document::parse(&bytes, &limits).unwrap();
        assert_eq!(doc.root().tag(), Tag::new(class, Form::Primitive, number));
    }
    for (encoded, kind) in [
        ("1fffffffffff7f00", ErrorKind::Limit),
        ("0488ffffffffffffffff", ErrorKind::Limit),
        ("0489010000000000000000", ErrorKind::Limit),
    ] {
        let bytes = unhex(encoded);
        assert_eq!(
            Document::parse(&bytes, &limits).unwrap_err().kind,
            kind,
            "{encoded}"
        );
    }
}

#[test]
fn named_tags_match_their_identifier_octets() {
    for (encoded, tag) in [
        ("010100", der::BOOLEAN),
        ("020100", der::INTEGER),
        ("030100", der::BIT_STRING),
        ("0400", der::OCTET_STRING),
        ("0500", der::NULL),
        ("06012a", der::OBJECT_IDENTIFIER),
        ("0a0100", der::ENUMERATED),
        ("0c00", der::UTF8_STRING),
        ("0d0101", der::RELATIVE_OID),
        ("3000", der::SEQUENCE),
        ("3100", der::SET),
        ("1300", der::PRINTABLE_STRING),
        ("1600", der::IA5_STRING),
        ("1e00", der::BMP_STRING),
    ] {
        let bytes = unhex(encoded);
        let document = Document::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(document.root().tag(), tag, "{encoded}");
    }
    assert!(der::SEQUENCE.is_constructed());
    assert_eq!(Tag::context(3, Form::Constructed).class, Class::Context);
}

#[test]
fn ber_deviations_are_explicit_and_bounded() {
    let bytes = unhex("24800481016124030401620000");
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let document = Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics).unwrap();
    assert_eq!(
        OctetString::parse(document.root())
            .unwrap()
            .to_bytes()
            .unwrap(),
        b"ab"
    );
    assert_eq!(
        diagnostics
            .as_slice()
            .iter()
            .filter_map(Diagnostic::repair)
            .collect::<Vec<_>>(),
        [
            repairs::BER_INDEFINITE,
            repairs::BER_OCTETS,
            repairs::BER_LENGTH,
            repairs::BER_OCTETS
        ]
    );
    let mut strict = Diagnostics::new(Recovery::Strict);
    let error = Document::parse_ber(&bytes, &Limits::default(), &mut strict).unwrap_err();
    assert_eq!(error.repair, Some(repairs::BER_INDEFINITE));
    assert_eq!(error.position, Position::Buffer(1));
    assert_eq!(strict.as_slice().len(), 0);
    // The concatenated copy is charged to its own asn1_bytes budget.
    let mut long = vec![0x24, 0x80, 0x04, 0x82, 0x07, 0xd0];
    long.resize(long.len() + 2000, b'a');
    long.extend_from_slice(&[0, 0]);
    let limits = Limits {
        signatures: SignatureLimits {
            asn1_bytes: 1500,
            ..Limits::default().signatures
        },
        ..Limits::default()
    };
    let document = Document::parse_ber(&long, &limits, &mut diagnostics).unwrap();
    let octets = OctetString::parse(document.root()).unwrap();
    assert_eq!(octets.to_bytes().unwrap_err().kind, ErrorKind::Limit);
    assert_eq!(octets.to_bytes().unwrap_err().position, Position::Buffer(0));
    for encoded in ["308002010100", "300230800000", "04800000", "30800001000000"] {
        let bytes = unhex(encoded);
        assert!(
            Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics).is_err(),
            "{encoded}"
        );
    }
    let bytes = unhex("2403020101");
    assert!(Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics).is_err());
}

#[test]
fn indefinite_values_end_at_their_end_of_contents() {
    let bytes = unhex("30803080020100000005000000");
    let wrapped = [&[0x30, 0x0d][..], &bytes].concat();
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let document = Document::parse_ber(&wrapped, &Limits::default(), &mut diagnostics).unwrap();
    let outer = document.root().children().next().unwrap();
    assert_eq!(outer.range(), 2..15);
    assert!(outer.is_indefinite());
    let mut children = outer.children();
    let inner = children.next().unwrap();
    assert_eq!(inner.encoded(), unhex("30800201000000"));
    assert_eq!(inner.content(), [2, 1, 0]);
    assert_eq!(children.next().unwrap().tag(), der::NULL);
    assert!(children.next().is_none());
    assert_eq!(document.root().walk().count(), 5);
}

#[test]
fn set_of_order_is_a_schema_check() {
    let bytes = unhex("3106020102020101");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let error = document.root().set_of().unwrap_err();
    assert_eq!(error.position, Position::Buffer(5));
    let bytes = unhex("3106020101020101");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    assert_eq!(document.root().set_of().unwrap().count(), 2);
}

#[test]
fn reader_budgets_fail_closed_and_deep_trees_drop_iteratively() {
    let bytes = unhex("3003020101");
    for limits in [
        Limits {
            signatures: SignatureLimits {
                asn1_depth: 1,
                ..Limits::default().signatures
            },
            ..Limits::default()
        },
        Limits {
            signatures: SignatureLimits {
                asn1_elements: 1,
                ..Limits::default().signatures
            },
            ..Limits::default()
        },
        Limits {
            signatures: SignatureLimits {
                asn1_bytes: 0,
                ..Limits::default().signatures
            },
            ..Limits::default()
        },
    ] {
        let error = Document::parse(&bytes, &limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{limits:?}");
        assert_ne!(error.position, Position::Unknown);
    }
    let exact = Limits {
        signatures: SignatureLimits {
            asn1_depth: 2,
            asn1_elements: 2,
            ..Limits::default().signatures
        },
        ..Limits::default()
    };
    assert_eq!(
        Document::parse(&bytes, &exact)
            .unwrap()
            .root()
            .walk()
            .count(),
        2
    );
    let depth = 4096;
    let mut bytes = Vec::new();
    for _ in 0..depth {
        bytes.extend_from_slice(&[0x30, 0x80]);
    }
    bytes.extend_from_slice(&[5, 0]);
    bytes.resize(bytes.len() + 2 * depth, 0);
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    assert_eq!(
        Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let limits = Limits {
        signatures: SignatureLimits {
            asn1_depth: u32::try_from(depth + 1).unwrap(),
            ..Limits::default().signatures
        },
        ..Limits::default()
    };
    let document = Document::parse_ber(&bytes, &limits, &mut diagnostics).unwrap();
    assert_eq!(document.root().walk().count(), depth + 1);
    assert_eq!(document.root().encoded(), bytes);
}

#[test]
fn integer_oid_bits_and_string_views_preserve_contents() {
    let bytes = unhex("020900ffffffffffffffff");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let integer = Integer::parse(document.root()).unwrap();
    assert_eq!(integer.to_u64(), Some(u64::MAX));
    assert_eq!(integer.as_bytes().len(), 9);
    for encoded in ["0201ff", "0209010000000000000000"] {
        let bytes = unhex(encoded);
        let document = Document::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(Integer::parse(document.root()).unwrap().to_u64(), None);
    }
    let bytes = unhex("0201ff");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    assert!(Integer::parse(document.root()).unwrap().unsigned().is_err());
    let bytes = unhex("060788808080808000");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    assert_eq!(
        ObjectIdentifier::parse(document.root()).unwrap().as_bytes(),
        &bytes[2..]
    );
    let bytes = unhex("03020780");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let bits = BitString::parse(document.root()).unwrap();
    assert!(bits.bit(0));
    assert!(!bits.bit(1));
    assert!(!bits.bit(8));
    for encoded in [
        "0c02c3bc",
        "1303455531",
        "1e0200fc",
        "1c040001f600",
        "1401fc",
    ] {
        let bytes = unhex(encoded);
        let doc = Document::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(
            StringValue::parse(doc.root()).unwrap().as_bytes(),
            &bytes[2..]
        );
    }
    for encoded in ["0c01ff", "130140", "160180", "1e02d800", "1c0400110000"] {
        let bytes = unhex(encoded);
        assert!(
            Document::parse(&bytes, &Limits::default()).is_err(),
            "{encoded}"
        );
    }
}

#[test]
fn integer_value_identity_excludes_error_location() {
    use std::collections::HashSet;

    let limits = Limits::default();
    let first = Document::parse(&[2, 1, 7], &limits).unwrap();
    let second = Document::parse(&[0x30, 3, 2, 1, 7], &limits).unwrap();
    let first = Integer::parse(first.root()).unwrap();
    let second = Integer::parse(second.root().children().next().unwrap()).unwrap();
    assert_eq!(first, second);
    let values = HashSet::from([first, second]);
    assert_eq!(values.len(), 1);
}

#[test]
fn ecdsa_components_are_unsigned_and_ber_envelopes_are_rechecked() {
    let bytes = unhex("3007020100020200ff");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let signature = EcdsaSignature::parse(document.root()).unwrap();
    assert!(signature.r.is_zero());
    assert_eq!(signature.s.unsigned().unwrap(), [0xff]);
    let bytes = unhex("30800201000201010000");
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let document = Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics).unwrap();
    let error = EcdsaSignature::parse(document.root()).unwrap_err();
    assert_eq!(error.repair, Some(repairs::BER_INDEFINITE));
}

#[expect(clippy::unwrap_used, reason = "test times are shorter than 128 bytes")]
fn time(tag: u8, text: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag, u8::try_from(text.len()).unwrap()];
    bytes.extend_from_slice(text);
    bytes
}

#[test]
fn pkix_and_timestamp_time_profiles_differ() {
    for (tag, text, year) in [
        (23, "500101000000Z", 1950),
        (23, "491231235959Z", 2049),
        (24, "20500228000000Z", 2050),
    ] {
        let bytes = time(tag, text.as_bytes());
        let document = Document::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(Time::parse(document.root()).unwrap().year(), year);
    }
    for text in [
        "20230229000000Z",
        "202610181200Z",
        "20261018120000+0000",
        "20261018120000.10Z",
        "20261018120000.Z",
        "20261018240000Z",
        "20261018120060Z",
        "20261318120000Z",
        "20261000120000Z",
    ] {
        let bytes = time(24, text.as_bytes());
        assert!(
            Document::parse(&bytes, &Limits::default()).is_err(),
            "{text}"
        );
    }
    assert!(Document::parse(&time(23, b"261018120000.5Z"), &Limits::default()).is_err());
    let bytes = time(24, b"20240229000000.125Z");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    assert!(Time::parse(document.root()).is_err());
    let time = Time::generalized(document.root()).unwrap();
    assert_eq!(
        (time.year(), time.month(), time.day(), time.fraction()),
        (2024, 2, 29, "125")
    );
}

#[test]
fn times_order_chronologically_including_fractions() {
    let times = [
        "20240229000000Z",
        "20240229000000.05Z",
        "20240229000000.1Z",
        "20240229000000.15Z",
        "20240229000000.2Z",
        "20240229000001Z",
    ]
    .map(|text| time(24, text.as_bytes()));
    let documents = times
        .iter()
        .map(|bytes| Document::parse(bytes, &Limits::default()).unwrap())
        .collect::<Vec<_>>();
    let parsed = documents
        .iter()
        .map(|document| Time::generalized(document.root()).unwrap())
        .collect::<Vec<_>>();
    assert!(parsed.is_sorted(), "{parsed:?}");
    let utc = time(23, b"240229000000Z");
    let utc = Document::parse(&utc, &Limits::default()).unwrap();
    assert_eq!(Time::parse(utc.root()).unwrap(), parsed[0]);
}

#[test]
fn host_times_are_checked_calendar_values() {
    let time = Time::from_utc(2024, 2, 29, 23, 59, 59).unwrap();
    assert_eq!((time.hour(), time.minute(), time.second()), (23, 59, 59));
    assert_eq!(time.fraction(), "");
    for (year, month, day, hour, minute, second) in [
        (2023, 2, 29, 0, 0, 0),
        (2024, 13, 1, 0, 0, 0),
        (2024, 4, 31, 0, 0, 0),
        (2024, 1, 0, 0, 0, 0),
        (2024, 1, 1, 24, 0, 0),
        (2024, 1, 1, 0, 60, 0),
        (2024, 1, 1, 0, 0, 60),
        (10_000, 1, 1, 0, 0, 0),
    ] {
        let error = Time::from_utc(year, month, day, hour, minute, second).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArgument);
    }
}

#[test]
fn prefix_parsing_keeps_value_zeros_and_leaves_the_suffix() {
    let bytes = [4, 3, 1, 0, 0, 0, 0];
    let (tree, consumed) = Document::parse_prefix(&bytes, &Limits::default()).unwrap();
    assert_eq!(consumed, 5);
    assert_eq!(tree.root().content(), [1, 0, 0]);
    assert_eq!(tree.root().encoded(), &bytes[..5]);
    assert!(Document::parse(&bytes, &Limits::default()).is_err());
    let bytes = [0x30, 0x80, 4, 1, 0, 0, 0, 0, 0];
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let (tree, consumed) =
        Document::parse_ber_prefix(&bytes, &Limits::default(), &mut diagnostics).unwrap();
    assert_eq!(consumed, 7);
    assert_eq!(tree.root().encoded(), &bytes[..7]);
    assert_eq!(
        diagnostics.as_slice()[0].repair(),
        Some(repairs::BER_INDEFINITE)
    );
}

#[test]
fn ber_set_order_is_reported_and_der_stays_strict() {
    let bytes = [0x31, 6, 2, 1, 2, 2, 1, 1];
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let tree = Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics).unwrap();
    let members = tree
        .root()
        .set_of_with_diagnostics(&mut diagnostics)
        .unwrap();
    assert_eq!(members.count(), 2);
    assert_eq!(
        diagnostics.as_slice()[0].repair(),
        Some(repairs::BER_SET_ORDER)
    );
    let mut strict = Diagnostics::new(Recovery::Strict);
    assert!(tree.root().set_of_with_diagnostics(&mut strict).is_err());
    let tree = Document::parse(&bytes, &Limits::default()).unwrap();
    assert!(
        tree.root()
            .set_of_with_diagnostics(&mut diagnostics)
            .is_err()
    );
}
