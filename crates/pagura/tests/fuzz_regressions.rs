#![forbid(unsafe_code)]
//! Replays every stored fuzz finding on stable Rust.
//!
//! `fuzz/regressions/<target>/<name>` holds inputs that once made a fuzz
//! target fail, minimized, each with a one-line note in `<name>.txt`. They
//! run through the drivers the libFuzzer targets use (`fuzz/src/`), so each
//! input must again reach the end: every API returns a value that keeps its
//! documented contract or a structured error, and nothing panics.
//! [`KNOWN_FAILURES`] lists the stored inputs whose fix has not landed yet;
//! an entry that passes fails this test, so that it is removed. The reading
//! drivers also run on the generated fixtures their seeds come from, which
//! they must pass: a driver that fails on valid input hides its findings.

#[path = "../../../fuzz/src/lib.rs"]
mod harness;

use pagura::{Document, OpenOptions};
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

/// Generated PDFs of `area` whose names start with `prefix`, in name order.
fn generated(area: &str, prefix: &str) -> io::Result<Vec<Vec<u8>>> {
    let directory = pagura_testkit::workspace_root()
        .join("corpus/generated")
        .join(area);
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        if path.extension().is_some_and(|e| e == "pdf")
            && name.is_some_and(|n| n.starts_with(prefix))
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths.iter().map(fs::read).collect()
}

#[test]
fn reading_drivers_pass_on_generated_fixtures() {
    let navigation = generated("document", "navigation-").unwrap();
    let text = generated("text", "extract-").unwrap();
    assert!(
        !navigation.is_empty() && !text.is_empty(),
        "fixtures missing"
    );
    for pdf in &navigation {
        assert!(harness::run("navigation", pdf));
    }
    // The host's reading requests (flag bit 3) in protocol 1.2 with
    // generations, and in protocol 1 (bit 4): page 0, scale 1, an 8 x 8
    // tile, a one-page search for `a` with at most four matches.
    for pdf in navigation.iter().chain(&text) {
        for flags in [9, 24, 41, 105, 73, 137, 201] {
            let mut input = vec![flags, 0, 15, 0, 0, 8, 8, 1, 0, 4, 0, 1, b'a'];
            input.extend_from_slice(pdf);
            assert!(harness::run("host", &input));
        }
    }
    // The text fixtures' page content on the extraction page, searched for `a`.
    for pdf in &text {
        let document = Document::open(pdf.as_slice(), &OpenOptions::default()).unwrap();
        let page = document.page(0).unwrap();
        let mut content = Vec::new();
        for stream in document.content_streams(page).unwrap() {
            content.extend_from_slice(&document.stream_data(stream).unwrap());
            content.push(b'\n');
        }
        content.extend_from_slice(b"%a");
        assert!(harness::run("extraction", &content));
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
