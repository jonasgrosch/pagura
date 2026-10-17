#[cfg(test)]
mod shareability {
    use crate::Document;

    #[test]
    fn documents_are_shareable_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Document>();
    }
}

#[cfg(test)]
mod structure_tests {
    use crate::{Document, Location, ObjectSource, OpenOptions, RevisionKind};
    use pagura_core::{ErrorKind, Limits, ObjectId, Recovery, Value};
    use pagura_testkit::fixture;

    const CONTENT: &[u8] = b"1 0 0 rg 8 8 48 32 re f\n";

    fn load(name: &str, recovery: Recovery) -> pagura_core::Result<Document> {
        Document::open(
            fixture("document", &format!("{name}.pdf")),
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )
    }

    fn contents(document: &Document) -> pagura_core::Result<Vec<u8>> {
        let mut output = Vec::new();
        for page in document.pages()? {
            for stream in document.content_streams(page)? {
                output.extend_from_slice(&document.stream_data(stream)?);
            }
        }
        Ok(output)
    }

    #[test]
    fn independent_xref_object_stream_and_revision_oracles() {
        for name in [
            "xref-stream",
            "xref-index",
            "xref-default-fields",
            "xref-unknown-type",
            "object-stream",
            "compressed-reference-chain",
            "hybrid-precedence",
            "revision-redefine",
        ] {
            let document = load(name, Recovery::Strict).unwrap();
            assert_eq!(document.page_count().unwrap(), 1, "{name}");
            assert_eq!(contents(&document).unwrap(), CONTENT, "{name}");
            assert!(document.diagnostics().is_empty(), "{name}");
        }
    }

    #[test]
    fn revision_metadata_free_entries_and_trailer_inheritance() {
        let document = load("revision-redefine", Recovery::Strict).unwrap();
        assert_eq!(document.revisions().len(), 2);
        assert!(document.revisions()[0].offset() > document.revisions()[1].offset());
        assert_eq!(document.revisions()[0].kind(), RevisionKind::Table);
        assert!(!document.revisions()[0].trailer().contains_key(b"Root"));
        assert!(document.trailer().contains_key(b"Root"));
        let document = load("revision-free", Recovery::Strict).unwrap();
        assert_eq!(document.page_count().unwrap(), 0);
        assert!(
            document
                .resolve(&Value::Reference(ObjectId::new(3, 0)))
                .unwrap()
                .is_null()
        );
        assert!(
            document
                .resolve(&Value::Reference(ObjectId::new(3, 1)))
                .unwrap()
                .is_null()
        );
        let document = load("xref-unknown-type", Recovery::Strict).unwrap();
        assert!(
            document
                .resolve(&Value::Reference(ObjectId::new(7, 0)))
                .unwrap()
                .is_null()
        );
        assert_eq!(
            load("hybrid-precedence", Recovery::Strict)
                .unwrap()
                .revisions()[0]
                .kind(),
            RevisionKind::Hybrid
        );
        assert_eq!(
            load("xref-stream", Recovery::Strict).unwrap().revisions()[0].kind(),
            RevisionKind::Stream
        );
    }

    #[test]
    fn compressed_objects_share_one_decoded_stream_and_are_lazy() {
        let document = load("object-stream", Recovery::Strict).unwrap();
        assert!(
            document
                .slots
                .get(&ObjectId::new(3, 0))
                .unwrap()
                .object
                .get()
                .is_none()
        );
        let catalog = document.object(ObjectId::new(1, 0)).unwrap();
        assert_eq!(
            catalog.source(),
            &ObjectSource::Compressed {
                stream: 5,
                index: 0
            }
        );
        let cached = document
            .object_streams
            .get(&5)
            .unwrap()
            .get()
            .unwrap()
            .as_ref()
            .unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    assert_eq!(document.page_count().unwrap(), 1);
                });
            }
        });
        assert!(std::ptr::eq(
            cached,
            document
                .object_streams
                .get(&5)
                .unwrap()
                .get()
                .unwrap()
                .as_ref()
                .unwrap()
        ));
        assert!(std::ptr::eq(
            catalog,
            document.object(ObjectId::new(1, 0)).unwrap()
        ));
    }

    #[test]
    fn every_repair_has_a_strict_error_and_an_independent_result() {
        for (name, tags) in [
            ("nearby-offset", vec!["xref.offset-corrected"]),
            ("header-offset", vec!["file.header-offset"]),
            ("header-physical-offsets", vec!["file.header-offset"]),
            ("rebuild-trailer", vec!["xref.rebuilt"]),
            (
                "rebuild-catalog",
                vec!["xref.rebuilt", "trailer.reconstructed"],
            ),
            ("rebuild-object-stream", vec!["xref.rebuilt"]),
            ("short-length", vec!["stream.length-corrected"]),
            ("trailing-junk", vec!["file.trailing-bytes"]),
        ] {
            let document = load(name, Recovery::Repair).unwrap();
            assert_eq!(contents(&document).unwrap(), CONTENT, "{name}");
            let actual: Vec<_> = document
                .diagnostics()
                .iter()
                .map(|item| item.repair.as_str())
                .collect();
            assert_eq!(actual, tags, "{name}");
            let strict = load(name, Recovery::Strict).and_then(|document| contents(&document));
            assert_eq!(strict.unwrap_err().kind, ErrorKind::Syntax, "{name}");
        }
    }

    #[test]
    fn cycles_truncation_and_new_structure_budgets_are_explicit() {
        for name in ["prev-cycle", "stream-prev-cycle"] {
            assert_eq!(
                load(name, Recovery::Strict).unwrap_err().kind,
                ErrorKind::Cycle
            );
        }
        for name in ["truncated", "xref-truncated-fields"] {
            assert_eq!(
                load(name, Recovery::Strict).unwrap_err().kind,
                ErrorKind::Syntax
            );
        }
        for (name, limits) in [
            (
                "revision-redefine",
                Limits {
                    revisions: 1,
                    ..Limits::default()
                },
            ),
            (
                "xref-stream",
                Limits {
                    objects: 2,
                    ..Limits::default()
                },
            ),
            (
                "object-stream",
                Limits {
                    decoded_bytes: 60,
                    ..Limits::default()
                },
            ),
        ] {
            let result = Document::open(
                fixture("document", &format!("{name}.pdf")),
                &OpenOptions {
                    limits,
                    ..OpenOptions::default()
                },
            );
            assert_eq!(result.unwrap_err().kind, ErrorKind::Limit, "{name}");
        }
    }

    #[test]
    fn self_referential_stream_length_is_a_cycle_instead_of_recursion() {
        let mut bytes = fixture("classic", "rectangle.pdf");
        let at = bytes
            .windows(8)
            .position(|part| part == b"/Length ")
            .unwrap()
            + 8;
        // Use the same byte count, and point Length at the stream itself.
        bytes.splice(at..at + 2, b"4 0 R".iter().copied());
        // The shifted xref is deliberately removed: take an already open
        // document and replace immutable source in this private unit test.
        let mut document =
            Document::open(fixture("classic", "rectangle.pdf"), &OpenOptions::default()).unwrap();
        document.bytes = bytes.into();
        assert_eq!(
            document.object(ObjectId::new(4, 0)).unwrap_err().kind,
            ErrorKind::Cycle
        );
    }

    #[test]
    fn deterministic_mutations_of_new_structures_do_not_panic() {
        for name in [
            "xref-stream",
            "xref-default-fields",
            "object-stream",
            "hybrid-precedence",
            "revision-redefine",
            "rebuild-object-stream",
            "rebuild-truncated-trailer",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            for index in (0..bytes.len()).step_by(7) {
                for byte in [0, 255, b'(', b'[', b'9'] {
                    let mut data = bytes.clone();
                    data[index] = byte;
                    if let Ok(document) = Document::open(data, &OpenOptions::default()) {
                        let _ = contents(&document);
                        for id in document.object_ids() {
                            if let Ok(object) = document.object(id)
                                && object.is_stream()
                            {
                                let _ = document.stream_data(object);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn compressed_entry_must_match_header_identity_and_index() {
        let mut document = load("object-stream", Recovery::Strict).unwrap();
        document
            .slots
            .get_mut(&ObjectId::new(3, 0))
            .unwrap()
            .location = Location::Compressed {
            stream: 5,
            index: 99,
        };
        let error = document.object(ObjectId::new(3, 0)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.object, Some(ObjectId::new(3, 0)));
    }
}

mod reconstruction {
    use crate::{Document, OpenOptions};
    use pagura_core::{Diagnostics, ErrorKind, ObjectId, Recovery, Value};
    use pagura_testkit::fixture;

    const CONTENT: &[u8] = b"1 0 0 rg 8 8 48 32 re f\n";

    #[test]
    fn every_structural_failure_has_a_strict_error_and_a_reported_rebuild() {
        for name in [
            "rebuild-missing-startxref",
            "rebuild-startxref-outside",
            "rebuild-startxref-object",
            "rebuild-startxref-junk",
            "rebuild-catalog-offset",
            "rebuild-unusable-root",
            "rebuild-malformed-table",
            "rebuild-stream-length",
            "rebuild-prev-missing",
            "rebuild-prev-malformed",
            "rebuild-truncated-startxref",
            "rebuild-truncated-trailer",
            "rebuild-compressed-catalog",
            "rebuild-identities",
            "rebuild-last-trailer",
            "rebuild-opaque-payloads",
            "rebuild-malformed-last-trailer",
            "prev-cycle",
            "stream-prev-cycle",
            "truncated",
            "xref-truncated-fields",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let strict = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap_err();
            assert!(
                matches!(strict.kind, ErrorKind::Syntax | ErrorKind::Cycle),
                "{name}"
            );
            let document = Document::open(bytes, &OpenOptions::default()).unwrap();
            assert_eq!(document.page_count().unwrap(), 1, "{name}");
            let streams = document.content_streams(document.page(0).unwrap()).unwrap();
            assert_eq!(
                document.stream_data(streams[0]).unwrap().as_ref(),
                CONTENT,
                "{name}"
            );
            assert_eq!(
                document.diagnostics()[0].repair.as_str(),
                "xref.rebuilt",
                "{name}"
            );
            assert!(document.revisions().is_empty(), "{name}");
        }
    }

    #[test]
    fn trailer_identity_fields_survive_and_the_last_parseable_values_win() {
        for (name, info, id) in [
            ("rebuild-identities", 5, b"first".as_slice()),
            ("rebuild-last-trailer", 8, b"newer"),
            ("rebuild-malformed-last-trailer", 5, b"first"),
        ] {
            let document = Document::open(
                fixture("document", &format!("{name}.pdf")),
                &OpenOptions::default(),
            )
            .unwrap();
            assert_eq!(
                document.trailer().get(b"Root"),
                Some(&Value::Reference(ObjectId::new(1, 0)))
            );
            assert_eq!(
                document.trailer().get(b"Info"),
                Some(&Value::Reference(ObjectId::new(info, 0)))
            );
            assert_eq!(
                document.trailer().get(b"ID").unwrap().as_array().unwrap()[0].as_string(),
                Some(id)
            );
        }
        let document = Document::open(
            fixture("document", "rebuild-opaque-payloads.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        assert!(!document.object_ids().any(|id| id.number == 99));
        assert!(!document.trailer().contains_key(b"Encrypt"));
    }

    #[test]
    fn encrypted_rebuild_retains_authentication_fields_and_defers_compressed_objects() {
        for (name, encrypt, streams) in [
            ("security-rebuilt", 5, 0),
            ("security-rebuilt-object-stream", 7, 1),
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let mut diagnostics = Diagnostics::new(Recovery::Repair);
            let table = crate::xref::read(&bytes, pagura_core::Limits::default(), &mut diagnostics)
                .unwrap();
            assert_eq!(
                table.trailer.get(b"Encrypt"),
                Some(&Value::Reference(ObjectId::new(encrypt, 0)))
            );
            assert_eq!(
                table.trailer.get(b"ID").unwrap().as_array().unwrap()[0]
                    .as_string()
                    .unwrap(),
                (0_u8..16).map(|n| n * 17).collect::<Vec<_>>()
            );
            assert_eq!(table.rebuild_streams.len(), streams);
            assert_eq!(diagnostics.as_slice()[0].repair.as_str(), "xref.rebuilt");
            assert!(table.entries.contains_key(&encrypt));
        }
    }

    #[test]
    fn reconstruction_cannot_invent_a_missing_complete_catalog() {
        for name in ["rebuild-no-catalog", "rebuild-incomplete-object"] {
            let bytes = fixture("document", &format!("{name}.pdf"));
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
                    ErrorKind::Syntax
                );
            }
        }
    }

    #[test]
    fn unsupported_decoders_and_budgets_do_not_trigger_reconstruction() {
        let original = fixture("document", "xref-stream.pdf");
        let marker = original
            .windows(11)
            .position(|part| part == b"/Type /XRef")
            .unwrap();
        let mut bytes = original;
        bytes.splice(marker..marker, b"/Filter /JPXDecode ".iter().copied());
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let error = crate::xref::read(&bytes, pagura_core::Limits::default(), &mut diagnostics)
            .unwrap_err();
        assert!(error.is_unsupported());
        assert!(diagnostics.is_empty());
        let bytes = fixture("document", "rebuild-startxref-outside.pdf");
        let result = Document::open(
            bytes,
            &OpenOptions {
                limits: pagura_core::Limits {
                    objects: 1,
                    ..pagura_core::Limits::default()
                },
                ..OpenOptions::default()
            },
        );
        assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
    }
}

mod round3 {
    use crate::{Document, OpenOptions, tags};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn minimized_repair_pairs_keep_source_bytes_and_exact_stream_plaintext() {
        for (name, tag) in [
            ("header-text", tags::HEADER_FORMAT),
            ("header-space", tags::HEADER_FORMAT),
            ("header-tab", tags::HEADER_FORMAT),
            ("catalog-type-missing", tags::CATALOG_TYPE),
            ("stream-bare-cr", tags::STREAM_EOL),
            ("permissions-unsigned", tags::PERMISSIONS_UNSIGNED),
            ("hash-literal-eol", tags::LITERAL_HASH),
            ("hash-zero-padding", tags::HASH_PADDING),
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let strict = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .and_then(|document| {
                for object in document.content_streams(document.page(0)?)? {
                    document.stream_data(object)?;
                }
                Ok(())
            })
            .unwrap_err();
            assert_eq!(strict.kind, ErrorKind::Syntax, "{name}");
            assert_eq!(strict.repair, Some(tag), "{name}");
            let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
            assert_eq!(document.original_bytes(), bytes);
            assert_eq!(document.page_count().unwrap(), 1);
            let object = document.object(ObjectId::new(4, 0)).unwrap();
            assert_eq!(
                document.stream_data(object).unwrap().as_ref(),
                b"1 0 0 rg 8 8 48 32 re f\n",
                "{name}"
            );
            assert!(
                document.diagnostics().iter().any(|item| item.repair == tag),
                "{name}"
            );
            if name == "catalog-type-missing" {
                assert!(!document.catalog().unwrap().contains_key(b"Type"));
            }
        }
    }

    #[test]
    fn header_offset_and_nonstandard_line_repairs_compose() {
        let mut bytes = b"junk\n".to_vec();
        bytes.extend(fixture("document", "header-text.pdf"));
        let document = Document::open(bytes, &OpenOptions::default()).unwrap();
        assert_eq!(document.page_count().unwrap(), 1);
        assert_eq!(
            document
                .diagnostics()
                .iter()
                .map(|item| item.repair)
                .collect::<Vec<_>>(),
            [tags::HEADER_OFFSET, tags::HEADER_FORMAT]
        );
    }

    #[test]
    fn catalog_reconstruction_and_unlined_indirect_streams_preserve_following_objects() {
        for name in [
            "catalog-type-missing-rebuilt",
            "rebuild-indirect-stream-boundary",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            assert!(
                Document::open(
                    bytes.clone(),
                    &OpenOptions {
                        recovery: Recovery::Strict,
                        ..OpenOptions::default()
                    }
                )
                .is_err()
            );
            let document = Document::open(bytes, &OpenOptions::default()).unwrap();
            assert_eq!(document.page_count().unwrap(), 1);
            assert!(
                document
                    .diagnostics()
                    .iter()
                    .any(|item| item.repair == tags::XREF_REBUILT)
            );
        }
    }
}

mod repaired_authentication {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, ObjectId};
    use pagura_testkit::fixture;

    #[test]
    fn repaired_security_fields_authenticate_both_authorities_and_reject_wrong_passwords() {
        for name in [
            "permissions-unsigned",
            "hash-zero-padding",
            "hash-literal-eol",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let document = Document::open(
                bytes.clone(),
                &OpenOptions {
                    password: b"owner".to_vec(),
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
