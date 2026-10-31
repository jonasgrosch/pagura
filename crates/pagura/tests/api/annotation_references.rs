//! Immediate Null identities preserve complete pixels and fail-closed policy.
use pagura::editing::{SaveOptions, Transaction, features};
use pagura::signatures::PermittedModifications;
use pagura::{
    Document, ErrorKind, Limits, ObjectId, OpenOptions, Recovery, RenderContext, RenderOptions,
    Rgba8, Value,
};
use pagura_testkit::fixture;
use std::sync::Arc;

fn open(name: &str, recovery: Recovery, limits: Limits) -> Document {
    pagura_testkit::open_fixture(
        "recovery_annotation_references",
        &format!("{name}.pdf"),
        |bytes| {
            Document::open(
                bytes,
                &OpenOptions {
                    recovery,
                    limits,
                    ..OpenOptions::default()
                },
            )
        },
    )
}

fn pixels(scale: u32, background: Rgba8) -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in 0..32 * scale {
        let y = 32 * scale - 1 - row;
        for x in 0..32 * scale {
            let rgba = if (6 * scale..14 * scale).contains(&x)
                && (6 * scale..14 * scale).contains(&y)
            {
                [0, 255, 0, 255]
            } else if (2 * scale..10 * scale).contains(&x) && (2 * scale..10 * scale).contains(&y) {
                [255, 0, 0, 255]
            } else if (20 * scale..24 * scale).contains(&x) && (2 * scale..6 * scale).contains(&y) {
                [0, 0, 255, 255]
            } else if background == Rgba8::WHITE {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            };
            bytes.extend(rgba);
        }
    }
    bytes
}

#[test]
fn zero_reference_parser_repair_composes_without_diagnostic_authority() {
    let strict = Arc::new(open(
        "zero-reference-body",
        Recovery::Strict,
        Limits::default(),
    ));
    let context = RenderContext::new(Arc::clone(&strict));
    let first = context.scene(strict.page(0).unwrap()).unwrap_err();
    assert_eq!(
        format!("{first:?}"),
        concat!(
            "Error { kind: Syntax, message: \"object-zero reference interpreted as null\", ",
            "position: File(823), object: Some(ObjectId { number: 9, generation: 0 }), ",
            "repair: Some(Repair(\"syntax.reference-zero\")) }"
        )
    );
    for _ in 0..2 {
        assert_eq!(context.scene(strict.page(0).unwrap()).unwrap_err(), first);
    }
    assert_eq!(strict.diagnostics(), []);
    for saturated in [false, true] {
        let document = Arc::new(open(
            "zero-reference-body",
            Recovery::Repair,
            Limits::default(),
        ));
        if saturated {
            document.report((0..1024).map(|offset| {
                pagura::Diagnostic::new(pagura::Repair::new("test.capacity"), "bounded reports")
                    .at(pagura::Position::Buffer(offset))
            }));
        }
        let context = RenderContext::new(Arc::clone(&document));
        for scale in [1_u32, 2] {
            for background in [Rgba8::WHITE, Rgba8::TRANSPARENT] {
                let actual = context
                    .render_page(
                        document.page(0).unwrap(),
                        &RenderOptions {
                            background,
                            ..RenderOptions::from_dpi(f64::from(scale) * 72.0)
                        },
                    )
                    .unwrap();
                assert_eq!(actual.data(), pixels(scale, background));
            }
        }
        let links = document.page(0).unwrap().links(&document).unwrap();
        assert_eq!(links.len(), 1);
        assert!(std::ptr::eq(
            links[0].dictionary,
            document
                .object(ObjectId::new(6, 0))
                .unwrap()
                .dictionary()
                .unwrap()
        ));
        assert_eq!(
            document.object(ObjectId::new(9, 0)).unwrap().value(),
            &Value::Null
        );
        if saturated {
            assert_eq!(document.diagnostics().len(), 1024);
            assert!(document.suppressed_diagnostics() >= 2);
        } else {
            let tags: Vec<_> = document
                .diagnostics()
                .iter()
                .map(pagura::Diagnostic::tag)
                .collect();
            assert_eq!(
                tags,
                ["syntax.reference-zero", "pages.annotation-null-reference"]
            );
        }
        assert_eq!(
            document.original_bytes(),
            fixture("recovery_annotation_references", "zero-reference-body.pdf")
        );
    }
}

#[test]
fn full_rgba_links_and_later_first_use_keep_supplied_content() {
    for name in [
        "defined-zero",
        "defined-seven",
        "repeat-identity",
        "many-references",
        "compressed-null",
        "later-first-use",
    ] {
        let document = Arc::new(open(name, Recovery::Repair, Limits::default()));
        let context = RenderContext::new(Arc::clone(&document));
        let target = usize::from(name == "later-first-use");
        if target == 1 {
            let p = context
                .render_page(document.page(0).unwrap(), &RenderOptions::from_dpi(72.0))
                .unwrap();
            assert_eq!(p.data(), [0, 0, 255, 255].repeat(32 * 32));
        }
        for scale in [1_u32, 2] {
            for background in [Rgba8::WHITE, Rgba8::TRANSPARENT] {
                let options = RenderOptions {
                    background,
                    ..RenderOptions::from_dpi(f64::from(scale) * 72.0)
                };
                for _ in 0..2 {
                    let actual = context
                        .render_page(document.page(target).unwrap(), &options)
                        .unwrap();
                    assert_eq!(
                        (actual.width(), actual.height()),
                        (32 * scale, 32 * scale),
                        "{name}"
                    );
                    assert_eq!(actual.data(), pixels(scale, background), "{name}");
                }
            }
        }
        let links = document.page(target).unwrap().links(&document).unwrap();
        assert_eq!(links.len(), 1);
        assert!(std::ptr::eq(
            links[0].dictionary,
            document
                .object(ObjectId::new(6, 0))
                .unwrap()
                .dictionary()
                .unwrap()
        ));
        assert_eq!(
            document.original_bytes(),
            fixture("recovery_annotation_references", &format!("{name}.pdf"))
        );
        assert_eq!(document.diagnostics().len(), 1, "{name}");
        assert_eq!(
            document.diagnostics()[0].repair(),
            Some(pagura::PAGES_ANNOTATION_NULL_REFERENCE)
        );
    }
}

#[test]
fn strict_refusals_and_non_null_failures_do_not_produce_a_scene() {
    for name in [
        "defined-zero",
        "defined-seven",
        "compressed-null",
        "reference-chain",
        "reference-cycle",
        "undefined",
        "free",
        "wrong-generation",
        "integer",
        "string",
        "array",
        "later-invalid",
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            if recovery == Recovery::Repair
                && ["defined-zero", "defined-seven", "compressed-null"].contains(&name)
            {
                continue;
            }
            let context = RenderContext::new(Arc::new(open(name, recovery, Limits::default())));
            let doc = context.document();
            let first = context.scene(doc.page(0).unwrap()).unwrap_err();
            // Resources decoded before the failing member may remain cached;
            // failed scenes must not grow that retained prefix on retry.
            let retained = context.retained_bytes();
            for _ in 0..2 {
                assert_eq!(
                    context.scene(doc.page(0).unwrap()).unwrap_err(),
                    first,
                    "{name}"
                );
                assert_eq!(context.retained_bytes(), retained, "{name}");
            }
            if recovery == Recovery::Strict
                && ["defined-zero", "defined-seven", "compressed-null"].contains(&name)
            {
                assert_eq!(first.kind, ErrorKind::Syntax);
                assert_eq!(first.message, "expected a dictionary");
                assert_eq!(
                    first.object,
                    Some(ObjectId::new(
                        9,
                        if name == "defined-seven" { 7 } else { 0 }
                    ))
                );
                assert_eq!(first.position, pagura::Position::Unknown);
                assert_eq!(first.repair, None);
                assert_eq!(doc.diagnostics(), []);
            }
        }
    }
}

#[test]
fn raw_annotation_work_and_repaired_save_rules_remain_required() {
    let doc = open(
        "many-references",
        Recovery::Repair,
        Limits {
            traversal_work: 16,
            ..Limits::default()
        },
    );
    assert_eq!(
        pagura::render_page(&doc, doc.page(0).unwrap(), &RenderOptions::from_dpi(72.0))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    // Rendering checks the raw array count before visiting any members.
    assert_eq!(doc.diagnostics(), []);
    assert_eq!(
        doc.page(0).unwrap().links(&doc).unwrap_err().kind,
        ErrorKind::Limit
    );
    // Links meter each original member; a report discovered before the
    // later budget failure remains visible and is deduplicated by identity.
    assert_eq!(doc.diagnostics().len(), 1);
    assert_eq!(
        doc.diagnostics()[0].repair(),
        Some(pagura::PAGES_ANNOTATION_NULL_REFERENCE)
    );
    let doc = open("defined-zero", Recovery::Repair, Limits::default());
    doc.page(0).unwrap().links(&doc).unwrap();
    let mut transaction = Transaction::new(&doc);
    assert_eq!(
        transaction.commit(&SaveOptions::default()).unwrap(),
        doc.original_bytes()
    );
    transaction.add(Value::Null.into()).unwrap();
    assert_eq!(
        transaction
            .commit(&SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::REPAIRED)
    );
}

#[test]
fn defined_null_recovery_never_establishes_signed_edit_permission() {
    let mut names = Vec::new();
    for location in ["clean", "signed-null", "added-null"] {
        for operation in ["annotation", "content", "signing"] {
            names.push(format!("signature-{location}-{operation}"));
        }
    }
    names.extend([
        "signature-historical-null-current-valid".to_owned(),
        "signature-historical-valid-current-null".to_owned(),
    ]);
    for name in names {
        let doc = open(&name, Recovery::Repair, Limits::default());
        let signatures = doc.signatures().unwrap();
        let signature = signatures
            .fields
            .iter()
            .find(|f| f.qualified_name == "Approval")
            .and_then(|f| f.signature.as_ref())
            .unwrap();
        let result = &signature.results.permitted_modifications;
        let actual = match result {
            PermittedModifications::Analysed(a) => format!("{:?}\n", a.outcome),
            PermittedModifications::Inconclusive => "Inconclusive\n".to_owned(),
            other => panic!("unexpected policy {other:?}"),
        };
        assert_eq!(
            actual.as_bytes(),
            fixture("recovery_annotation_references", &format!("{name}.outcome")),
            "{name}"
        );
        if !name.contains("-clean-") {
            assert_eq!(result, &PermittedModifications::Inconclusive, "{name}");
        }
    }
}

#[test]
fn archived_indirect_null_input_changes_only_its_repair_contract() {
    let source = fixture("recovery_annotations", "indirect-null.pdf");
    let doc = Document::open(source.clone(), &OpenOptions::default()).unwrap();
    let pixmap =
        pagura::render_page(&doc, doc.page(0).unwrap(), &RenderOptions::from_dpi(72.0)).unwrap();
    assert_eq!(pixmap.data(), pixels(1, Rgba8::WHITE));
    assert_eq!(
        doc.diagnostics()[0].repair(),
        Some(pagura::PAGES_ANNOTATION_NULL_REFERENCE)
    );
    let strict = Document::open(
        source,
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let error = pagura::render_page(
        &strict,
        strict.page(0).unwrap(),
        &RenderOptions::from_dpi(72.0),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.message, "expected a dictionary");
    assert_eq!(error.object, Some(ObjectId::new(9, 0)));
    assert_eq!(error.position, pagura::Position::Unknown);
    assert_eq!(error.repair, None);
}
