#![forbid(unsafe_code)]
//! Typed file views against independently authored source data.

use pagura_core::{ErrorKind, Limits, ObjectId, Recovery, Result, Value};
use pagura_document::{
    Document, OpenOptions,
    files::{FileName, Relationship},
};
use pagura_testkit::{fixture, unhex_array};

fn open(name: &str, options: &OpenOptions) -> Result<Document> {
    Document::open(fixture("files", &format!("{name}.pdf")), options)
}

#[test]
fn filenames_tree_keys_parameters_and_payloads_keep_their_own_identity() {
    let document = open("populated", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].key, b"lookup-key");
    let spec = &files[0].specification;
    assert_eq!(spec.object, Some(ObjectId::new(5, 0)));
    assert_eq!(spec.names.file, Some(b"legacy.txt".as_slice()));
    assert_eq!(
        spec.names.preferred(),
        Some(FileName::Unicode("Grüße Ω.txt"))
    );
    assert_eq!(spec.description.as_deref(), Some("Attachment €"));
    assert_eq!(spec.relationship, Relationship::Data);
    assert!(spec.volatile);
    assert_eq!(spec.identifiers, Some([b"original".as_slice(), b"current"]));
    assert!(spec.source.as_dictionary().unwrap().contains_key(b"Custom"));
    assert_eq!(spec.embedded.len(), 2);
    for (entry, key) in spec.embedded.iter().zip([b"F".as_slice(), b"UF"]) {
        assert_eq!(entry.key.as_bytes(), key);
        let file = &entry.file;
        assert_eq!(file.object(), ObjectId::new(6, 0));
        assert!(file.source().is_stream());
        assert_eq!(
            file.media_type().unwrap().as_bytes(),
            b"application/octet-stream"
        );
        let params = file.parameters().unwrap();
        assert_eq!(params.size, Some(18));
        assert_eq!(params.creation_date.as_deref(), Some("D:20261026000000Z"));
        assert_eq!(
            params.modification_date.as_deref(),
            Some("D:20261026010000Z")
        );
        assert_eq!(
            params.checksum,
            Some(&unhex_array("b1b13bd1129967addd47be33623d53b6"))
        );
        assert_eq!(file.data().unwrap().as_ref(), b"attachment\0bytes\xff\n");
    }
    assert_eq!(
        document.materialize().unwrap().bytes(),
        fixture("files", "populated.pdf")
    );
}

#[test]
fn filters_empty_streams_and_optional_type_are_decoded_lazily() {
    for (name, expected) in [
        ("flate", b"attachment\0bytes\xff\n".as_slice()),
        ("stream-type-absent", b"attachment\0bytes\xff\n"),
        ("empty-payload", b""),
    ] {
        let document = open(name, &OpenOptions::default()).unwrap();
        let files = document.embedded_files().unwrap();
        for entry in &files[0].specification.embedded {
            assert_eq!(entry.file.data().unwrap().as_ref(), expected);
        }
    }
    let document = open("related", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    let related = &files[0].specification.related;
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].key.as_bytes(), b"F");
    assert_eq!(related[0].name, b"sidecar");
    assert_eq!(related[0].file.object(), ObjectId::new(7, 0));
    assert_eq!(related[0].file.data().unwrap().as_ref(), b"related bytes");
}

#[test]
fn external_paths_and_extension_relationships_are_only_data() {
    let document = open("external-url", &OpenOptions::default()).unwrap();
    let files = document.associated_files().unwrap();
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file.file_system.unwrap().as_bytes(), b"URL");
    assert_eq!(
        file.names.preferred(),
        Some(FileName::Bytes(b"https://example.invalid/never-fetch"))
    );
    assert!(file.embedded.is_empty());
    assert!(
        matches!(file.relationship, Relationship::Other(name) if name.as_bytes() == b"Vendor_Relation")
    );
    let document = open("simple-string", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files[0].specification.names.preferred(),
        Some(FileName::Bytes(b"/raw\xff/file"))
    );
    let document = open("legacy-paths", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    let names = &files[0].specification.names;
    assert!(names.preferred().is_none());
    assert_eq!(names.dos, Some(b"C:\\legacy.dat".as_slice()));
    assert_eq!(names.mac, Some(b"disk:legacy.dat".as_slice()));
    assert_eq!(names.unix, Some(b"/tmp/legacy.dat".as_slice()));
    let value = Value::String(b"direct path".to_vec());
    let spec = document.file_specification(&value).unwrap();
    assert_eq!(spec.object, None);
    assert_eq!(
        spec.names.preferred(),
        Some(FileName::Bytes(b"direct path"))
    );
}

#[test]
fn missing_null_duplicate_and_associated_entries_preserve_declared_order() {
    let document = open("empty-unicode-name", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files[0].specification.names.preferred(),
        Some(FileName::Unicode(""))
    );
    let document = open("associated-direct-external", &OpenOptions::default()).unwrap();
    let files = document.associated_files().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].object, None);
    for name in ["absent", "null-tree", "empty-tree"] {
        let document = open(name, &OpenOptions::default()).unwrap();
        assert!(document.embedded_files().unwrap().is_empty());
        assert!(document.associated_files().unwrap().is_empty());
    }
    let document = open("null-entries", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    let spec = &files[0].specification;
    assert_eq!(spec.names.preferred(), Some(FileName::Bytes(b"null.bin")));
    assert_eq!(spec.relationship, Relationship::Unspecified);
    assert!(!spec.volatile);
    assert!(spec.embedded.is_empty());
    assert!(document.associated_files().unwrap().is_empty());
    let document = open("duplicate-payload", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files.iter().map(|entry| entry.key).collect::<Vec<_>>(),
        [b"a", b"b"]
    );
    assert_eq!(files[0].specification.object, files[1].specification.object);
    let document = open("populated", &OpenOptions::default()).unwrap();
    assert_eq!(document.associated_files().unwrap().len(), 1);
    let page = document.page(0).unwrap();
    let files = page.associated_files(&document).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].object, files[1].object);
    assert_eq!(
        document
            .object_associated_files(page.dictionary())
            .unwrap()
            .len(),
        2
    );
    let another = open("populated", &OpenOptions::default()).unwrap();
    assert_eq!(
        page.associated_files(&another)
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn malformed_views_and_reference_cycles_never_become_empty_successes() {
    for name in [
        "wrong-spec-type",
        "spec-stream",
        "missing-file-name",
        "wrong-file-name",
        "wrong-unicode-name",
        "wrong-file-system",
        "wrong-volatile",
        "wrong-relationship",
        "wrong-identifier",
        "wrong-spec-type-name",
        "embedded-missing-type",
        "embedded-wrong-type",
        "embedded-no-matching-name",
        "embedded-invalid-key",
        "embedded-direct-stream",
        "embedded-nonstream",
        "related-without-embedded",
        "related-unmatched-key",
        "related-odd-array",
        "stream-wrong-type",
        "stream-wrong-subtype",
        "stream-wrong-params",
        "negative-size",
        "checksum-length",
        "wrong-date",
    ] {
        let document = open(name, &OpenOptions::default()).unwrap();
        assert_eq!(
            document
                .embedded_files()
                .unwrap_err()
                .as_error()
                .unwrap()
                .kind,
            ErrorKind::Syntax,
            "{name}"
        );
    }
    let document = open("embedded-nonstream", &OpenOptions::default()).unwrap();
    assert_eq!(
        document
            .embedded_files()
            .unwrap_err()
            .as_error()
            .unwrap()
            .object,
        Some(ObjectId::new(8, 0))
    );
    let document = open("spec-cycle", &OpenOptions::default()).unwrap();
    assert_eq!(
        document
            .embedded_files()
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Cycle
    );
    for name in [
        "associated-wrong-type",
        "associated-string",
        "associated-direct-embedded",
    ] {
        let document = open(name, &OpenOptions::default()).unwrap();
        assert_eq!(
            document
                .associated_files()
                .unwrap_err()
                .as_error()
                .unwrap()
                .kind,
            ErrorKind::Syntax
        );
    }
}

#[test]
fn text_and_tree_repairs_remain_visible_and_strictly_rejectable() {
    let strict = OpenOptions {
        recovery: Recovery::Strict,
        ..OpenOptions::default()
    };
    for (name, repair) in [
        ("invalid-unicode", pagura_core::repairs::INVALID_TEXT),
        ("tree-order", pagura_document::repairs::TREE_ORDER),
    ] {
        let document = open(name, &strict).unwrap();
        assert_eq!(
            document
                .embedded_files()
                .unwrap_err()
                .as_error()
                .unwrap()
                .repair,
            Some(repair)
        );
        let document = open(name, &OpenOptions::default()).unwrap();
        let files = document.embedded_files().unwrap();
        assert!(!files.is_empty());
        assert!(
            document
                .diagnostics()
                .iter()
                .any(|d| d.repair() == Some(repair))
        );
        if name == "tree-order" {
            assert_eq!(
                files.iter().map(|file| file.key).collect::<Vec<_>>(),
                [b"a", b"z"]
            );
        } else {
            assert_eq!(files[0].specification.names.unicode.as_deref(), Some("�"));
        }
    }
}

#[test]
fn bounded_extraction_does_not_follow_external_streams() {
    let document = open("external-stream", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files[0].specification.embedded[0]
            .file
            .data()
            .unwrap_err()
            .as_error()
            .unwrap()
            .feature(),
        Some(pagura_document::features::EXTERNAL_STREAM)
    );
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
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files[0].specification.embedded[0]
            .file
            .data()
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
    let document = open(
        "populated",
        &OpenOptions {
            limits: Limits {
                traversal_work: 0,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        document
            .embedded_files()
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn embedded_authorization_applies_without_a_type_and_with_explicit_crypt() {
    let document = open("encrypted-identity-no-type", &OpenOptions::default()).unwrap();
    let files = document.embedded_files().unwrap();
    assert_eq!(
        files[0].specification.embedded[0]
            .file
            .data()
            .unwrap()
            .as_ref(),
        b"public attachment bytes"
    );
    for name in [
        "encrypted",
        "encrypted-no-type",
        "encrypted-explicit-no-type",
    ] {
        for password in [b"".as_slice(), b"wrong", b"attachment", b"owner"] {
            let document = open(
                name,
                &OpenOptions {
                    password: password.to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            let files = document.embedded_files().unwrap();
            let file = &files[0].specification.embedded[0].file;
            if password == b"attachment" || password == b"owner" {
                assert_eq!(file.data().unwrap().as_ref(), b"attached file bytes");
            } else {
                let error = file.data().unwrap_err();
                assert_eq!(error.as_error().unwrap().kind, ErrorKind::Password);
                assert_eq!(error.as_error().unwrap().object, Some(ObjectId::new(6, 0)));
                assert_eq!(file.data().unwrap_err(), error);
            }
        }
    }
}
