#![forbid(unsafe_code)]
//! The new interpreter against the old renderer's compiler, as the
//! behavioural reference: same success or failure (error kind, tag, object
//! and position), same diagnostics, same marks and glyph placements, and
//! the same positioned text, over every generated fixture and the
//! benchmark pages.

mod compare;
mod decoding;

use compare::Outcome;
use pagura_core::Recovery;
use std::path::{Path, PathBuf};

/// Pages compared per document at most.
const PAGES: usize = 4;

fn pdfs(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            pdfs(&path, out);
        } else if path.extension().is_some_and(|e| e == "pdf") {
            out.push(path);
        }
    }
}

fn report(outcome: &Outcome, what: &str) {
    eprintln!(
        "{what}: {} matched, {} pending, {} intended, {} mismatched",
        outcome.matched,
        outcome.pending.len(),
        outcome.intended.len(),
        outcome.mismatches.len()
    );
    for line in &outcome.intended {
        eprintln!("  intended {line}");
    }
    for line in outcome.pending.iter().take(20) {
        eprintln!("  pending {line}");
    }
    for line in &outcome.mismatches {
        eprintln!("  MISMATCH {line}");
    }
}

fn page_count(bytes: &[u8]) -> usize {
    pagura_document::Document::open(bytes.to_vec(), &pagura_document::OpenOptions::default())
        .ok()
        .and_then(|d| d.page_count().ok())
        .unwrap_or(0)
}

#[test]
fn generated_fixtures_match_the_old_interpreter() {
    let mut files = Vec::new();
    pdfs(
        &pagura_testkit::workspace_root().join("corpus/generated"),
        &mut files,
    );
    assert!(files.len() > 1000, "{} fixtures", files.len());
    let filter = std::env::var("PAGURA_DIFFERENTIAL_FILTER").unwrap_or_default();
    let mut outcome = Outcome::default();
    for path in files {
        let name = path
            .strip_prefix(pagura_testkit::workspace_root().join("corpus/generated"))
            .unwrap_or(&path)
            .display()
            .to_string();
        if !name.contains(&filter) {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        for index in 0..page_count(&bytes).min(PAGES) {
            for recovery in [Recovery::Repair, Recovery::Strict] {
                compare::page(&name, &bytes, index, recovery, &mut outcome);
            }
        }
    }
    report(&outcome, "generated fixtures");
    assert!(
        outcome.mismatches.is_empty(),
        "{} mismatches",
        outcome.mismatches.len()
    );
}

#[test]
fn filled_forms_match_the_old_interpreter() {
    let mut files = Vec::new();
    pdfs(
        &pagura_testkit::workspace_root().join("corpus/generated/forms"),
        &mut files,
    );
    let mut outcome = Outcome::default();
    for path in files {
        let name = path.display().to_string();
        let bytes = std::fs::read(&path).unwrap();
        for index in 0..page_count(&bytes).min(PAGES) {
            compare::filled_page(&name, &bytes, index, &mut outcome);
            compare::appearances(&name, &bytes, index, &mut outcome);
        }
    }
    report(&outcome, "filled forms and appearance text");
    assert!(
        outcome.mismatches.is_empty(),
        "{} mismatches",
        outcome.mismatches.len()
    );
}

#[test]
fn benchmark_pages_match_the_old_interpreter() {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let corpora = home.join(".cache/pagura-corpora");
    let list = home.join(".cache/pagura-coordination/fleet/bench-pages.json");
    let (true, Ok(text)) = (corpora.is_dir(), std::fs::read_to_string(&list)) else {
        eprintln!("benchmark corpus absent; skipped");
        return;
    };
    let pages = pagura_testkit::Json::parse(&text).unwrap();
    let mut outcome = Outcome::default();
    for entry in pages.as_array().unwrap() {
        let file = entry
            .get("file")
            .and_then(pagura_testkit::Json::as_str)
            .unwrap();
        let index = usize::try_from(
            entry
                .get("page")
                .and_then(pagura_testkit::Json::as_u64)
                .unwrap(),
        )
        .unwrap();
        let Ok(bytes) = std::fs::read(corpora.join(file)) else {
            continue;
        };
        compare::page(file, &bytes, index, Recovery::Repair, &mut outcome);
    }
    report(&outcome, "benchmark pages");
    assert!(
        outcome.mismatches.is_empty(),
        "{} mismatches",
        outcome.mismatches.len()
    );
}

/// Real documents: every PDF under `PAGURA_DIFFERENTIAL_CORPUS`, at most
/// `PAGURA_DIFFERENTIAL_LIMIT` of them (default 200), first pages only.
#[test]
#[ignore = "needs a corpus directory; run with --ignored"]
fn corpus_pages_match_the_old_interpreter() {
    let Some(directory) = std::env::var_os("PAGURA_DIFFERENTIAL_CORPUS") else {
        eprintln!("PAGURA_DIFFERENTIAL_CORPUS unset; skipped");
        return;
    };
    let limit = std::env::var("PAGURA_DIFFERENTIAL_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let mut files = Vec::new();
    pdfs(Path::new(&directory), &mut files);
    let mut outcome = Outcome::default();
    for path in files.into_iter().take(limit) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let name = path.display().to_string();
        for index in 0..page_count(&bytes).min(2) {
            compare::page(&name, &bytes, index, Recovery::Repair, &mut outcome);
        }
    }
    report(&outcome, "corpus pages");
    assert!(
        outcome.mismatches.is_empty(),
        "{} mismatches",
        outcome.mismatches.len()
    );
}
