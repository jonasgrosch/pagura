#![forbid(unsafe_code)]
//! Independent FileAttachment fixtures: source identity, geometry and lazy payloads.

use pagura_core::{ErrorKind, Limits, ObjectId, Recovery, Rect, Result};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;

fn open(name: &str, options: &OpenOptions) -> Result<Document> {
    Document::open(fixture("file_annotations", &format!("{name}.pdf")), options)
}

#[test]
fn annotations_preserve_source_order_identity_description_and_file_handles() {
    let document = open("populated", &OpenOptions::default()).unwrap();
    let annotations = document
        .page(0)
        .unwrap()
        .attachment_annotations(&document)
        .unwrap();
    assert_eq!(annotations.len(), 1);
    let annotation = &annotations[0];
    assert_eq!(annotation.index, 0);
    assert_eq!(annotation.object, ObjectId::new(8, 0));
    assert_eq!(
        annotation.rectangle,
        Rect::from_corners(1.0, 2.0, 9.0, 12.0).unwrap()
    );
    assert_eq!(annotation.contents.as_deref(), Some("Annotation Ω"));
    assert_eq!(annotation.icon_name(), b"Paperclip");
    assert!(annotation.dictionary.contains_key(b"Custom"));
    assert_eq!(annotation.specification.object, Some(ObjectId::new(5, 0)));
    assert_eq!(
        annotation.specification.description.as_deref(),
        Some("Attachment €")
    );
    assert_eq!(
        annotation.specification.names.unicode.as_deref(),
        Some("Grüße Ω.txt")
    );
    for payload in &annotation.specification.embedded {
        assert_eq!(payload.file.object(), ObjectId::new(6, 0));
        assert_eq!(
            payload.file.data().unwrap().as_ref(),
            b"attachment\0bytes\xff\n"
        );
    }
    assert_eq!(
        document.original_bytes(),
        fixture("file_annotations", "populated.pdf")
    );
    let document = open("mixed", &OpenOptions::default()).unwrap();
    let annotations = document
        .page(0)
        .unwrap()
        .attachment_annotations(&document)
        .unwrap();
    assert_eq!(
        annotations
            .iter()
            .map(|annotation| annotation.index)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    for annotation in annotations {
        assert_eq!(annotation.object, ObjectId::new(9, 0));
        assert_eq!(annotation.contents.as_deref(), Some(""));
        assert_eq!(annotation.icon_name(), b"CustomIcon");
    }
    let document = open("indirect-array-and-geometry", &OpenOptions::default()).unwrap();
    let annotations = document
        .page(0)
        .unwrap()
        .attachment_annotations(&document)
        .unwrap();
    assert_eq!(annotations[0].object, ObjectId::new(8, 0));
    assert_eq!(
        annotations[0].rectangle,
        Rect::from_corners(1.0, 2.0, 9.0, 12.0).unwrap()
    );
    assert_eq!(annotations[0].contents.as_deref(), Some("Referenced"));
    assert_eq!(annotations[0].icon_name(), b"Graph");
}

#[test]
fn discovery_routes_absence_defaults_and_external_files_stay_distinct() {
    for name in ["absent", "missing", "empty"] {
        let document = open(name, &OpenOptions::default()).unwrap();
        assert!(
            document
                .page(0)
                .unwrap()
                .attachment_annotations(&document)
                .unwrap()
                .is_empty(),
            "{name}"
        );
        assert_eq!(
            document.embedded_files().unwrap().len(),
            1,
            "catalog is separate"
        );
    }
    for name in ["page-only", "no-type", "null-optional"] {
        let document = open(name, &OpenOptions::default()).unwrap();
        let annotations = document
            .page(0)
            .unwrap()
            .attachment_annotations(&document)
            .unwrap();
        assert_eq!(annotations.len(), 1);
        assert_eq!(
            annotations[0].contents, None,
            "no inferred FS/Desc fallback"
        );
        assert_eq!(annotations[0].icon, None);
        assert_eq!(annotations[0].icon_name(), b"PushPin");
        if name == "page-only" {
            assert!(
                document.embedded_files().unwrap().is_empty(),
                "page attachment needs no catalog entry"
            );
        }
    }
    for (name, expected) in [
        ("external", b"../outside.bin".as_slice()),
        ("direct-external-spec", b"https://invalid.example/file"),
    ] {
        let document = open(name, &OpenOptions::default()).unwrap();
        let annotations = document
            .page(0)
            .unwrap()
            .attachment_annotations(&document)
            .unwrap();
        let file = &annotations[0].specification;
        assert_eq!(file.object, None);
        assert_eq!(file.names.file, Some(expected));
        assert!(file.embedded.is_empty(), "external source remains inert");
    }
    let document = open("populated", &OpenOptions::default()).unwrap();
    let other = open("populated", &OpenOptions::default()).unwrap();
    assert_eq!(
        document
            .page(0)
            .unwrap()
            .attachment_annotations(&other)
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn malformed_annotations_and_reference_cycles_never_return_partial_views() {
    for name in [
        "wrong-array",
        "direct-annotation",
        "null-member",
        "missing-reference",
        "stream-annotation",
        "missing-subtype",
        "wrong-subtype",
        "wrong-type",
        "missing-file",
        "null-file",
        "wrong-file",
        "missing-rectangle",
        "wrong-rectangle",
        "short-rectangle",
        "wrong-coordinate",
        "wrong-contents",
        "wrong-icon",
        "late-failure",
    ] {
        let document = open(name, &OpenOptions::default()).unwrap();
        let error = document
            .page(0)
            .unwrap()
            .attachment_annotations(&document)
            .unwrap_err();
        assert_eq!(error.as_error().unwrap().kind, ErrorKind::Syntax, "{name}");
        assert!(
            error.as_error().unwrap().object.is_some(),
            "{name}: error retains source identity"
        );
        if name == "late-failure" {
            assert_eq!(error.as_error().unwrap().object, Some(ObjectId::new(9, 0)));
        }
    }
    let document = open("reference-cycle", &OpenOptions::default()).unwrap();
    assert_eq!(
        document
            .page(0)
            .unwrap()
            .attachment_annotations(&document)
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Cycle
    );
    let strict = open(
        "invalid-unicode",
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        strict
            .page(0)
            .unwrap()
            .attachment_annotations(&strict)
            .unwrap_err()
            .as_error()
            .unwrap()
            .repair,
        Some(pagura_core::repairs::INVALID_TEXT)
    );
    let repaired = open("invalid-unicode", &OpenOptions::default()).unwrap();
    let annotations = repaired
        .page(0)
        .unwrap()
        .attachment_annotations(&repaired)
        .unwrap();
    assert_eq!(annotations[0].contents.as_deref(), Some("A�"));
    assert!(
        repaired
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.repair() == Some(pagura_core::repairs::INVALID_TEXT))
    );
}

#[test]
fn discovery_and_decoding_have_independent_budgets_and_authorization() {
    for (limits, message) in [
        (
            Limits {
                traversal_work: 32,
                ..Limits::default()
            },
            "navigation traversal budget exceeded",
        ),
        (
            Limits {
                document_bytes: 512 * 1024,
                ..Limits::default()
            },
            "navigation result budget exceeded",
        ),
    ] {
        let document = open(
            "many",
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let page = document.page(0).unwrap();
        let error = page.attachment_annotations(&document).unwrap_err();
        assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit);
        assert_eq!(
            error.as_error().unwrap().message,
            message,
            "the requested view budget, not page-cache preparation, must refuse this fixture"
        );
    }
    for name in ["flate", "absent-payload-type"] {
        let document = open(name, &OpenOptions::default()).unwrap();
        let annotations = document
            .page(0)
            .unwrap()
            .attachment_annotations(&document)
            .unwrap();
        assert_eq!(
            annotations[0].specification.embedded[0]
                .file
                .data()
                .unwrap()
                .as_ref(),
            b"attachment\0bytes\xff\n"
        );
    }
    let document = open(
        "flate",
        &OpenOptions {
            limits: Limits {
                decoded_bytes: 4,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let annotations = document
        .page(0)
        .unwrap()
        .attachment_annotations(&document)
        .unwrap();
    assert_eq!(
        annotations[0].specification.embedded[0]
            .file
            .data()
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
    for name in ["encrypted-no-type", "encrypted-identity"] {
        for password in [b"".as_slice(), b"wrong", b"attachment", b"owner"] {
            let document = open(
                name,
                &OpenOptions {
                    password: password.to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let annotations = document
                .page(0)
                .unwrap()
                .attachment_annotations(&document)
                .unwrap();
            let payload = annotations[0].specification.embedded[0].file.data();
            if name == "encrypted-identity" {
                assert_eq!(payload.unwrap().as_ref(), b"public attachment bytes");
            } else if password == b"attachment" || password == b"owner" {
                assert_eq!(payload.unwrap().as_ref(), b"attached file bytes");
            } else {
                assert_eq!(
                    payload.unwrap_err().as_error().unwrap().kind,
                    ErrorKind::Password
                );
            }
        }
    }
}
