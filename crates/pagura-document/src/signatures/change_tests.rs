use super::*;
use crate::{Document, OpenOptions};
use pagura_core::{ErrorKind, Limits, ObjectId};
use pagura_testkit::{fixture, fixture_path};
use std::fmt::Write;

fn open(name: &str) -> Document {
    Document::open(
        fixture("signature-changes", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
    .unwrap()
}
#[test]
fn independent_modification_classifications() {
    let mut count = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(fixture_path("signature-changes", "")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|s| s != "mdp") {
            continue;
        }
        count += 1;
        let name = path.file_stem().unwrap().to_str().unwrap();
        let expected = std::fs::read_to_string(&path).unwrap();
        let doc = open(name);
        let signatures = doc.signatures().unwrap_or_else(|e| panic!("{name}: {e}"));
        let signature = signatures
            .fields
            .iter()
            .filter(|f| f.qualified_name == "Approval")
            .filter_map(|f| f.signature.as_ref())
            .chain(
                signatures
                    .historical
                    .iter()
                    .filter(|h| h.field_name.as_deref() == Some("Approval"))
                    .map(|h| &h.signature),
            )
            .min_by_key(|s| {
                s.byte_range
                    .as_ref()
                    .map_or(usize::MAX, ByteRange::signed_end)
            })
            .unwrap();
        let PermittedModifications::Analysed(analysis) = &signature.results.permitted_modifications
        else {
            assert_eq!(
                signature.results.permitted_modifications,
                PermittedModifications::Inconclusive,
                "{name}"
            );
            assert_eq!(expected, "Inconclusive\n", "{name}");
            continue;
        };
        let mut actual = format!("{:?}\n", analysis.outcome);
        let mut net = format!("{:?}\n", analysis.outcome);
        for (output, cumulative) in [(&mut actual, false), (&mut net, true)] {
            for revision in &analysis.revisions {
                if analysis.revisions.len() > 1 {
                    writeln!(output, "@{}|{:?}", revision.revision, revision.outcome).unwrap();
                }
                for change in if cumulative {
                    &revision.net_changes
                } else {
                    &revision.changes
                } {
                    let target = match change.target {
                        ChangeTarget::Object(id) => {
                            if id.generation == 0 {
                                id.number.to_string()
                            } else {
                                format!("{}:{}", id.number, id.generation)
                            }
                        }
                        ChangeTarget::Trailer => "trailer".into(),
                    };
                    writeln!(
                        output,
                        "{target}|{:?}|{:?}|{:?}|{:?}|{}",
                        change.kind,
                        change.doc_mdp,
                        change.field_mdp,
                        change.rule,
                        if change.visible_overlay {
                            "overlay"
                        } else {
                            "clear"
                        }
                    )
                    .unwrap();
                }
            }
        }
        let net_expected = std::fs::read_to_string(path.with_extension("net")).unwrap();
        if net != net_expected {
            failures.push(format!(
                "{name} net: expected {net_expected:?}, found {net:?}"
            ));
        }
        if actual != expected {
            failures.push(format!("{name}: expected {expected:?}, found {actual:?}"));
        }
        assert_eq!(
            signature.results.integrity,
            Integrity::ByteRangeCheckedCryptographyNotChecked
        );
        assert_eq!(signature.results.signer_trust, SignerTrust::NotCheckedYet);
    }
    assert!(count >= 20);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn historical_objects_are_prefix_bound_and_cached() {
    let doc = open("form-fill-p2");
    let id = ObjectId::new(7, 0);
    let old = doc.object_at_revision(1, id).unwrap();
    assert_eq!(
        old.dictionary().unwrap().get(b"V").unwrap().as_string(),
        Some(b"old".as_slice())
    );
    assert_eq!(
        doc.object(id)
            .unwrap()
            .dictionary()
            .unwrap()
            .get(b"V")
            .unwrap()
            .as_string(),
        Some(b"new".as_slice())
    );
    assert!(std::ptr::eq(
        doc.revision_document(1)
            .unwrap()
            .materialize()
            .unwrap()
            .bytes()
            .as_ptr(),
        doc.materialize().unwrap().bytes().as_ptr()
    ));
    let used = doc.retained.used();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| doc.object_at_revision(1, id).unwrap()))
            .collect();
        for handle in handles {
            assert!(std::ptr::eq(old, handle.join().unwrap()));
        }
    });
    assert_eq!(doc.retained.used(), used);
    assert_eq!(
        doc.object_at_revision(2, id)
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        doc.object_at_revision(1, ObjectId::new(7, 1))
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::MissingObject
    );
    let doc = open("signature-added");
    assert_eq!(
        doc.object_at_revision(1, ObjectId::new(9, 0))
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::MissingObject
    );
}
#[test]
fn historical_caches_share_the_aggregate_limit() {
    let probe = open("form-fill-p2");
    probe.object_at_revision(1, ObjectId::new(7, 0)).unwrap();
    let used = probe.retained.used();
    let options = OpenOptions {
        limits: Limits {
            document_bytes: used - 1,
            ..Limits::default()
        },
        ..OpenOptions::default()
    };
    let doc = Document::open(fixture("signature-changes", "form-fill-p2.pdf"), &options).unwrap();
    assert_eq!(
        doc.object_at_revision(1, ObjectId::new(7, 0))
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
    assert!(doc.retained.used() < used);
}

#[test]
fn historical_generations_frees_and_stream_dependencies_use_the_old_xref() {
    let doc = open("field-generation-replaced");
    let old = ObjectId::new(7, 0);
    let new = ObjectId::new(7, 1);
    assert_eq!(
        doc.object(old).unwrap_err().as_error().unwrap().kind,
        ErrorKind::MissingObject
    );
    assert_eq!(
        doc.object_at_revision(1, new)
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::MissingObject
    );
    assert_eq!(
        doc.object_at_revision(1, old)
            .unwrap()
            .dictionary()
            .unwrap()
            .get(b"V")
            .unwrap()
            .as_string(),
        Some(b"old".as_slice())
    );
    assert_eq!(
        doc.object(new)
            .unwrap()
            .dictionary()
            .unwrap()
            .get(b"V")
            .unwrap()
            .as_string(),
        Some(b"new".as_slice())
    );
    let doc = open("annotation-deleted-p3");
    assert_eq!(
        doc.object(ObjectId::new(10, 0))
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::MissingObject
    );
    assert!(doc.object_at_revision(1, ObjectId::new(10, 0)).is_ok());
    let doc = open("historical-stream-length");
    let old = doc.object_at_revision(1, ObjectId::new(12, 0)).unwrap();
    assert_eq!(doc.raw_stream_data(old).unwrap().bytes(), b"old");
    assert_eq!(
        doc.raw_stream_data(doc.object(ObjectId::new(12, 0)).unwrap())
            .unwrap()
            .bytes(),
        b"newtext"
    );
    assert_eq!(
        doc.object_at_revision(1, ObjectId::new(13, 0))
            .unwrap()
            .value()
            .as_integer(),
        Some(3)
    );
    assert_eq!(
        doc.object(ObjectId::new(13, 0))
            .unwrap()
            .value()
            .as_integer(),
        Some(7)
    );
}

#[test]
fn hidden_signatures_and_inaccessible_history_remain_visible() {
    for name in ["removed-signature", "replaced-signature"] {
        let doc = open(name);
        let signatures = doc.signatures().unwrap();
        assert!(signatures.history_complete);
        assert_eq!(signatures.historical.len(), 1);
        let historical = &signatures.historical[0];
        assert_eq!(historical.field_name.as_deref(), Some("Approval"));
        assert_eq!(historical.signature.object, Some(ObjectId::new(5, 0)));
        assert!(matches!(&historical.signature.results.revision_coverage,
            RevisionCoverage::Covered { index: 1, later_revisions, .. } if later_revisions.len() == 1));
        assert!(matches!(
            &historical.signature.results.permitted_modifications,
            PermittedModifications::Analysed(ModificationAnalysis {
                outcome: ModificationOutcome::Disallowed,
                ..
            })
        ));
    }
    let doc = open("historical-forward-reference");
    let signatures = doc.signatures().unwrap();
    assert!(!signatures.history_complete);
    assert_eq!(
        signatures.fields[0]
            .signature
            .as_ref()
            .unwrap()
            .results
            .permitted_modifications,
        PermittedModifications::Inconclusive
    );
    assert!(doc.object_at_revision(1, ObjectId::new(12, 0)).is_err());
}

#[test]
fn shared_rendering_uses_override_appearance_permission() {
    let doc = open("shared-appearance-content");
    let signatures = doc.signatures().unwrap();
    let PermittedModifications::Analysed(analysis) = &signatures.fields[0]
        .signature
        .as_ref()
        .unwrap()
        .results
        .permitted_modifications
    else {
        panic!("missing analysis");
    };
    let change = analysis.revisions[1]
        .changes
        .iter()
        .find(|c| c.target == ChangeTarget::Object(ObjectId::new(11, 0)))
        .unwrap();
    assert_eq!(
        change.roles,
        [ObjectRole::WidgetAppearance, ObjectRole::PageContent]
    );
    assert_eq!(change.doc_mdp, ChangeDecision::Disallowed);
}

#[test]
fn a_failed_snapshot_releases_its_own_cache_reservations() {
    let probe = open("form-fill-p2");
    let initial = probe.retained.used();
    let historical = probe.revision_document(1).unwrap();
    let parsed: usize = historical
        .slots
        .values()
        .filter_map(|slot| slot.object.get())
        .filter_map(|o| o.as_ref().ok())
        .map(crate::IndirectObject::retained_bytes)
        .sum();
    let metadata = historical.retained.used() - parsed;
    let history_slots = probe.revisions().len() * size_of::<crate::history::Snapshot>();
    let doc = Document::open(
        fixture("signature-changes", "form-fill-p2.pdf"),
        &OpenOptions {
            limits: Limits {
                document_bytes: initial + history_slots + metadata,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let error = doc.object_at_revision(1, ObjectId::new(7, 0)).unwrap_err();
    assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit);
    assert_eq!(doc.retained.used(), initial + history_slots);
    assert_eq!(
        doc.object_at_revision(1, ObjectId::new(7, 0)).unwrap_err(),
        error
    );
    assert_eq!(doc.retained.used(), initial + history_slots);
}

#[test]
fn malformed_signature_sources_are_not_deduplicated_by_missing_byte_ranges() {
    let doc = open("replaced-malformed-signatures");
    let signatures = doc.signatures().unwrap();
    assert!(signatures.history_complete);
    assert_eq!(signatures.historical.len(), 1);
    let historical = &signatures.historical[0];
    let expected = std::fs::read_to_string(fixture_path(
        "signature-changes",
        "replaced-malformed-signatures.history",
    ))
    .unwrap();
    assert_eq!(
        format!(
            "{}|{}|{}|{:?}\n",
            historical.revision,
            historical.field_name.as_deref().unwrap(),
            historical.signature.object.unwrap().number,
            historical.signature.results.permitted_modifications
        ),
        expected
    );
    assert_eq!(
        signatures.fields[0]
            .signature
            .as_ref()
            .unwrap()
            .results
            .permitted_modifications,
        PermittedModifications::NotAnalysedYet
    );
}

#[test]
fn exhausted_change_work_preserves_structural_results() {
    let mut doc = open("object-stream-redefined");
    // Populate source caches so this exercises traversal work, not cache limits.
    let good = doc.signatures().unwrap();
    assert!(matches!(
        &good.fields[0]
            .signature
            .as_ref()
            .unwrap()
            .results
            .permitted_modifications,
        PermittedModifications::Analysed(ModificationAnalysis {
            outcome: ModificationOutcome::Allowed,
            ..
        })
    ));
    drop(good);
    // Source inspection is small; walking all revision roles exceeds this cap.
    doc.limits.traversal_work = 300;
    let limited = doc.signatures().unwrap();
    assert!(limited.history_complete);
    let signature = limited.fields[0].signature.as_ref().unwrap();
    assert_eq!(
        signature.results.integrity,
        Integrity::ByteRangeCheckedCryptographyNotChecked
    );
    assert!(matches!(
        signature.results.revision_coverage,
        RevisionCoverage::Covered { index: 2, .. }
    ));
    assert!(signature.byte_range.is_some());
    assert_eq!(
        signature.results.permitted_modifications,
        PermittedModifications::Inconclusive
    );
}

#[test]
fn change_candidates_include_only_xref_deltas_and_stream_dependencies() {
    for (name, expected) in [
        ("form-fill-p2", vec![7]),
        ("indirect-length-only", vec![12, 13]),
        ("object-stream-redefined", vec![7, 40, 52]),
    ] {
        let doc = open(name);
        let mut ctx = crate::view::Context::new(&doc);
        let mut signatures = read::read(&doc, &ctx).unwrap();
        let facts = history::discover(&doc, &mut ctx, &mut signatures).unwrap();
        let old_doc = doc.revision_with_context(1, &mut ctx).unwrap();
        let old = roles::read(old_doc, &mut ctx, facts[1].as_ref().unwrap()).unwrap();
        let new = roles::read(&doc, &mut ctx, facts[0].as_ref().unwrap()).unwrap();
        let actual: Vec<_> = deltas::read(&mut ctx, (old_doc, &old), (&doc, &new))
            .unwrap()
            .into_iter()
            .map(|id| id.number)
            .collect();
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn large_revision_roles_are_shared_and_all_signatures_survive_limits() {
    let mut doc = Document::open(test_pdf::large(10_000, 4), &OpenOptions::default()).unwrap();
    let good = doc.signatures().unwrap();
    assert!(good.history_complete);
    assert_eq!(
        good.fields.iter().filter(|f| f.signature.is_some()).count(),
        5
    );
    for (index, signature) in good
        .fields
        .iter()
        .filter_map(|f| f.signature.as_ref())
        .enumerate()
    {
        assert_eq!(
            signature.results.integrity,
            Integrity::ByteRangeCheckedCryptographyNotChecked
        );
        assert!(matches!(&signature.results.permitted_modifications,
            PermittedModifications::Analysed(analysis) if analysis.outcome == if index == 4 {
                ModificationOutcome::NoChanges
            } else { ModificationOutcome::Allowed }));
    }
    drop(good);
    doc.limits.traversal_work = 150_000;
    let limited = doc.signatures().unwrap();
    assert!(limited.history_complete);
    assert_eq!(
        limited
            .fields
            .iter()
            .filter(|f| f.signature.is_some())
            .count(),
        5
    );
    for (index, signature) in limited
        .fields
        .iter()
        .filter_map(|f| f.signature.as_ref())
        .enumerate()
    {
        assert_eq!(
            signature.results.integrity,
            Integrity::ByteRangeCheckedCryptographyNotChecked
        );
        assert!(matches!(
            signature.results.revision_coverage,
            RevisionCoverage::Covered { .. }
        ));
        if index < 4 {
            assert_eq!(
                signature.results.permitted_modifications,
                PermittedModifications::Inconclusive
            );
        } else {
            assert!(matches!(
                &signature.results.permitted_modifications,
                PermittedModifications::Analysed(ModificationAnalysis {
                    outcome: ModificationOutcome::NoChanges,
                    ..
                })
            ));
        }
    }
}

#[test]
fn a_large_form_fill_is_allowed_with_default_limits() {
    let doc = Document::open(test_pdf::large(60_000, 0), &OpenOptions::default()).unwrap();
    let signatures = doc.signatures().unwrap();
    assert!(signatures.history_complete);
    let signature = signatures
        .fields
        .iter()
        .find_map(|f| f.signature.as_ref())
        .unwrap();
    assert_eq!(
        signature.results.integrity,
        Integrity::ByteRangeCheckedCryptographyNotChecked
    );
    let PermittedModifications::Analysed(analysis) = &signature.results.permitted_modifications
    else {
        panic!("form fill must be classified");
    };
    assert_eq!(analysis.outcome, ModificationOutcome::Allowed);
    assert_eq!(analysis.revisions.len(), 1);
    assert_eq!(analysis.revisions[0].changes.len(), 1);
    assert_eq!(
        analysis.revisions[0].changes[0].rule,
        ModificationRule::FormFill
    );
}

#[test]
fn snapshot_work_limits_do_not_poison_a_later_analysis() {
    let doc = open("form-fill-p2");
    let mut spent = crate::view::Context::new(&doc);
    spent
        .allocate::<u8>(doc.limits().document_bytes, crate::view::Source::default())
        .unwrap();
    assert_eq!(
        doc.revision_with_context(1, &mut spent)
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
    let value = doc.object_at_revision(1, ObjectId::new(7, 0)).unwrap();
    assert_eq!(
        value.dictionary().unwrap().get(b"V").unwrap().as_string(),
        Some(b"old".as_slice())
    );
    let signatures = doc.signatures().unwrap();
    assert!(signatures.history_complete);
    assert!(
        matches!(&signatures.fields[0].signature.as_ref().unwrap().results.permitted_modifications,
        PermittedModifications::Analysed(analysis) if analysis.outcome == ModificationOutcome::Allowed)
    );
}

#[test]
fn linearized_sections_form_one_source_revision() {
    for name in [
        "linearized-base",
        "linearized-base-early-eof",
        "linearized-base-main-id",
    ] {
        let doc = open(name);
        assert_eq!(doc.revisions().len(), 3);
        let base = doc.revision_document(2).unwrap();
        assert_eq!(base.revisions().len(), 1);
        assert!(base.revisions()[0].offset() < base.revisions()[0].end_section);
        assert!(base.catalog().unwrap().get(b"AcroForm").is_none());
        assert_eq!(
            base.signatures().unwrap().fields,
            Vec::<SignatureField<'_>>::new()
        );
        assert_eq!(base.trailer().get(b"ID"), doc.trailer().get(b"ID"));
        let signatures = doc.signatures().unwrap();
        assert!(signatures.history_complete);
        let signature = signatures.fields[0].signature.as_ref().unwrap();
        assert!(matches!(
            signature.results.revision_coverage,
            RevisionCoverage::Covered { index: 1, .. }
        ));
    }
}

#[test]
fn unrelated_invalid_slots_stay_unparsed_during_change_analysis() {
    let doc = open("unused-invalid-slot");
    let signatures = doc.signatures().unwrap();
    let signature = signatures.fields[0].signature.as_ref().unwrap();
    assert!(matches!(&signature.results.permitted_modifications,
        PermittedModifications::Analysed(analysis) if analysis.outcome == ModificationOutcome::Allowed));
    for revision in 0..doc.revisions().len() {
        let snapshot = doc.revision_document(revision).unwrap();
        assert!(snapshot.slots[&ObjectId::new(99, 0)].object.get().is_none());
    }
}

#[test]
fn snapshot_metadata_admission_limits_can_be_retried() {
    let doc = open("form-fill-p2");
    let mut temporary = doc.retained.reservation();
    temporary
        .charge(
            doc.limits().document_bytes - doc.retained.used(),
            ObjectId::new(0, 0),
        )
        .unwrap();
    assert_eq!(
        doc.object_at_revision(1, ObjectId::new(7, 0))
            .unwrap_err()
            .as_error()
            .unwrap()
            .kind,
        ErrorKind::Limit
    );
    drop(temporary);
    let object = doc.object_at_revision(1, ObjectId::new(7, 0)).unwrap();
    assert_eq!(
        object.dictionary().unwrap().get(b"V").unwrap().as_string(),
        Some(b"old".as_slice())
    );
}
