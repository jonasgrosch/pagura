#![forbid(unsafe_code)]
//! CMS SignedData (RFC 5652 3, 5 and 10-11) on the generated `pki` fixtures.

use pagura_core::{Diagnostic, Diagnostics, ErrorKind, Limits, Recovery, SignatureLimits};
use pagura_pki::cms::{
    self, CertificateChoice, CmsVersion, RevocationInfo, SignedAttributes, SignedData,
    SignerIdentifier,
};
use pagura_pki::der::Document;
use pagura_pki::repairs;
use pagura_testkit::fixture;

fn der(name: &str) -> Vec<u8> {
    fixture("pki", &format!("{name}.der"))
}

#[test]
fn signed_attributes_keep_their_exact_signature_input() {
    let bytes = der("cms-detached");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let signed = SignedData::from_content_info(document.root()).unwrap();
    assert_eq!(signed.version, CmsVersion::V1);
    assert_eq!(signed.content_type, cms::ID_DATA);
    assert!(signed.content.is_none());
    assert_eq!(signed.digest_algorithms.len(), 1);
    assert!(matches!(
        signed.certificates.as_slice(),
        [CertificateChoice::X509(_)]
    ));
    assert!(matches!(
        signed.revocation.as_slice(),
        [RevocationInfo::Crl(_)]
    ));
    let [signer] = signed.signers.as_slice() else {
        panic!("one signer expected");
    };
    assert_eq!(signer.version(), CmsVersion::V1);
    assert!(matches!(
        signer.sid,
        SignerIdentifier::IssuerAndSerial { .. }
    ));
    assert_eq!(signer.signature.as_primitive(), Some(&b"signature"[..]));
    let attrs = signer.signed_attributes.as_ref().unwrap();
    assert_eq!(attrs.signature_input(), der("cms-signature-input"));
    assert_eq!(attrs.element.encoded()[0], 0xa0);
    assert_eq!(&attrs.signature_input()[1..], &attrs.element.encoded()[1..]);
    assert_eq!(attrs.attributes.len(), 2);
    assert_eq!(
        attrs.content_digest(signed.content_type).unwrap(),
        (0..32).collect::<Vec<u8>>()
    );
    let content_type = attrs.unique(cms::CONTENT_TYPE).unwrap().unwrap();
    assert_eq!(content_type.id, cms::CONTENT_TYPE);
}

#[test]
fn signed_attribute_copies_are_budgeted() {
    // [0] { SEQUENCE { OID 1.2.3.4, SET { OCTET STRING of 2000 bytes } } }
    let value = [&[0x04, 0x82, 0x07, 0xd0][..], &[b'a'; 2000]].concat();
    let set = [&[0x31, 0x82, 0x07, 0xd4][..], &value].concat();
    let attribute = [&[0x30, 0x82, 0x07, 0xdd, 0x06, 0x03, 42, 3, 4][..], &set].concat();
    let attributes = [&[0xa0, 0x82, 0x07, 0xe1][..], &attribute].concat();
    // The small index fits the budget; the 2,021-byte copy does not.
    let limits = Limits {
        signatures: SignatureLimits {
            asn1_bytes: 1500,
            ..Limits::default().signatures
        },
        ..Limits::default()
    };
    let document = Document::parse(&attributes, &limits).unwrap();
    let error = SignedAttributes::parse(document.root()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    let document = Document::parse(&attributes, &Limits::default()).unwrap();
    let parsed = SignedAttributes::parse(document.root()).unwrap();
    assert_eq!(parsed.signature_input().len(), attributes.len());
    assert_eq!(parsed.signature_input()[0], 0x31);
}

#[test]
fn subject_key_identifier_signers_require_version_3() {
    let bytes = der("cms-key-identifier");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let signed = SignedData::from_content_info(document.root()).unwrap();
    assert_eq!(signed.version, CmsVersion::V3);
    let signer = &signed.signers[0];
    assert_eq!(signer.version(), CmsVersion::V3);
    assert!(matches!(
        signer.sid,
        SignerIdentifier::SubjectKeyIdentifier(b"key1")
    ));
}

#[test]
fn certificate_and_revocation_choices_set_the_version() {
    let bytes = der("cms-attribute-certificate");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let signed = SignedData::from_content_info(document.root()).unwrap();
    assert_eq!(signed.version, CmsVersion::V3);
    assert!(matches!(
        signed.certificates.as_slice(),
        [CertificateChoice::Other(_)]
    ));
    let bytes = der("cms-other-revocation");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let signed = SignedData::from_content_info(document.root()).unwrap();
    assert_eq!(signed.version, CmsVersion::V5);
    let [RevocationInfo::Other { format, .. }] = signed.revocation.as_slice() else {
        panic!("one other revocation format expected");
    };
    assert_eq!(format.as_bytes(), [43, 6, 1, 5, 5, 7, 16, 2]);
}

#[test]
fn ber_content_is_concatenated_with_reported_repairs() {
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let bytes = der("cms-ber-content");
    let document = Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics).unwrap();
    let signed = SignedData::from_content_info(document.root()).unwrap();
    assert_eq!(signed.content.unwrap().to_bytes().unwrap(), b"abcdef");
    assert_ne!(diagnostics.as_slice().len(), 0);
    let mut strict = Diagnostics::new(Recovery::Strict);
    assert!(Document::parse_ber(&bytes, &Limits::default(), &mut strict).is_err());
}

#[test]
fn malformed_certificates_are_kept_apart() {
    let bytes = der("cms-malformed-certificate");
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let signed = SignedData::from_content_info(document.root()).unwrap();
    assert_eq!(signed.certificates.len(), 2);
    let malformed = signed
        .certificates
        .iter()
        .filter_map(|choice| match choice {
            CertificateChoice::Malformed { element, error } => Some((*element, error)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(element, error)] = malformed.as_slice() else {
        panic!("one malformed certificate expected");
    };
    // An out-of-range version is malformed input, not a budget failure.
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(&bytes[element.range()], element.encoded());
}

#[test]
fn ber_set_order_is_a_reported_repair() {
    let bytes = der("cms-ber-unordered-digests");
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let document = Document::parse_ber(&bytes, &Limits::default(), &mut diagnostics).unwrap();
    let signed =
        SignedData::from_content_info_with_diagnostics(document.root(), &mut diagnostics).unwrap();
    assert_eq!(signed.digest_algorithms.len(), 2);
    assert!(
        diagnostics
            .as_slice()
            .iter()
            .filter_map(Diagnostic::repair)
            .any(|repair| repair == repairs::BER_SET_ORDER)
    );
    let mut strict = Diagnostics::new(Recovery::Strict);
    let error =
        SignedData::from_content_info_with_diagnostics(document.root(), &mut strict).unwrap_err();
    assert_eq!(error.repair, Some(repairs::BER_SET_ORDER));
    assert!(SignedData::from_content_info(document.root()).is_err());
}
