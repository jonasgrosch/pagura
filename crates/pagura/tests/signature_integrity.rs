#![forbid(unsafe_code)]
//! Independent PDF/CMS signatures authored by the standard-library generator.
use pagura::signatures::{
    Integrity, PermittedModifications, RevocationStatus, SignerTrust, ValidationTime,
};
use pagura::{Document, OpenOptions, Recovery, signatures};
use pagura_testkit::{fixture, fixture_path};
use std::fmt::Write;

#[test]
fn independent_signed_pdfs() {
    let mut count = 0;
    for entry in std::fs::read_dir(fixture_path("cms-signatures", "")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "expect") {
            continue;
        }
        count += 1;
        let name = path.file_stem().unwrap().to_str().unwrap();
        let expected = std::fs::read_to_string(&path).unwrap();
        let doc = Document::open(
            fixture("cms-signatures", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let results = signatures(&doc).unwrap();
        assert_eq!(results.fields.len(), expected.lines().count(), "{name}");
        let actual = results
            .fields
            .iter()
            .fold(String::new(), |mut output, field| {
                assert!(field.error.is_none(), "{name}: {:?}", field.error);
                let signature = field.signature.as_ref().unwrap();
                assert_eq!(signature.results.signer_trust, SignerTrust::NotCheckedYet);
                assert_eq!(
                    signature.results.revocation,
                    RevocationStatus::NotCheckedYet
                );
                // Permitted-change analysis now runs; its cases live in the signature-changes suite.
                assert_ne!(
                    signature.results.permitted_modifications,
                    PermittedModifications::NotAnalysedYet,
                    "{name}"
                );
                assert_eq!(
                    signature.results.validation_time,
                    ValidationTime::NotCheckedYet
                );
                writeln!(
                    output,
                    "{:?}|{:?}",
                    signature.results.integrity, signature.results.algorithms
                )
                .unwrap();
                output
            });
        assert_eq!(actual, expected, "{name}");
        assert!(results.fields[0].error.is_none(), "{name}");
        let signature = results.fields[0].signature.as_ref().unwrap();
        if matches!(
            name,
            "malformed-extra-certificate" | "only-malformed-certificate" | "bad-ski-certificate"
        ) {
            assert_eq!(signature.results.certificate_errors.len(), 1);
        }
        if name == "certification-shared-container" {
            let certification = results.certification.as_ref().unwrap();
            assert_eq!(certification.object, signature.object);
            assert!(!std::ptr::eq(
                certification.dictionary,
                signature.dictionary
            ));
            assert_eq!(certification.results.integrity, Integrity::Malformed);
        } else if name.starts_with("certification") {
            let certification = results.certification.as_ref().unwrap();
            assert_eq!(certification.object, signature.object);
            assert_eq!(certification.results, signature.results);
        }
        if name == "ber-unsorted" {
            assert!(
                signature
                    .results
                    .diagnostics
                    .iter()
                    .any(|d| d.tag() == "asn1.ber-set-order")
            );
        }
    }
    assert!(count >= 35);
}

#[test]
fn strict_recovery_rejects_ber_set_repair_per_signature() {
    let doc = Document::open(
        fixture("cms-signatures", "ber-unsorted.pdf"),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let results = signatures(&doc).unwrap();
    let signature = results.fields[0].signature.as_ref().unwrap();
    assert_eq!(signature.results.integrity, Integrity::Malformed);
    assert_eq!(
        signature
            .results
            .error
            .as_ref()
            .unwrap()
            .repair
            .unwrap()
            .as_str(),
        "asn1.ber-set-order"
    );
}

#[test]
fn malformed_signature_does_not_hide_another_field() {
    let mut bytes = fixture("signatures", "multiple-revisions.pdf");
    // Equal-width mutation preserves every physical offset and revision.
    let start = bytes.windows(7).position(|b| b == b"/Filter").unwrap();
    bytes[start..start + 7].copy_from_slice(b"/Falter");
    let doc = Document::open(bytes, &OpenOptions::default()).unwrap();
    let results = signatures(&doc).unwrap();
    assert_eq!(results.fields.len(), 2);
    assert!(results.fields[0].error.is_some());
    assert!(results.fields[0].signature.is_none());
    assert!(results.fields[1].error.is_none());
    assert_eq!(
        results.fields[1]
            .signature
            .as_ref()
            .unwrap()
            .results
            .integrity,
        Integrity::Malformed
    );
}

#[test]
fn verification_limits_are_per_signature_results() {
    let doc = Document::open(
        fixture("cms-signatures", "rsa-sha256.pdf"),
        &OpenOptions {
            limits: pagura::Limits {
                signatures: pagura::SignatureLimits {
                    rsa_work: 1,
                    ..pagura::SignatureLimits::default()
                },
                ..pagura::Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let results = signatures(&doc).unwrap();
    let signature = results.fields[0].signature.as_ref().unwrap();
    assert_eq!(signature.results.integrity, Integrity::ResourceLimit);
    assert_eq!(
        signature.results.error.as_ref().unwrap().kind,
        pagura::ErrorKind::Limit
    );
}
