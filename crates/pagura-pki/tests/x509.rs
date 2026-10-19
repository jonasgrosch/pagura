#![forbid(unsafe_code)]
//! Certificates, CRLs and extension views (RFC 5280 4-5) on the generated
//! `pki` fixtures.

use pagura_core::{Diagnostics, ErrorKind, Limits, Recovery};
use pagura_pki::der::{self, Document};
use pagura_pki::x509::extensions::{
    self, AuthorityKeyIdentifier, BasicConstraints, GeneralName, KeyUsage, NameConstraints,
    PolicyConstraints,
};
use pagura_pki::x509::{Certificate, CertificateList, CrlVersion, Version};
use pagura_pki::{Integer, repairs};
use pagura_testkit::fixture;

fn der(name: &str) -> Vec<u8> {
    fixture("pki", &format!("{name}.der"))
}

#[test]
fn signed_ranges_are_exact() {
    let bytes = der("x509-v3");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let certificate = Certificate::parse(document.root()).unwrap();
    assert_eq!(&bytes[certificate.tbs_range()], certificate.tbs.encoded());
    assert_eq!(
        certificate.tbs.range().start,
        document.root().content_offset()
    );
    assert_eq!(certificate.version, Version::V3);
    assert_eq!(certificate.serial.to_u64(), Some(7));
    assert_eq!(certificate.not_after.year(), 2050);
    assert_eq!(certificate.signature.as_bytes(), b"unverified");
    assert_eq!(
        certificate
            .public_key
            .algorithm
            .parameters
            .map(der::Element::tag),
        Some(der::NULL)
    );
}

#[test]
fn versions_and_unique_identifiers_follow_the_schema() {
    let bytes = der("x509-v1");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let v1 = Certificate::parse(document.root()).unwrap();
    assert_eq!(v1.version, Version::V1);
    assert_eq!(v1.extensions.len(), 0);
    let bytes = der("x509-v2-unique-ids");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let v2 = Certificate::parse(document.root()).unwrap();
    assert_eq!(v2.version, Version::V2);
    assert_eq!(v2.issuer_unique_id.unwrap().as_bytes(), [1]);
    assert_eq!(v2.subject_unique_id.unwrap().as_bytes(), [2]);
    let bytes = der("x509-oversized-version");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let error = Certificate::parse(document.root()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
}

#[test]
fn ber_origin_certificates_are_rechecked_as_der() {
    let bytes = der("x509-v1");
    let header = if bytes[1] < 0x80 {
        2
    } else {
        2 + usize::from(bytes[1] & 0x7f)
    };
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    // A DER certificate inside an indefinite BER container is accepted as is.
    let wrapped = [&[0x31, 0x80][..], &bytes, &[0, 0]].concat();
    let document = Document::parse_ber(&wrapped, &Limits::default(), &mut diagnostics).unwrap();
    let certificate = Certificate::parse(document.root().children().next().unwrap()).unwrap();
    assert_eq!(certificate.element.range(), 2..2 + bytes.len());
    // The certificate itself in indefinite form is not DER.
    let ber = [&[0x30, 0x80][..], &bytes[header..], &[0, 0]].concat();
    let document = Document::parse_ber(&ber, &Limits::default(), &mut diagnostics).unwrap();
    let error = Certificate::parse(document.root()).unwrap_err();
    assert_eq!(error.repair, Some(repairs::BER_INDEFINITE));
}

#[test]
fn common_extensions_are_typed_and_unknown_values_remain_accessible() {
    let bytes = der("x509-v3");
    let doc = Document::parse(&bytes, &Limits::default()).unwrap();
    let cert = Certificate::parse(doc.root()).unwrap();
    assert!(cert.extension(extensions::KEY_USAGE).unwrap().is_critical());
    assert!(
        cert.extension(extensions::CRL_DISTRIBUTION_POINTS)
            .is_none()
    );
    for extension in cert.extensions {
        let value = extension.value_der().unwrap();
        let root = value.root();
        assert_eq!(
            root.range().start,
            extension.element().range().end - extension.value().len()
        );
        match extension.id() {
            extensions::BASIC_CONSTRAINTS => {
                let constraints = BasicConstraints::parse(root).unwrap();
                assert!(constraints.ca);
                assert_eq!(constraints.path_len.unwrap().to_u64(), Some(2));
            }
            extensions::KEY_USAGE => {
                let usage = KeyUsage::parse(root).unwrap();
                assert!(usage.contains(KeyUsage::DIGITAL_SIGNATURE));
                assert!(usage.contains(KeyUsage::KEY_CERT_SIGN | KeyUsage::CRL_SIGN));
                assert!(!usage.contains(KeyUsage::DECIPHER_ONLY));
                assert_eq!(usage.bits(), 0b110_0001);
            }
            extensions::SUBJECT_KEY_IDENTIFIER => {
                assert_eq!(pagura_pki::types::octets(root).unwrap(), b"key1");
            }
            extensions::AUTHORITY_KEY_IDENTIFIER => {
                let aki = AuthorityKeyIdentifier::parse(root).unwrap();
                assert_eq!(aki.key_identifier.unwrap(), b"key1");
                assert!(aki.certificate.is_none());
            }
            extensions::EXTENDED_KEY_USAGE => {
                assert_eq!(extensions::object_identifiers(root).unwrap().len(), 1);
            }
            extensions::SUBJECT_ALT_NAME => {
                let names = extensions::general_names(root).unwrap();
                assert!(matches!(names[0], GeneralName::Dns(b"example.test")));
                assert!(matches!(
                    names[1],
                    GeneralName::Uri(b"https://example.test")
                ));
            }
            extensions::CERTIFICATE_POLICIES => {
                let policies = extensions::certificate_policies(root).unwrap();
                assert_eq!(policies.len(), 1);
                assert!(policies[0].qualifiers.is_none());
            }
            extensions::POLICY_MAPPINGS => {
                let mappings = extensions::policy_mappings(root).unwrap();
                assert_eq!(mappings[0].issuer_domain.as_bytes(), [42, 3, 4]);
                assert_eq!(mappings[0].subject_domain.as_bytes(), [42, 3, 5]);
            }
            extensions::POLICY_CONSTRAINTS => {
                let constraints = PolicyConstraints::parse(root).unwrap();
                assert_eq!(
                    constraints.require_explicit_policy.unwrap().to_u64(),
                    Some(1)
                );
                assert_eq!(
                    constraints.inhibit_policy_mapping.unwrap().to_u64(),
                    Some(2)
                );
            }
            extensions::NAME_CONSTRAINTS => {
                let constraints = NameConstraints::parse(root).unwrap();
                assert_eq!(constraints.permitted.len(), 1);
                assert_eq!(constraints.excluded.len(), 0);
            }
            extensions::INHIBIT_ANY_POLICY => {
                assert_eq!(Integer::parse(root).unwrap().to_u64(), Some(4));
            }
            id if id.as_bytes() == [85, 29, 99] => {
                assert!(extension.is_critical());
                assert_eq!(root.tag(), der::SEQUENCE);
                assert_eq!(root.children().next().unwrap().content(), b"opaque");
            }
            _ => panic!("unexpected generated extension"),
        }
    }
}

#[test]
fn extension_views_keep_their_schema_invariants() {
    let bytes = der("aki-issuer-serial");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let aki = AuthorityKeyIdentifier::parse(document.root()).unwrap();
    let certificate = aki.certificate.unwrap();
    assert!(matches!(
        certificate.issuer.as_slice(),
        [GeneralName::Directory(_)]
    ));
    assert_eq!(certificate.serial.to_u64(), Some(7));

    let bytes = der("key-usage-decipher-only");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    assert_eq!(
        KeyUsage::parse(document.root()).unwrap(),
        KeyUsage::DECIPHER_ONLY
    );

    let bytes = der("general-names-every-choice");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let names = extensions::general_names(document.root()).unwrap();
    assert!(matches!(names[0], GeneralName::Other(_)));
    assert!(matches!(names[1], GeneralName::Mailbox(b"a@example.test")));
    assert!(matches!(names[3], GeneralName::Directory(_)));
    assert!(matches!(names[5], GeneralName::Ip([192, 0, 2, 1])));
    match names[6] {
        GeneralName::RegisteredId(id) => assert_eq!(id.as_bytes(), [42, 3]),
        other => panic!("{other:?}"),
    }

    let bytes = der("name-constraints-distances");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let constraints = NameConstraints::parse(document.root()).unwrap();
    let subtree = constraints.excluded[0];
    assert_eq!(subtree.minimum.unwrap().to_u64(), Some(1));
    assert_eq!(subtree.maximum.unwrap().to_u64(), Some(2));

    let bytes = der("access-descriptions-ca-issuers");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let access = extensions::access_descriptions(document.root()).unwrap();
    assert!(matches!(
        access[0].location,
        GeneralName::Uri(b"https://example.test/ca.cer")
    ));

    let bytes = der("basic-constraints-end-entity");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let constraints = BasicConstraints::parse(document.root()).unwrap();
    assert!(!constraints.ca);
    assert!(constraints.path_len.is_none());
}

#[test]
fn crl_versions_entries_and_extensions() {
    let bytes = der("x509-crl-v2");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let crl = CertificateList::parse(document.root()).unwrap();
    assert_eq!(crl.version, CrlVersion::V2);
    assert_eq!(&bytes[crl.tbs.range()], crl.tbs.encoded());
    assert_eq!(crl.next_update.unwrap().year(), 2050);
    assert_eq!(crl.revoked.len(), 1);
    assert_eq!(crl.revoked[0].serial.to_u64(), Some(7));
    assert_eq!(crl.revoked[0].extensions.len(), 1);
    assert_eq!(crl.extensions.len(), 1);
    let bytes = der("x509-crl-v1");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let crl = CertificateList::parse(document.root()).unwrap();
    assert_eq!(crl.version, CrlVersion::V1);
    assert_eq!(crl.revoked[0].extensions.len(), 0);
    assert_eq!(crl.extensions.len(), 0);
}
