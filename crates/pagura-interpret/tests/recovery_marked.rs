#![forbid(unsafe_code)]
//! Unmatched metadata endings retain scope and cannot produce partial pages.

use pagura_core::{ErrorKind, Limits, ObjectId, Position, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::Interpreter;
use pagura_testkit::fixture;

fn document(name: &str, options: &OpenOptions) -> pagura_core::Result<Document> {
    Document::open(fixture("recovery_marked", &format!("{name}.pdf")), options)
}

#[test]
fn strict_rejects_the_positioned_ending_without_reporting_applied_repairs() {
    let document = document(
        "unmatched-prefix",
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let error = Interpreter::new(&document)
        .scene(document.page(0).unwrap())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.repair.unwrap().as_str(), "content.marked-unmatched");
    assert_eq!(error.object, Some(ObjectId::new(4, 0)));
    assert!(matches!(error.position, Position::Stream { offset: 0, .. }));
    assert_eq!(document.diagnostics().len(), 0);
}

#[test]
fn repaired_endings_do_not_emit_unmatched_structure_events() {
    let document = document("actual-text", &OpenOptions::default()).unwrap();
    let text = Interpreter::new(&document)
        .text(document.page(0).unwrap())
        .unwrap();
    assert_eq!(text.glyphs.len(), 1);
    assert_eq!(text.glyphs[0].unicode.as_deref(), Some("A"));
    assert_eq!(text.marked.len(), 1);
    assert_eq!(text.marked[0].glyphs, 0..1);
    assert_eq!(text.marked[0].content.mcid, Some(7));
    assert_eq!(
        text.marked[0].content.actual_text.as_deref(),
        Some("replacement")
    );
    let diagnostics = document.diagnostics();
    assert_eq!(diagnostics.len(), 2);
    assert!(
        diagnostics
            .iter()
            .all(|d| d.tag() == "content.marked-unmatched")
    );
    assert_ne!(diagnostics[0].position, diagnostics[1].position);
}

#[test]
fn a_form_ending_cannot_close_its_callers_marked_content() {
    let document = document("form-keeps-caller-marked", &OpenOptions::default()).unwrap();
    let text = Interpreter::new(&document)
        .text(document.page(0).unwrap())
        .unwrap();
    assert_eq!(text.glyphs.len(), 1);
    assert_eq!(text.objects.len(), 1);
    assert_eq!(text.objects[0].object, ObjectId::new(7, 0));
    assert_eq!(text.marked.len(), 1);
    assert_eq!(text.marked[0].glyphs, 0..1);
    assert_eq!(
        text.marked[0].content.actual_text.as_deref(),
        Some("replacement")
    );
    assert_eq!(document.diagnostics().len(), 1);
    assert_eq!(document.diagnostics()[0].object, Some(ObjectId::new(7, 0)));
}

#[test]
fn a_hidden_form_keeps_inherited_visibility_and_balanced_structure() {
    let document = document("hidden-form-keeps-visibility", &OpenOptions::default()).unwrap();
    let text = Interpreter::new(&document)
        .text(document.page(0).unwrap())
        .unwrap();
    assert!(text.glyphs.is_empty());
    assert_eq!(text.marked.len(), 1);
    assert!(text.marked[0].hidden);
    assert_eq!(text.objects.len(), 1);
    assert!(text.objects[0].hidden);
    assert_eq!(document.diagnostics().len(), 1);
}

#[test]
fn unmatched_endings_still_consume_the_operation_budget() {
    let document = document(
        "repeated-before-paint",
        &OpenOptions {
            limits: Limits {
                operations: 2,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let error = Interpreter::new(&document)
        .scene(document.page(0).unwrap())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(document.diagnostics().len(), 2);
}

#[test]
fn later_incomplete_state_and_undefined_resources_remain_errors() {
    for name in [
        "unfinished-text",
        "unfinished-path",
        "unfinished-clip",
        "unfinished-operands",
        "unfinished-compatibility",
        "unknown-operator",
        "undefined-resource",
    ] {
        let document = document(name, &OpenOptions::default()).unwrap();
        let error = Interpreter::new(&document)
            .scene(document.page(0).unwrap())
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
        assert_eq!(document.diagnostics().len(), 1, "{name}");
        assert_eq!(
            document.diagnostics()[0].tag(),
            "content.marked-unmatched",
            "{name}"
        );
    }
}

#[test]
fn required_recursive_content_remains_an_error_after_the_reported_ending() {
    let document = document("required-cycle", &OpenOptions::default()).unwrap();
    let error = Interpreter::new(&document)
        .scene(document.page(0).unwrap())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Cycle);
    assert_eq!(error.object, Some(ObjectId::new(5, 0)));
    assert_eq!(error.repair, None);
    let diagnostics = document.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].tag(), "content.marked-unmatched");
    assert_eq!(diagnostics[0].object, Some(ObjectId::new(4, 0)));
    assert!(matches!(
        diagnostics[0].position,
        Position::Stream { offset: 22, .. }
    ));
}
