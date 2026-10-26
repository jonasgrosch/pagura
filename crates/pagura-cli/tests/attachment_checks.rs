#![forbid(unsafe_code)]
//! CLI reports completed parameter checks separately from operation failures.

use pagura_testkit::{Json, fixture_path};
use std::process::Command;

#[test]
fn attachment_check_json_and_exit_status_preserve_missing_and_mismatch() {
    for (name, expected) in [
        ("check-flate.pdf", ["match", "match"]),
        ("check-size-mismatch.pdf", ["mismatch", "match"]),
        ("check-checksum-mismatch.pdf", ["match", "mismatch"]),
        ("check-both-mismatch.pdf", ["mismatch", "mismatch"]),
        ("check-size-only.pdf", ["match", "not-declared"]),
        ("check-checksum-only.pdf", ["not-declared", "match"]),
        ("check-empty-params.pdf", ["not-declared", "not-declared"]),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pagura"))
            .args(["check-attachment", "--strict"])
            .arg(fixture_path("files", name))
            .args(["0", "UF"])
            .output()
            .unwrap();
        let mismatch = expected.contains(&"mismatch");
        assert_eq!(output.status.code(), Some(i32::from(mismatch)), "{name}");
        let report = Json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
        assert_eq!(report.get("schema").and_then(Json::as_u64), Some(1));
        assert_eq!(report.get("index").and_then(Json::as_u64), Some(0));
        assert_eq!(report.get("key").and_then(Json::as_str), Some("UF"));
        assert_eq!(report.get("decoded_size").and_then(Json::as_u64), Some(18));
        for (key, expected) in ["size", "checksum"].iter().zip(expected) {
            assert_eq!(
                report.get(key).and_then(Json::as_str),
                Some(expected),
                "{name} {key}"
            );
        }
        if mismatch {
            assert!(String::from_utf8_lossy(&output.stderr).contains("declarations do not match"));
        } else {
            assert!(
                output.stderr.is_empty(),
                "matching/absent declarations are not errors"
            );
        }
    }
}

#[test]
fn attachment_check_refusals_have_no_success_report_or_extracted_bytes() {
    for (name, selection) in [
        ("populated.pdf", ["99", "UF"]),
        ("populated.pdf", ["0", "Unix"]),
        ("external-stream.pdf", ["0", "F"]),
        ("encrypted-no-type.pdf", ["0", "F"]),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pagura"))
            .arg("check-attachment")
            .arg(fixture_path("files", name))
            .args(selection)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(
            output.stdout.is_empty(),
            "{name}: failure must not emit a successful check"
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_pagura"))
        .args(["check-attachment", "--password", "attachment"])
        .arg(fixture_path("files", "encrypted-no-type.pdf"))
        .args(["0", "F"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report = Json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(report.get("decoded_size").and_then(Json::as_u64), Some(19));
    assert_eq!(
        report.get("checksum").and_then(Json::as_str),
        Some("not-declared")
    );
}
