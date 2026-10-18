use crate::cms::{OctetString, SignedData};
use crate::der::{Class, Document, Tag};
use crate::timestamp::TstInfo;
use crate::types::{EcdsaSignature, StringValue};
use crate::x509::{Certificate, CertificateList, extensions};
use crate::{BitString, Integer, ObjectIdentifier, Time};
use pagura_core::{Diagnostic, Diagnostics, ErrorKind, Limits, Position, Recovery, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug)]
struct Case {
    kind: String,
    id: String,
    success: bool,
    bytes: Vec<u8>,
}

fn unhex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2));
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

/// An independent fixture generator under `tools/fixturegen/pki`.
pub(crate) fn generator(script: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/fixturegen/pki")
        .join(script)
}

fn python(script: &str, args: &[&Path]) -> Vec<Case> {
    let output = Command::new("python3")
        .arg(generator(script))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            Case {
                kind: fields.next().unwrap().into(),
                id: fields.next().unwrap().into(),
                success: fields.next().unwrap() == "1",
                bytes: unhex(fields.next().unwrap()),
            }
        })
        .collect()
}

fn fixtures() -> &'static [Case] {
    static CASES: OnceLock<Vec<Case>> = OnceLock::new();
    CASES.get_or_init(|| python("fixtures.py", &[]))
}

fn fixture(id: &str) -> &'static [u8] {
    &fixtures().iter().find(|case| case.id == id).unwrap().bytes
}

fn parse_case(case: &Case) -> Result<()> {
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let document = if case.kind == "cms-ber" {
        Document::parse_ber(&case.bytes, &Limits::default(), &mut diagnostics)?
    } else {
        Document::parse(&case.bytes, &Limits::default())?
    };
    match case.kind.as_str() {
        "certificate" => Certificate::parse(document.root()).map(|_| ()),
        "crl" => CertificateList::parse(document.root()).map(|_| ()),
        "cms" | "cms-ber" => SignedData::from_content_info(document.root()).map(|_| ()),
        "timestamp" => TstInfo::parse(document.root()).map(|_| ()),
        "ecdsa" => EcdsaSignature::parse(document.root()).map(|_| ()),
        "signed-set" => document.root().set_of().map(|_| ()),
        _ => panic!("unknown case type"),
    }
}

#[test]
fn independently_generated_envelopes() {
    for case in fixtures() {
        let result = parse_case(case);
        assert_eq!(result.is_ok(), case.success, "{}: {result:?}", case.id);
    }
}

/// The signature reference cache. The scripts report a missing cache as absent.
pub(crate) fn references() -> PathBuf {
    std::env::var_os("PAGURA_SIGNATURE_REFERENCES_DIR").map_or_else(
        || {
            std::env::var_os("PAGURA_REFERENCES_DIR")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .map(|home| PathBuf::from(home).join(".cache/pagura-references"))
                })
                .unwrap_or_default()
                .join("signatures")
        },
        PathBuf::from,
    )
}

#[test]
fn cached_pkits_and_wycheproof_encodings() {
    let cases = python("vectors.py", &[&references()]);
    if cases.is_empty() {
        eprintln!("External PKI vectors are absent; generated encoding cases still run");
        return;
    }
    let mut certificates = 0;
    let mut crls = 0;
    let mut signatures = 0;
    for case in &cases {
        match case.kind.as_str() {
            "certificate" => certificates += 1,
            "crl" => crls += 1,
            _ => signatures += 1,
        }
        let result = parse_case(case);
        assert_eq!(result.is_ok(), case.success, "{}: {result:?}", case.id);
    }
    eprintln!(
        "External encoding cases: {certificates} PKITS certificates, {crls} PKITS CRLs, {signatures} Wycheproof ECDSA signatures"
    );
}

#[test]
fn malformed_der_has_precise_errors() {
    // X.690 §§8.1.2, 8.3, 8.6, 8.19, 10.1, 11.1–11.2.
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
    ];
    for (encoded, offset) in cases {
        let bytes = unhex(encoded);
        let error = Document::parse(&bytes, &Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{encoded}: {error}");
        assert_eq!(error.position, Position::Buffer(offset), "{encoded}");
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
        assert_eq!(
            doc.root().tag(),
            Tag {
                class,
                constructed: false,
                number
            }
        );
    }
    for encoded in [
        "1fffffffffff7f00",
        "0488ffffffffffffffff",
        "0489010000000000000000",
    ] {
        let bytes = unhex(encoded);
        assert!(Document::parse(&bytes, &limits).is_err());
    }
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
            crate::tags::BER_INDEFINITE,
            crate::tags::BER_OCTETS,
            crate::tags::BER_LENGTH,
            crate::tags::BER_OCTETS
        ]
    );
    let mut strict = Diagnostics::new(Recovery::Strict);
    let error = Document::parse_ber(&bytes, &Limits::default(), &mut strict).unwrap_err();
    assert_eq!(error.repair, Some(crate::tags::BER_INDEFINITE));
    assert_eq!(error.position, Position::Buffer(1));
    assert!(strict.is_empty());
    let limits = Limits {
        decoded_bytes: 1,
        ..Limits::default()
    };
    let limited = Document::parse_ber(&bytes, &limits, &mut diagnostics).unwrap();
    assert_eq!(
        OctetString::parse(limited.root())
            .unwrap()
            .to_bytes()
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
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
fn all_reader_budgets_fail_closed_and_deep_trees_drop_iteratively() {
    let bytes = unhex("3003020101");
    let limits = [
        Limits {
            file_bytes: 4,
            ..Limits::default()
        },
        Limits {
            depth: 1,
            ..Limits::default()
        },
        Limits {
            value_nodes: 1,
            ..Limits::default()
        },
        Limits {
            document_bytes: 0,
            ..Limits::default()
        },
        Limits {
            string_bytes: 0,
            ..Limits::default()
        },
    ];
    for limit in limits {
        assert_eq!(
            Document::parse(&bytes, &limit).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
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
        depth: depth + 1,
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
    assert_eq!(integer.to_u64().unwrap(), u64::MAX);
    assert_eq!(integer.as_bytes().len(), 9);
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
fn pkix_and_timestamp_time_profiles_differ() {
    for (tag, text, year) in [
        (23, "500101000000Z", 1950),
        (23, "491231235959Z", 2049),
        (24, "20500228000000Z", 2050),
    ] {
        let mut bytes = vec![tag, u8::try_from(text.len()).unwrap()];
        bytes.extend_from_slice(text.as_bytes());
        let document = Document::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(Time::parse(document.root()).unwrap().year, year);
    }
    for text in [
        "20230229000000Z",
        "202610181200Z",
        "20261018120000+0000",
        "20261018120000.10Z",
        "20261018240000Z",
        "20261018120060Z",
    ] {
        let mut bytes = vec![24, u8::try_from(text.len()).unwrap()];
        bytes.extend_from_slice(text.as_bytes());
        assert!(
            Document::parse(&bytes, &Limits::default()).is_err(),
            "{text}"
        );
    }
    let text = b"20240229000000.125Z";
    let mut bytes = vec![24, u8::try_from(text.len()).unwrap()];
    bytes.extend_from_slice(text);
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    assert!(Time::parse(document.root()).is_err());
    assert_eq!(Time::generalized(document.root()).unwrap().fraction, "125");
}

#[test]
fn exact_signed_ranges_and_inputs() {
    let bytes = fixture("x509-v3");
    let document = Document::parse(bytes, &Limits::default()).unwrap();
    let certificate = Certificate::parse(document.root()).unwrap();
    assert_eq!(&bytes[certificate.tbs_range()], certificate.tbs.encoded());
    assert_eq!(
        certificate.tbs.range().start,
        document.root().content_offset()
    );
    let document = Document::parse(fixture("cms-detached"), &Limits::default()).unwrap();
    let signed = SignedData::from_content_info(document.root()).unwrap();
    assert!(signed.content.is_none());
    let attrs = signed.signers[0].signed_attributes.as_ref().unwrap();
    assert_eq!(attrs.signature_input(), fixture("cms-signature-input"));
    assert_eq!(attrs.element.encoded()[0], 0xa0);
    assert_eq!(&attrs.signature_input()[1..], &attrs.element.encoded()[1..]);
    assert_eq!(
        attrs.content_digest(signed.content_type).unwrap(),
        (0..32).collect::<Vec<u8>>()
    );
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let document = Document::parse_ber(
        fixture("cms-ber-content"),
        &Limits::default(),
        &mut diagnostics,
    )
    .unwrap();
    let signed = SignedData::from_content_info(document.root()).unwrap();
    assert_eq!(signed.content.unwrap().to_bytes().unwrap(), b"abcdef");
    assert!(!diagnostics.is_empty());
}

#[test]
fn common_extensions_are_typed_and_unknown_values_remain_accessible() {
    let doc = Document::parse(fixture("x509-v3"), &Limits::default()).unwrap();
    let cert = Certificate::parse(doc.root()).unwrap();
    for extension in &cert.extensions {
        let value = extension.value_der().unwrap();
        let root = value.root();
        match extension.id.as_bytes() {
            extensions::BASIC_CONSTRAINTS => {
                let constraints = extensions::BasicConstraints::parse(root).unwrap();
                assert!(constraints.ca);
                assert_eq!(constraints.path_len.unwrap().to_u64().unwrap(), 2);
            }
            extensions::KEY_USAGE => {
                let usage = extensions::KeyUsage::parse(root).unwrap();
                assert!(usage.permits(0));
                assert!(usage.permits(5));
                assert!(usage.permits(6));
                assert!(!usage.permits(9));
            }
            extensions::SUBJECT_KEY_IDENTIFIER => {
                assert_eq!(crate::types::octets(root).unwrap(), b"key1");
            }
            extensions::AUTHORITY_KEY_IDENTIFIER => assert_eq!(
                extensions::AuthorityKeyIdentifier::parse(root)
                    .unwrap()
                    .key_identifier
                    .unwrap(),
                b"key1"
            ),
            extensions::EXTENDED_KEY_USAGE => {
                assert_eq!(extensions::object_identifiers(root).unwrap().len(), 1);
            }
            extensions::SUBJECT_ALT_NAME => {
                assert_eq!(extensions::general_names(root).unwrap().len(), 2);
            }
            extensions::CERTIFICATE_POLICIES => {
                assert_eq!(extensions::certificate_policies(root).unwrap().len(), 1);
            }
            extensions::POLICY_MAPPINGS => {
                assert_eq!(extensions::policy_mappings(root).unwrap().len(), 1);
            }
            extensions::POLICY_CONSTRAINTS => assert_eq!(
                extensions::PolicyConstraints::parse(root)
                    .unwrap()
                    .require_explicit_policy
                    .unwrap()
                    .to_u64()
                    .unwrap(),
                1
            ),
            extensions::NAME_CONSTRAINTS => assert_eq!(
                extensions::NameConstraints::parse(root)
                    .unwrap()
                    .permitted
                    .len(),
                1
            ),
            extensions::INHIBIT_ANY_POLICY => {
                assert_eq!(Integer::parse(root).unwrap().to_u64().unwrap(), 4);
            }
            [85, 29, 99] => {
                assert!(extension.critical);
                assert_eq!(root.tag(), crate::der::SEQUENCE);
                assert_eq!(root.children().next().unwrap().content(), b"opaque");
            }
            _ => panic!("unexpected generated extension"),
        }
    }
}

#[test]
fn prefix_parsing_preserves_value_zeros_and_suffix() {
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
        Some(crate::tags::BER_INDEFINITE)
    );
}

#[test]
fn ber_set_order_is_reported_and_der_stays_strict() {
    let bytes = [0x31, 6, 2, 1, 2, 2, 1, 1];
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let tree = Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics).unwrap();
    assert_eq!(
        tree.root()
            .set_of_reported(&mut diagnostics)
            .unwrap()
            .count(),
        2
    );
    assert_eq!(
        diagnostics.as_slice()[0].repair(),
        Some(crate::tags::BER_SET_ORDER)
    );
    assert!(
        tree.root()
            .set_of_reported(&mut Diagnostics::new(Recovery::Strict))
            .is_err()
    );
    let tree = Document::parse(&bytes, &Limits::default()).unwrap();
    assert!(tree.root().set_of_reported(&mut diagnostics).is_err());
}
