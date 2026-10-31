//! Complete appearances, link identity and fail-closed signed null recovery.
use crate::support::ppm;
use pagura::signatures::PermittedModifications;
use pagura::{
    Document, ErrorKind, Limits, ObjectId, OpenOptions, Position, Recovery, RenderOptions,
};
use pagura_testkit::{assert_bytes_eq, fixture};

#[test]
fn literal_null_members_keep_pixels_order_links_and_original_bytes() {
    for name in [
        "clean",
        "empty",
        "leading",
        "middle",
        "trailing",
        "repeated",
        "null-only",
        "reverse-order",
        "indirect-array",
        "many-nulls",
    ] {
        let source = fixture("recovery_annotations", &format!("{name}.pdf"));
        let document = Document::open(source.clone(), &OpenOptions::default()).unwrap();
        let page = document.page(0).unwrap();
        let pixmap = pagura::render_page(&document, page, &RenderOptions::from_dpi(72.0)).unwrap();
        assert_bytes_eq(
            &ppm(&pixmap),
            &fixture("recovery_annotations", &format!("{name}.ppm")),
            name,
        );
        let links = page.links(&document).unwrap();
        assert_eq!(
            links.len(),
            usize::from(!["empty", "null-only"].contains(&name)),
            "{name}"
        );
        if let Some(link) = links.first() {
            let original = document
                .object(ObjectId::new(6, 0))
                .unwrap()
                .dictionary()
                .unwrap();
            assert!(std::ptr::eq(link.dictionary, original), "{name}");
        }
        assert_eq!(document.original_bytes(), source.as_slice());
        let diagnostics = document.diagnostics();
        let repaired = !["clean", "empty"].contains(&name);
        assert_eq!(diagnostics.len(), usize::from(repaired), "{name}");
        if repaired {
            assert_eq!(diagnostics[0].tag(), "pages.annotation-null");
            assert_eq!(diagnostics[0].object, Some(ObjectId::new(3, 0)));
            assert_eq!(diagnostics[0].position, Position::Unknown);
        }
    }
}

#[test]
fn strict_null_rejection_has_page_context_and_no_applied_repair() {
    for name in [
        "leading",
        "middle",
        "trailing",
        "repeated",
        "null-only",
        "indirect-array",
        "many-nulls",
    ] {
        let source = fixture("recovery_annotations", &format!("{name}.pdf"));
        let document = Document::open(
            source.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..Default::default()
            },
        )
        .unwrap();
        let page = document.page(0).unwrap();
        for error in [
            pagura::render_page(&document, page, &RenderOptions::from_dpi(72.0)).unwrap_err(),
            page.links(&document).unwrap_err(),
        ] {
            assert_eq!(error.kind, ErrorKind::Syntax);
            assert_eq!(
                error.repair.map(pagura::Repair::as_str),
                Some("pages.annotation-null")
            );
            assert_eq!(error.object, Some(ObjectId::new(3, 0)));
            assert_eq!(error.position, Position::Unknown);
        }
        assert_eq!(document.diagnostics(), []);
        assert_eq!(document.original_bytes(), source.as_slice());
    }
}

#[test]
fn referenced_nulls_and_other_members_still_fail_both_consumers() {
    for name in [
        "indirect-null",
        "undefined-reference",
        "wrong-generation",
        "boolean",
        "number",
        "string",
        "nested-array",
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("recovery_annotations", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..Default::default()
                },
            )
            .unwrap();
            let page = document.page(0).unwrap();
            assert!(
                pagura::render_page(&document, page, &RenderOptions::from_dpi(72.0)).is_err(),
                "{name}"
            );
            assert!(page.links(&document).is_err(), "{name}");
            assert_eq!(document.diagnostics(), []);
        }
    }
    for (name, kind) in [
        ("after-null-boolean", ErrorKind::Syntax),
        ("after-null-bbox", ErrorKind::Syntax),
        ("after-null-cycle", ErrorKind::Cycle),
        ("after-null-subtype", ErrorKind::Syntax),
    ] {
        let document = Document::open(
            fixture("recovery_annotations", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let page = document.page(0).unwrap();
        let error =
            pagura::render_page(&document, page, &RenderOptions::from_dpi(72.0)).unwrap_err();
        assert_eq!(error.kind, kind, "{name}");
        assert!(
            document
                .diagnostics()
                .iter()
                .any(|d| d.tag() == "pages.annotation-null")
        );
    }
}

#[test]
fn nulls_consume_original_annotation_budgets() {
    let document = Document::open(
        fixture("recovery_annotations", "many-nulls.pdf"),
        &OpenOptions {
            limits: Limits {
                traversal_work: 16,
                ..Limits::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let page = document.page(0).unwrap();
    assert_eq!(
        pagura::render_page(&document, page, &RenderOptions::from_dpi(72.0))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert_eq!(document.diagnostics(), []);
    assert_eq!(page.links(&document).unwrap_err().kind, ErrorKind::Limit);
}

#[test]
fn recovered_nulls_never_authorize_signed_mutations() {
    for location in ["clean", "signed-null", "added-null"] {
        for operation in ["annotation", "content", "signing"] {
            let name = format!("signature-{location}-{operation}");
            let source = fixture("recovery_annotations", &format!("{name}.pdf"));
            let document = Document::open(source.clone(), &OpenOptions::default()).unwrap();
            let signatures = document.signatures().unwrap();
            let signature = signatures
                .fields
                .iter()
                .find(|f| f.qualified_name == "Approval")
                .and_then(|f| f.signature.as_ref())
                .unwrap();
            let actual = match &signature.results.permitted_modifications {
                PermittedModifications::Analysed(analysis) => format!("{:?}\n", analysis.outcome),
                PermittedModifications::Inconclusive => "Inconclusive\n".to_owned(),
                value => panic!("unexpected policy result {name}: {value:?}"),
            };
            assert_eq!(
                actual.as_bytes(),
                fixture("recovery_annotations", &format!("{name}.outcome")),
                "{name}"
            );
            if location != "clean" {
                assert_eq!(
                    signature.results.permitted_modifications,
                    PermittedModifications::Inconclusive,
                    "{name}"
                );
            }
            assert_eq!(document.original_bytes(), source.as_slice());
        }
    }
}
