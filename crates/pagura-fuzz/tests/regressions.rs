#![forbid(unsafe_code)]
//! Replays every stored fuzz finding on stable Rust.
//!
//! `fuzz/regressions/<target>/<name>` holds inputs that once made a fuzz
//! target fail, minimized, each with a one-line note in `<name>.txt`. They
//! run through the drivers the libFuzzer targets use (this crate), so each
//! input must again reach the end: every API returns a value that keeps its
//! documented contract or a structured error, and nothing panics.
//! [`KNOWN_FAILURES`] lists the stored inputs whose fix has not landed yet;
//! an entry that passes fails this test, so that it is removed. The reading
//! drivers also run on the generated fixtures their seeds come from, which
//! they must pass: a driver that fails on valid input hides its findings.

use pagura_fuzz as harness;

use harness::host::{Flags, Input, Search};
use harness::wrapper::SEPARATOR;
use pagura::{Document, OpenOptions};
use pagura_testkit::{Manifest, workspace_root};
use std::path::PathBuf;
use std::{fs, io, panic};

/// Stored inputs that still fail, as `target/name`, with the defect.
const KNOWN_FAILURES: &[(&str, &str)] = &[];

fn regressions() -> PathBuf {
    workspace_root().join("fuzz/regressions")
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
    let directory = workspace_root().join("corpus/generated").join(area);
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
    // The host's reading requests in every protocol version that has them,
    // and refused in older ones: page 0, scale 1, an 8 x 8 tile, a one-page
    // search for `a` with at most four matches.
    let reading = Flags::READING | Flags::GENERATIONS;
    let versions = [
        reading,
        Flags::READING | Flags::OLDER,
        reading | Flags::TYPED_DIAGNOSTICS,
        reading | Flags::TYPED_DIAGNOSTICS | Flags::UNNEGOTIATED_DIAGNOSTICS,
        reading | Flags::UNNEGOTIATED_DIAGNOSTICS,
        reading | Flags::VERSION_1_3,
        reading | Flags::VERSION_1_3 | Flags::UNNEGOTIATED_DIAGNOSTICS,
    ];
    for pdf in navigation.iter().chain(&text) {
        for flags in versions {
            let input = Input {
                flags,
                page: 0,
                scale: 15,
                tile: [0, 0, 8, 8],
                search: Some(Search {
                    pages: 1,
                    offset: 0,
                    limit: 4,
                    options: 0,
                    query: b"a",
                }),
                pdf,
            };
            assert!(harness::run("host", &input.to_bytes()));
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
        let passed = matches!(
            panic::catch_unwind(|| harness::run(target, &data)),
            Ok(true)
        );
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

/// The document driver, which verifies signatures, on every signed fixture.
#[test]
fn document_driver_passes_on_signed_fixtures() {
    let signed: Vec<_> = ["signatures", "cms-signatures"]
        .into_iter()
        .flat_map(|area| generated(area, "").unwrap())
        .collect();
    assert!(signed.len() >= 10, "signed fixtures missing");
    for pdf in &signed {
        assert!(harness::run("document", pdf));
    }
}

/// The form driver on every generated form, with each kind of fill value
/// in both recovery modes.
#[test]
fn forms_driver_passes_on_generated_forms() {
    let forms = generated("forms", "").unwrap();
    assert!(forms.len() >= 20, "form fixtures missing");
    for pdf in &forms {
        for selector in [0, 0x81, 2, 0x83] {
            let mut input = vec![selector];
            input.extend_from_slice(pdf);
            assert!(harness::run("forms", &input));
        }
    }
}

/// The security driver on each security handler's fixture with a user
/// password (every encrypted fixture is a seed; debug builds make the
/// revision 6 key derivation too slow to replay them all here).
#[test]
fn security_driver_passes_on_encrypted_fixtures() {
    let crypto = Manifest::load("crypto");
    let mut encrypted = 0;
    for case in crypto.cases() {
        let (Some(password), Some(path)) = (case.string("password"), case.path()) else {
            continue;
        };
        if !case.name().ends_with("-password-user") {
            continue;
        }
        let mut input = vec![0];
        input.extend_from_slice(password.as_bytes());
        input.extend_from_slice(SEPARATOR);
        input.extend_from_slice(&fs::read(path).unwrap());
        assert!(harness::run("security", &input), "{}", case.id());
        encrypted += 1;
    }
    assert!(encrypted >= 9, "encrypted fixtures missing");
}

/// The CMap driver on every bundled predefined CMap.
#[test]
fn cmap_driver_passes_on_bundled_cmaps() {
    let mut cmaps = 0;
    for collection in fs::read_dir(workspace_root().join("data/cmaps")).unwrap() {
        let collection = collection.unwrap().path();
        if !collection.is_dir() {
            continue;
        }
        for file in fs::read_dir(collection).unwrap() {
            let mut input = fs::read(file.unwrap().path()).unwrap();
            input.extend_from_slice(SEPARATOR);
            input.extend_from_slice(SEPARATOR);
            input.extend_from_slice(b"\x00\x41\x81\x40\xff");
            assert!(harness::run("cmap", &input));
            cmaps += 1;
        }
    }
    assert!(cmaps >= 50, "bundled CMaps missing");
}

/// The writer round trip on objects of every kind, including a stream and
/// references between parts.
#[test]
fn write_driver_round_trips_every_value_kind() {
    let parts: [&[u8]; 4] = [
        b"<< /Type /Example /Numbers [0 -7 2.5 -0.125 123456.789] /Flags [true false null] \
          /Strings [(text) <00ff10> (\\(nested\\))] /Name /A#20B#23 /Next 4 0 R >>",
        b"<< /Filter /ASCIIHexDecode /Extra [3 0 R] >> stream\n48656c6c6f>",
        b"[[[1] [2]] << /Inner << /Deep 5 0 R >> >> ()]",
        b"42",
    ];
    assert!(harness::run("write", &parts.join(SEPARATOR)));
}
