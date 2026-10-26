#![forbid(unsafe_code)]
//! Declared size/checksum checks use independent fixtures, not product-generated hashes.

use pagura_core::{ErrorKind, Limits, ObjectId, Result};
use pagura_document::{Document, OpenOptions, files::ParameterCheck};
use pagura_testkit::fixture;

fn open(name: &str, options: &OpenOptions) -> Result<Document> {
    Document::open(fixture("files", &format!("{name}.pdf")), options)
}

#[test]
fn uncompressed_size_and_checksum_declarations_have_distinct_outcomes() {
    use ParameterCheck::{Matches, Mismatch, NotDeclared};
    for (name, expected) in [
        ("populated", [Matches, Matches]),
        ("empty-payload", [Matches, Matches]),
        ("check-flate", [Matches, Matches]),
        ("check-size-mismatch", [Mismatch, Matches]),
        ("check-checksum-mismatch", [Matches, Mismatch]),
        ("check-both-mismatch", [Mismatch, Mismatch]),
        ("check-size-only", [Matches, NotDeclared]),
        ("check-checksum-only", [NotDeclared, Matches]),
        ("check-empty-params", [NotDeclared, NotDeclared]),
        ("stream-type-absent", [NotDeclared, NotDeclared]),
        ("large-size-declaration", [Mismatch, NotDeclared]),
    ] {
        let document = open(name, &OpenOptions::default()).unwrap();
        let files = document.embedded_files().unwrap();
        for entry in &files[0].specification.embedded {
            let checked = entry.file.check_parameters().unwrap();
            assert_eq!([checked.size, checked.checksum], expected, "{name}");
            assert_eq!(
                checked.decoded_size,
                if name == "empty-payload" { 0 } else { 18 }
            );
            assert_eq!(
                entry.file.check_parameters().unwrap(),
                checked,
                "repeatable read"
            );
        }
        assert_eq!(
            document.materialize().unwrap().bytes(),
            fixture("files", &format!("{name}.pdf"))
        );
    }
    let document = open("related", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    let checked = files[0].specification.related[0]
        .file
        .check_parameters()
        .unwrap();
    assert_eq!(checked.decoded_size, 13);
    assert_eq!([checked.size, checked.checksum], [NotDeclared, NotDeclared]);
}

#[test]
fn parameter_checks_require_complete_authorized_bounded_payloads() {
    for name in ["encrypted-no-type", "encrypted-explicit-no-type"] {
        for password in [b"".as_slice(), b"wrong", b"attachment", b"owner"] {
            let document = open(
                name,
                &OpenOptions {
                    password: password.to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let files = document.embedded_files().unwrap();
            let result = files[0].specification.embedded[0].file.check_parameters();
            if password == b"attachment" || password == b"owner" {
                let checked = result.unwrap();
                assert_eq!(checked.decoded_size, 19);
                assert_eq!(checked.checksum, ParameterCheck::NotDeclared);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.as_error().unwrap().kind, ErrorKind::Password);
                assert_eq!(error.as_error().unwrap().object, Some(ObjectId::new(6, 0)));
            }
        }
    }
    let document = open(
        "check-flate",
        &OpenOptions {
            limits: Limits {
                decoded_bytes: 4,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files[0].specification.embedded[0]
            .file
            .check_parameters()
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
    let document = open("external-stream", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files[0].specification.embedded[0]
            .file
            .check_parameters()
            .unwrap_err()
            .as_error()
            .unwrap()
            .feature(),
        Some(pagura_document::features::EXTERNAL_STREAM)
    );
    let document = open("encrypted-identity-no-type", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files[0].specification.embedded[0]
            .file
            .check_parameters()
            .unwrap()
            .decoded_size,
        23
    );
}
