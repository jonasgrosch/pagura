#![forbid(unsafe_code)]
//! The independent fixtures of `tools/fixturegen/pki/`, and the cached
//! PKITS and Wycheproof encoding cases that `tools/pki_vectors.py`
//! exports into the signature reference cache.

use pagura_core::{Diagnostics, Limits, Recovery, Result};
use pagura_pki::cms::{self, SignedData};
use pagura_pki::der::{Document, Element};
use pagura_pki::timestamp::{self, TstInfo};
use pagura_pki::types::EcdsaSignature;
use pagura_pki::x509::extensions::{self, AuthorityKeyIdentifier, BasicConstraints, KeyUsage};
use pagura_pki::x509::{Certificate, CertificateList};
use pagura_pki::{ObjectIdentifier, oids};
use pagura_testkit::{Manifest, fixture};
use std::path::PathBuf;

/// Parses `bytes` as `structure`; extension values name their `view`.
/// `None` is an unknown structure or view.
fn parse(structure: &str, view: Option<&str>, bytes: &[u8]) -> Option<Result<()>> {
    let limits = Limits::default();
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let document = if structure == "cms-ber" {
        Document::parse_ber(bytes, &limits, &mut diagnostics)
    } else {
        Document::parse(bytes, &limits)
    };
    let document = match document {
        Ok(document) => document,
        Err(error) => return Some(Err(error)),
    };
    let root = document.root();
    Some(match structure {
        "certificate" => Certificate::parse(root).map(|_| ()),
        "crl" => CertificateList::parse(root).map(|_| ()),
        "cms" => SignedData::from_content_info(root).map(|_| ()),
        "cms-ber" => {
            SignedData::from_content_info_with_diagnostics(root, &mut diagnostics).map(|_| ())
        }
        "timestamp" => TstInfo::parse(root).map(|_| ()),
        "ecdsa" => EcdsaSignature::parse(root).map(|_| ()),
        "signed-set" => root.set_of().map(|_| ()),
        "oids" => named_oids(root),
        "extension" => return extension(view?, root),
        _ => return None,
    })
}

fn extension(view: &str, root: Element<'_>) -> Option<Result<()>> {
    Some(match view {
        "basic-constraints" => BasicConstraints::parse(root).map(|_| ()),
        "key-usage" => KeyUsage::parse(root).map(|_| ()),
        "authority-key-identifier" => AuthorityKeyIdentifier::parse(root).map(|_| ()),
        "general-names" => extensions::general_names(root).map(|_| ()),
        "certificate-policies" => extensions::certificate_policies(root).map(|_| ()),
        "policy-constraints" => extensions::PolicyConstraints::parse(root).map(|_| ()),
        "name-constraints" => extensions::NameConstraints::parse(root).map(|_| ()),
        "policy-mappings" => extensions::policy_mappings(root).map(|_| ()),
        "access-descriptions" => extensions::access_descriptions(root).map(|_| ()),
        "object-identifiers" => extensions::object_identifiers(root).map(|_| ()),
        _ => return None,
    })
}

/// The crate's OID constants equal the generator's encoding of their dotted
/// names, in the generator's `NAMED_OIDS` order.
fn named_oids(root: Element<'_>) -> Result<()> {
    let named = [
        extensions::AUTHORITY_KEY_IDENTIFIER,
        extensions::SUBJECT_KEY_IDENTIFIER,
        extensions::BASIC_CONSTRAINTS,
        extensions::KEY_USAGE,
        extensions::EXTENDED_KEY_USAGE,
        extensions::CERTIFICATE_POLICIES,
        extensions::POLICY_MAPPINGS,
        extensions::NAME_CONSTRAINTS,
        extensions::POLICY_CONSTRAINTS,
        extensions::INHIBIT_ANY_POLICY,
        extensions::SUBJECT_ALT_NAME,
        extensions::CRL_DISTRIBUTION_POINTS,
        extensions::AUTHORITY_INFO_ACCESS,
        cms::ID_DATA,
        cms::ID_SIGNED_DATA,
        cms::CONTENT_TYPE,
        cms::MESSAGE_DIGEST,
        timestamp::ID_TST_INFO,
        oids::SHA1,
        oids::SHA224,
        oids::SHA256,
        oids::SHA384,
        oids::SHA512,
        oids::RSA_ENCRYPTION,
        oids::RSASSA_PSS,
        oids::MGF1,
        oids::EC_PUBLIC_KEY,
        oids::SIGNING_CERTIFICATE,
        oids::SIGNING_CERTIFICATE_V2,
        oids::CMS_ALGORITHM_PROTECTION,
        oids::SECP256R1,
        oids::SECP384R1,
        oids::SECP521R1,
        oids::SHA1_WITH_RSA,
        oids::ECDSA_WITH_SHA1,
        oids::SHA224_WITH_RSA,
        oids::ECDSA_WITH_SHA224,
        oids::SHA256_WITH_RSA,
        oids::ECDSA_WITH_SHA256,
        oids::SHA384_WITH_RSA,
        oids::ECDSA_WITH_SHA384,
        oids::SHA512_WITH_RSA,
        oids::ECDSA_WITH_SHA512,
    ];
    let encoded = extensions::object_identifiers(root)?;
    assert_eq!(encoded, named);
    Ok(())
}

#[test]
fn independent_fixtures_match_their_expected_results() {
    let manifest = Manifest::load("pki");
    assert_ne!(manifest.cases().len(), 0);
    for case in manifest.cases() {
        let structure = case.string("structure").expect("structure");
        if structure == "records" {
            continue; // Path-validation records; the validation unit tests read them.
        }
        let bytes = fixture("pki", &format!("{}.der", case.name()));
        let result = parse(structure, case.string("view"), &bytes)
            .unwrap_or_else(|| panic!("{}: unknown structure or view", case.id()));
        match case.string("expect") {
            Some("accept") => assert!(result.is_ok(), "{}: {result:?}", case.id()),
            Some("reject") => {
                let error = result.expect_err(case.id());
                if let Some(kind) = case.string("error") {
                    assert_eq!(error.kind.name(), kind, "{}: {error}", case.id());
                }
            }
            other => panic!("{}: unknown expectation {other:?}", case.id()),
        }
    }
}

/// The signature reference cache, located as `pki_vectors.py` does.
fn references() -> PathBuf {
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
    let export = references().join("export/pki");
    let Ok(text) = std::fs::read_to_string(export.join("manifest.json")) else {
        eprintln!("Cached PKI suites are not exported; run tools/pki_vectors.py");
        return;
    };
    let manifest = Manifest::parse(&text).expect("export manifest");
    let mut counts = [0usize; 3];
    for case in manifest.cases() {
        let path = export.join(case.string("path").expect("path"));
        let bytes = std::fs::read(&path).expect("exported case");
        let structure = case.string("structure").expect("structure");
        counts[match structure {
            "certificate" => 0,
            "crl" => 1,
            _ => 2,
        }] += 1;
        let result = parse(structure, None, &bytes)
            .unwrap_or_else(|| panic!("{}: unknown structure", case.id()));
        let accepted = case.string("expect") == Some("accept");
        assert_eq!(
            result.is_ok(),
            accepted,
            "{} ({}): {result:?}",
            case.id(),
            case.string("source").unwrap_or_default()
        );
    }
    let [certificates, crls, signatures] = counts;
    eprintln!(
        "Cached encoding cases: {certificates} PKITS certificates, {crls} PKITS CRLs, {signatures} Wycheproof ECDSA signatures"
    );
}

#[test]
fn oid_constants_compare_with_parsed_identifiers() {
    let bytes = [6, 3, 85, 29, 15];
    let document = Document::parse(&bytes, &Limits::default()).unwrap();
    let id = ObjectIdentifier::parse(document.root()).unwrap();
    assert_eq!(id, extensions::KEY_USAGE);
    assert_ne!(id, extensions::BASIC_CONSTRAINTS);
}
