#![forbid(unsafe_code)]
//! Replays every stored fuzz finding on stable Rust.
//!
//! `fuzz/regressions/<target>/<name>` holds inputs that once made a fuzz
//! target fail, minimized, each with a one-line note in `<name>.txt`. They
//! run through the drivers the libFuzzer targets use (`fuzz/src/`), so each
//! input must again reach the end: every API returns a value that keeps its
//! documented contract or a structured error, and nothing panics.
//! [`KNOWN_FAILURES`] lists the stored inputs whose fix has not landed yet;
//! an entry that passes fails this test, so that it is removed.

#[path = "../../../fuzz/src/lib.rs"]
mod harness;

use std::path::PathBuf;
use std::{fs, io, panic};

/// Stored inputs that still fail, as `target/name`, with the defect.
const KNOWN_FAILURES: &[(&str, &str)] = &[];

fn regressions() -> PathBuf {
    pagura_testkit::workspace_root().join("fuzz/regressions")
}

/// Every stored input as (`target/name`, bytes), in name order.
fn stored() -> io::Result<Vec<(String, Vec<u8>)>> {
    let mut inputs = Vec::new();
    for target in fs::read_dir(regressions())? {
        let target = target?.path();
        for input in fs::read_dir(&target)? {
            let path = input?.path();
            if path.extension().is_none() {
                let id = path.strip_prefix(regressions()).map_err(io::Error::other)?;
                let id = id.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
                inputs.push((id, fs::read(&path)?));
            }
        }
    }
    inputs.sort();
    Ok(inputs)
}

#[test]
fn stored_inputs_have_a_target_and_a_note() {
    let inputs = stored().unwrap();
    assert!(!inputs.is_empty(), "no stored fuzz inputs");
    for (id, _) in &inputs {
        let (target, _) = id.split_once('/').unwrap();
        assert!(
            harness::TARGETS.iter().any(|(name, _)| *name == target),
            "{id}: no fuzz target {target}"
        );
        let note = fs::read_to_string(regressions().join(format!("{id}.txt")))
            .unwrap_or_else(|error| panic!("{id}: note missing: {error}"));
        assert_eq!(note.trim().lines().count(), 1, "{id}: the note is one line");
    }
    for (id, defect) in KNOWN_FAILURES {
        assert!(
            inputs.iter().any(|(stored, _)| stored == id),
            "known failure {id} is not stored"
        );
        assert!(!defect.is_empty(), "known failure {id} names no defect");
    }
}

#[test]
fn stored_inputs_replay_without_failures() {
    let mut unexpected = Vec::new();
    let mut fixed = Vec::new();
    for (id, data) in stored().unwrap() {
        let (target, _) = id.split_once('/').unwrap();
        let passed = panic::catch_unwind(|| harness::run(target, &data)).is_ok();
        match (passed, KNOWN_FAILURES.iter().any(|(known, _)| *known == id)) {
            (false, false) => unexpected.push(id),
            (true, true) => fixed.push(id),
            _ => {}
        }
    }
    assert!(
        unexpected.is_empty(),
        "stored fuzz inputs fail again: {unexpected:?}"
    );
    assert!(
        fixed.is_empty(),
        "fixed; remove from KNOWN_FAILURES: {fixed:?}"
    );
}
