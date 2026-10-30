#![forbid(unsafe_code)]
//! Reported state recovery cannot bypass syntax, scope or resource limits.
use pagura_core::{ErrorKind, Limits, Position, Recovery, Result};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::Interpreter;
use pagura_testkit::fixture;

fn document(content: &str, options: &OpenOptions) -> Result<Document> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 16 16] /Resources << >> /Contents 4 0 R >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).bytes());
    }
    let xref = bytes.len();
    bytes.extend(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    bytes.extend(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").bytes());
    Document::open(bytes, options)
}

#[test]
fn every_state_repair_has_a_positioned_strict_refusal() {
    for (name, tag) in [
        ("unmatched-Q-prefix", "content.graphics-unmatched"),
        ("unmatched-ET-prefix", "content.text-unmatched"),
    ] {
        let bytes = fixture("recovery_state", &format!("{name}.pdf"));
        let strict = Document::open(
            bytes.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..Default::default()
            },
        )
        .unwrap();
        let error = Interpreter::new(&strict)
            .scene(strict.page(0).unwrap())
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
        assert_eq!(error.repair.unwrap().as_str(), tag, "{name}");
        assert!(matches!(error.position, Position::Stream { .. }), "{name}");
        assert!(strict.diagnostics().is_empty(), "{name}");
        let repaired = Document::open(bytes, &OpenOptions::default()).unwrap();
        Interpreter::new(&repaired)
            .scene(repaired.page(0).unwrap())
            .unwrap();
        assert!(
            repaired.diagnostics().iter().any(|d| d.tag() == tag),
            "{name}"
        );
    }
}

#[test]
fn syntax_and_crossed_state_boundaries_remain_errors() {
    for content in [
        "1 2",
        "BX",
        "notAnOperator",
        "BT BT",
        "BT",
        "1 1 m",
        "W",
        "BT q ET",
        "q BT Q",
        "[1",
        "1 1 m impossible",
    ] {
        let document = document(content, &OpenOptions::default()).unwrap();
        let error = Interpreter::new(&document)
            .scene(document.page(0).unwrap())
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{content}");
    }
}

#[test]
fn unmatched_closures_are_charged_to_the_operation_budget() {
    let document = document(
        "Q ET Q ET",
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
}
