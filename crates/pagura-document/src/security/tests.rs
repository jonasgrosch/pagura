//! Independent dictionary/vector regressions for clauses 7.5.5 and 7.6.

use crate::{Document, OpenOptions, features, repairs};
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
        ("dictionary-crypt-filter-type", repairs::CRYPT_TYPE),
        ("dictionary-crypt-filter-no-length", repairs::CRYPT_LENGTH),
        ("dictionary-standard-filter-name", repairs::CRYPT_NAME),
        ("dictionary-perms-mismatch", repairs::PERMS_MISMATCH),
        ("dictionary-id-absent", repairs::DOCUMENT_ID),
        ("dictionary-id-one", repairs::DOCUMENT_ID),
        ("dictionary-id-short", repairs::DOCUMENT_ID),
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
            document
                .diagnostics()
                .iter()
                .any(|item| item.repair() == Some(tag)),
            "{name}"
        );
        assert_eq!(document.materialize().unwrap().bytes(), bytes);
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
        assert_eq!(error.feature(), Some(features::CRYPT_METHOD));
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
        assert_eq!(error.feature(), Some(features::CRYPT_METHOD));
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
        assert_eq!(error.repair, Some(repairs::SHORT_KEY));
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
                    .filter(|d| d.repair() == Some(repairs::KEY_PADDING))
                    .count(),
                1
            );
            let retained = document.retained.used();
            document.pages().unwrap();
            document.object(ObjectId::new(6, 0)).unwrap();
            assert_eq!(document.retained.used(), retained);
            assert_eq!(document.materialize().unwrap().bytes(), bytes);
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

mod unit {
    use crate::security::{Methods, lookup};
    fn methods<'a>(
        dictionary: &'a Dictionary,
        params: &pagura_crypto::standard::SecurityParams,
        resolve: impl Fn(&'a Value) -> crate::source::ReadResult<&'a Value>,
    ) -> crate::source::ReadResult<Methods> {
        crate::security::methods(
            dictionary,
            params,
            resolve,
            &mut crate::security::policy::Policy::permissive(),
        )
    }
    use crate::stream::validate_crypt;
    use crate::{Document, OpenOptions};
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Dictionary, ErrorKind, Limits, Value};
    use pagura_crypto::standard::CryptMethod;
    use pagura_testkit::fixture;

    fn parameters(
        dictionary: &Dictionary,
        trailer: &Dictionary,
    ) -> pagura_core::Result<pagura_crypto::standard::SecurityParams> {
        crate::security::parameters(dictionary, trailer, None)
    }

    fn dictionary(bytes: &[u8]) -> Dictionary {
        Parser::new(bytes, Origin::Buffer, Limits::default())
            .parse()
            .unwrap()
            .as_dictionary()
            .unwrap()
            .clone()
    }

    fn trailer() -> Dictionary {
        dictionary(b"<< /ID [<00112233> <44556677>] >>")
    }

    #[test]
    fn versions_key_lengths_and_plain_parameters_match_the_standard() {
        for (version, revision, length) in
            [(1, 2, 5), (2, 3, 16), (4, 4, 16), (5, 5, 32), (5, 6, 32)]
        {
            let encryption = dictionary(format!("<< /V {version} /R {revision} /Length 128 /O (owner) /U (user) /P -4 /OE (oe) /UE (ue) /Perms (perms) /EncryptMetadata false >>").as_bytes());
            let params = parameters(&encryption, &trailer()).unwrap();
            assert_eq!(params.version, version);
            assert_eq!(params.revision, revision);
            assert_eq!(params.key_length, length);
            assert_eq!(params.permissions, -4);
            assert_eq!(params.owner_hash, b"owner");
            assert_eq!(params.user_hash, b"user");
            assert_eq!(params.owner_encrypted_key.as_deref(), Some(&b"oe"[..]));
            assert_eq!(params.user_encrypted_key.as_deref(), Some(&b"ue"[..]));
            assert_eq!(params.encrypted_permissions.as_deref(), Some(&b"perms"[..]));
            assert_eq!(params.document_id, [0, 0x11, 0x22, 0x33]);
            assert_eq!(params.encrypt_metadata, version < 4);
        }
    }

    #[test]
    fn crypt_filter_defaults_identity_and_embedded_file_events() {
        let encryption = dictionary(b"<< /V 4 /R 4 /Length 128 /O () /U () /P -4 /CF << /RC << /CFM /V2 /Length 16 >> /AES << /CFM /AESV2 >> /EF << /CFM /V2 /AuthEvent /EFOpen >> >> /StrF /Identity /StmF /AES /EFF /EF >>");
        let params = parameters(&encryption, &trailer()).unwrap();
        let Methods {
            filters,
            strings,
            streams,
            embedded,
        } = methods(&encryption, &params, Ok).unwrap();
        assert_eq!(strings, CryptMethod::Identity);
        assert_eq!(streams, CryptMethod::AesV2);
        assert_eq!(embedded, CryptMethod::Rc4);
        assert_eq!(lookup(&filters, b"RC", false).unwrap(), CryptMethod::Rc4);
        assert_eq!(
            lookup(&filters, b"Identity", false).unwrap(),
            CryptMethod::Identity
        );
        assert_eq!(
            lookup(&filters, b"EF", false).unwrap_err().kind,
            ErrorKind::Syntax
        );
        let mut encryption = encryption;
        encryption.remove(b"StrF");
        encryption.remove(b"StmF");
        encryption.remove(b"EFF");
        let Methods {
            strings,
            streams,
            embedded,
            ..
        } = methods(&encryption, &params, Ok).unwrap();
        assert_eq!(
            (strings, streams, embedded),
            (
                CryptMethod::Identity,
                CryptMethod::Identity,
                CryptMethod::Identity
            )
        );
    }

    #[test]
    fn invalid_security_parameters_are_never_silently_defaulted() {
        for entry in [
            b"/Length 41".as_slice(),
            b"/Length -8",
            b"/EncryptMetadata 1",
            b"/P 2147483648",
            b"/U 9",
        ] {
            let base = b"<< /V 4 /R 4 /Length 128 /O () /U () /P -4 >>";
            let mut encryption = dictionary(base);
            encryption.extend(dictionary(&[b"<< ".as_slice(), entry, b" >>"].concat()));
            assert_eq!(
                parameters(&encryption, &trailer()).unwrap_err().kind,
                ErrorKind::Syntax
            );
        }
        for entry in [
            b"/CF << /X << /CFM 1 >> >>".as_slice(),
            b"/StmF /Missing",
            b"/StrF 1",
            b"/CF << /X << /CFM /V2 /Length 16 /AuthEvent /Wrong >> >>",
        ] {
            let mut encryption = dictionary(b"<< /V 4 /R 4 /Length 128 /O () /U () /P -4 >>");
            encryption.extend(dictionary(&[b"<< ".as_slice(), entry, b" >>"].concat()));
            let params = parameters(&encryption, &trailer()).unwrap();
            assert_eq!(
                methods(&encryption, &params, Ok)
                    .unwrap_err()
                    .as_error()
                    .unwrap()
                    .kind,
                ErrorKind::Syntax
            );
        }
        assert_eq!(
            validate_crypt(
                Some(&Value::name(b"Crypt")),
                Some(&Value::Dictionary(dictionary(b"<< /Name 7 >>")))
            )
            .unwrap_err()
            .kind,
            ErrorKind::Syntax
        );
    }

    #[test]
    fn handler_errors_propagate_and_password_debug_is_redacted() {
        let mut document =
            Document::open(fixture("classic", "rectangle.pdf"), &OpenOptions::default()).unwrap();
        document.trailer.insert(
            pagura_core::Name::new(b"Encrypt"),
            Value::Dictionary(dictionary(
                b"<< /Filter /Standard /V 1 /R 2 /O () /U () /P -4 >>",
            )),
        );
        document.trailer.extend(trailer());
        let parameters = parameters(
            document
                .trailer
                .get(b"Encrypt")
                .unwrap()
                .as_dictionary()
                .unwrap(),
            document.trailer(),
        )
        .unwrap();
        let expected = pagura_crypto::standard::authenticate(&parameters, b"").unwrap_err();
        assert_eq!(
            crate::security::read(&document, &[])
                .unwrap_err()
                .as_error(),
            Some(&expected)
        );
        *document.trailer.get_mut(b"Encrypt").unwrap() =
            Value::Dictionary(dictionary(b"<< /Filter /Adobe.PubSec >>"));
        assert_eq!(
            crate::security::read(&document, &[])
                .unwrap_err()
                .as_error()
                .unwrap()
                .feature(),
            Some(crate::features::PUBSEC)
        );
        let options = OpenOptions {
            password: b"private-password".to_vec(),
            ..OpenOptions::default()
        };
        assert!(!format!("{options:?}").contains("private-password"));
    }
}

mod indirect_filters {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn indirect_filter_dictionaries_authenticate_in_strict_mode() {
        for name in ["security-indirect-cf", "security-indirect-filter"] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            for password in [b"".as_slice(), b"owner"] {
                let document = Document::open(
                    bytes.clone(),
                    &OpenOptions {
                        recovery: Recovery::Strict,
                        password: password.to_vec(),
                        ..OpenOptions::default()
                    },
                )
                .unwrap();
                assert_eq!(
                    document
                        .stream_data(document.object(ObjectId::new(4, 0)).unwrap())
                        .unwrap()
                        .as_ref(),
                    b"1 0 0 rg 8 8 48 32 re f\n"
                );
                assert_eq!(document.diagnostics(), []);
            }
            assert_eq!(
                Document::open(
                    bytes,
                    &OpenOptions {
                        password: b"wrong".to_vec(),
                        ..OpenOptions::default()
                    }
                )
                .unwrap_err()
                .kind,
                ErrorKind::Password
            );
        }
    }
}

mod authorization_events {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn embedded_file_authorization_does_not_block_identity_page_data() {
        let name = "security-embedded-event";
        let bytes = fixture("document", &format!("{name}.pdf"));
        assert_eq!(
            Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                }
            )
            .unwrap_err()
            .repair,
            Some(crate::repairs::CRYPT_EVENT)
        );
        let recovery = Recovery::Repair;
        for password in [b"".as_slice(), b"wrong"] {
            let document = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery,
                    password: password.to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            assert_eq!(
                document
                    .catalog()
                    .unwrap()
                    .get(b"Marker")
                    .unwrap()
                    .as_string(),
                Some(b"independent".as_slice())
            );
            let content = document.object(ObjectId::new(4, 0)).unwrap();
            assert_eq!(
                document.stream_data(content).unwrap().as_ref(),
                b"1 0 0 rg 8 8 48 32 re f\n"
            );
            let embedded = document.object(ObjectId::new(6, 0)).unwrap();
            let error = document.stream_data(embedded).unwrap_err();
            assert_eq!(error.as_error().unwrap().kind, ErrorKind::Password);
            assert_eq!(error.as_error().unwrap().object, Some(embedded.id()));
            let retained = document.retained.used();
            assert_eq!(document.stream_data(embedded).unwrap_err(), error);
            assert_eq!(document.retained.used(), retained);
            assert!(
                document
                    .diagnostics()
                    .iter()
                    .any(|d| d.repair() == Some(crate::repairs::CRYPT_EVENT))
            );
        }
        for password in [b"attachment".as_slice(), b"owner"] {
            let document = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery,
                    password: password.to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(
                document
                    .stream_data(document.object(ObjectId::new(6, 0)).unwrap())
                    .unwrap()
                    .as_ref(),
                b"attached file bytes"
            );
        }
    }

    #[test]
    fn doc_open_is_required_for_default_filters_and_explicit_doc_open_events() {
        let bytes = fixture("document", "security-embedded-doc-open.pdf");
        for recovery in [Recovery::Strict, Recovery::Repair] {
            assert_eq!(
                Document::open(
                    bytes.clone(),
                    &OpenOptions {
                        recovery,
                        ..OpenOptions::default()
                    }
                )
                .unwrap_err()
                .kind,
                ErrorKind::Password
            );
            let document = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery,
                    password: b"attachment".to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            let default = Document::open(
                fixture("document", "security-default-event-override.pdf"),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            assert_eq!(
                default
                    .stream_data(default.object(ObjectId::new(4, 0)).unwrap())
                    .unwrap()
                    .as_ref(),
                b"1 0 0 rg 8 8 48 32 re f\n"
            );
            assert_eq!(default.diagnostics(), []);
        }
    }
}

mod missing_length {
    use crate::{Document, OpenOptions, repairs};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn only_a_unique_authenticated_size_replaces_the_missing_default() {
        let bytes = fixture("document", "security-length-missing.pdf");
        for password in [b"".as_slice(), b"owner"] {
            let options = OpenOptions {
                password: password.to_vec(),
                ..OpenOptions::default()
            };
            let strict = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..options.clone()
                },
            )
            .unwrap_err();
            assert_eq!(strict.repair, Some(repairs::KEY_LENGTH_MISSING));
            assert_eq!(strict.object, Some(ObjectId::new(5, 0)));
            let document = Document::open(bytes.clone(), &options).unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            assert_eq!(
                document
                    .catalog()
                    .unwrap()
                    .get(b"Marker")
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
                    .filter(|d| d.repair() == Some(repairs::KEY_LENGTH_MISSING))
                    .count(),
                1
            );
        }
        for name in [
            "security-length-missing",
            "security-length-missing-password",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let error = Document::open(
                bytes,
                &OpenOptions {
                    password: b"wrong".to_vec(),
                    ..OpenOptions::default()
                },
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Password);
            assert_eq!(error.repair, None);
        }
    }
}
