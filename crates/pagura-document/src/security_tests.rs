//! Independent dictionary/vector regressions for clauses 7.5.5 and 7.6.

use crate::{Document, OpenOptions, tags};
use pagura_core::{ErrorKind, Limits, ObjectId, Recovery};
use pagura_testkit::fixture;

#[test]
fn standard_defaults_and_ignored_identity_entries_need_no_repair() {
    for name in [
        "dictionary-identity-shadow",
        "dictionary-auth-event-default",
        "dictionary-v4-no-length",
    ] {
        let document = Document::open(
            fixture("crypto", &format!("{name}.pdf")),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        assert_eq!(document.page_count().unwrap(), 1, "{name}");
        assert!(
            document
                .stream_data(document.object(ObjectId::new(4, 0)).unwrap())
                .unwrap()
                .starts_with(b"1 0 0 rg")
        );
        assert_eq!(document.diagnostics(), [], "{name}");
    }
}

#[test]
fn safe_dictionary_deviations_are_strict_errors_and_reported_repairs() {
    for (name, tag) in [
        ("dictionary-crypt-filter-type", tags::CRYPT_TYPE),
        ("dictionary-crypt-filter-no-length", tags::CRYPT_LENGTH),
        ("dictionary-standard-filter-name", tags::CRYPT_NAME),
        ("dictionary-perms-mismatch", tags::PERMS_MISMATCH),
        ("dictionary-id-absent", tags::DOCUMENT_ID),
        ("dictionary-id-one", tags::DOCUMENT_ID),
        ("dictionary-id-short", tags::DOCUMENT_ID),
    ] {
        let bytes = fixture("crypto", &format!("{name}.pdf"));
        let error = Document::open(
            bytes.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.repair, Some(tag), "{name}");
        assert_eq!(error.object, Some(ObjectId::new(5, 0)), "{name}");
        let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
        assert_eq!(document.page_count().unwrap(), 1, "{name}");
        assert!(
            document
                .stream_data(document.object(ObjectId::new(4, 0)).unwrap())
                .unwrap()
                .starts_with(b"1 0 0 rg")
        );
        assert!(
            document.diagnostics().iter().any(|item| item.repair == tag),
            "{name}"
        );
        assert_eq!(document.original_bytes(), bytes);
        let diagnostics = document.diagnostics();
        document.pages().unwrap();
        assert_eq!(document.diagnostics(), diagnostics);
    }
}

#[test]
fn delegation_is_not_identity_and_metadata_exemption_uses_catalog_identity() {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let error = Document::open(
            fixture("crypto", "dictionary-cfm-none.pdf"),
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error.feature(),
            Some(pagura_core::Feature::new("security.CFM"))
        );
        for (name, object) in [
            ("dictionary-other-metadata", 7),
            ("dictionary-legacy-metadata", 6),
        ] {
            let document = Document::open(
                fixture("crypto", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(
                document
                    .stream_data(document.object(ObjectId::new(object, 0)).unwrap())
                    .unwrap()
                    .as_ref(),
                fixture("crypto", &format!("{name}.expected"))
            );
        }
        let error = Document::open(
            fixture("document", "security-embedded-identity-filter.pdf"),
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error.feature(),
            Some(pagura_core::Feature::new("security.CFM"))
        );
    }
}

#[test]
fn padded_key_choice_precedes_cached_strings_and_obeys_decoding_limits() {
    for name in ["security-zero-padded-40", "security-zero-padded-48"] {
        let bytes = fixture("document", &format!("{name}.pdf"));
        let error = Document::open(
            bytes.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.repair, Some(tags::SHORT_KEY));
        for password in [b"".as_slice(), b"owner"] {
            let document = Document::open(
                bytes.clone(),
                &OpenOptions {
                    password: password.to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            let info = document.object(ObjectId::new(6, 0)).unwrap();
            assert_eq!(
                info.dictionary()
                    .unwrap()
                    .get(b"Title")
                    .unwrap()
                    .as_string(),
                Some(b"independent".as_slice())
            );
            assert_eq!(
                document
                    .stream_data(document.object(ObjectId::new(4, 0)).unwrap())
                    .unwrap()
                    .as_ref(),
                b"1 0 0 rg 8 8 48 32 re f\n"
            );
            assert_eq!(
                document
                    .diagnostics()
                    .iter()
                    .filter(|d| d.repair == tags::KEY_PADDING)
                    .count(),
                1
            );
            let retained = document.retained.used();
            document.pages().unwrap();
            document.object(ObjectId::new(6, 0)).unwrap();
            assert_eq!(document.retained.used(), retained);
            assert_eq!(document.original_bytes(), bytes);
        }
        let error = Document::open(
            bytes.clone(),
            &OpenOptions {
                password: b"wrong".to_vec(),
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Password);
        let error = Document::open(
            bytes,
            &OpenOptions {
                limits: Limits {
                    decoded_bytes: 64,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.object, Some(ObjectId::new(5, 0)));
    }
}
