//! Fixture discovery shared by the differential tests.

use pagura_core::Recovery;
use pagura_document::{Document, OpenOptions};
use std::path::{Path, PathBuf};

/// Every generated PDF fixture, in a stable order.
pub(crate) fn corpus() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect(
        &pagura_testkit::workspace_root().join("corpus/generated"),
        &mut paths,
    );
    paths.sort();
    assert!(paths.len() > 1000, "generated fixtures are missing");
    paths
}

#[expect(clippy::unwrap_used, reason = "an unreadable corpus fails the test")]
fn collect(directory: &Path, paths: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, paths);
        } else if path.extension().is_some_and(|e| e == "pdf") {
            paths.push(path);
        }
    }
}

/// Opens `bytes` in `recovery` mode, or `None` when the fixture does not
/// open (an expected failure of a recovery or document fixture).
pub(crate) fn open(bytes: &[u8], recovery: Recovery) -> Option<Document> {
    open_with(
        bytes,
        &OpenOptions {
            recovery,
            ..Default::default()
        },
    )
}

/// [`open`] with explicit options.
pub(crate) fn open_with(bytes: &[u8], options: &OpenOptions) -> Option<Document> {
    Document::open(bytes.to_vec(), options).ok()
}
