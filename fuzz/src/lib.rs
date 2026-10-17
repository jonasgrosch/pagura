#![forbid(unsafe_code)]
//! Drivers of Pagura's coverage-guided fuzz targets.
//!
//! Each driver feeds one input through public APIs and checks what every
//! input must keep: no panic, either a structured error or a result that
//! honours its documented contract, and work bounded by [`limits`]. The
//! libFuzzer entry points in `fuzz_targets/` are one line each; the stable
//! replay test `crates/pagura/tests/fuzz_regressions.rs` runs the stored
//! findings in `fuzz/regressions/` through this same code, so a stored input
//! means the same thing in both places.
//!
//! Inputs with several parts split them at [`wrapper::SEPARATOR`]; a leading
//! selector byte picks the format, decoder or recovery mode where a target
//! has more than one. `fuzz/README.md` documents each layout.

use pagura_core::{Diagnostic, Error, Limits, Recovery, Result};

pub mod content;
pub mod document;
pub mod filters;
pub mod font;
pub mod host;
pub mod image;
pub mod render;
pub mod syntax;
pub mod text;
pub mod wrapper;

/// A driver: runs one input, panicking only when a property is violated.
pub type Driver = fn(&[u8]);

/// Every target, by the name `cargo fuzz` and `fuzz/regressions/` use.
pub const TARGETS: [(&str, Driver); 9] = [
    ("syntax", syntax::run),
    ("filters", filters::run),
    ("document", document::run),
    ("render", render::run),
    ("font", font::run),
    ("image", image::run),
    ("text", text::run),
    ("content", content::run),
    ("host", host::run),
];

/// Runs the driver of `target` on `data`. Returns `false` for an unknown
/// target name.
pub fn run(target: &str, data: &[u8]) -> bool {
    match TARGETS.iter().find(|(name, _)| *name == target) {
        Some((_, driver)) => {
            driver(data);
            true
        }
        None => false,
    }
}

/// Budgets for hostile input, as `Limits` recommends for bulk processing:
/// small enough that one input finishes well inside the fuzzing time limit,
/// large enough for every generated fixture. Work that outgrows the time
/// limit under these budgets is unbounded work, and therefore a finding.
pub fn limits() -> Limits {
    Limits {
        file_bytes: 4 << 20,
        objects: 10_000,
        value_nodes: 200_000,
        string_bytes: 1 << 20,
        stream_bytes: 4 << 20,
        decoded_bytes: 16 << 20,
        revisions: 64,
        pages: 256,
        pixels: 1 << 18,
        operations: 100_000,
        path_segments: 200_000,
        raster_work: 50_000_000,
        mask_sources: 32,
        display_commands: 10_000,
        ..Limits::default()
    }
}

/// The recovery mode a selector bit chooses.
fn recovery(strict: bool) -> Recovery {
    if strict {
        Recovery::Strict
    } else {
        Recovery::Repair
    }
}

/// The value of a successful result. An error must be structured: it has a
/// fixed message, and its feature and refused-repair tags follow the
/// `area.item` convention, so callers can branch on them.
fn checked<T>(result: Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            assert_structured(&error);
            None
        }
    }
}

fn assert_structured(error: &Error) {
    assert!(
        !error.message.is_empty(),
        "error without a message: {error:?}"
    );
    if let Some(feature) = error.feature() {
        assert!(is_tag(feature.as_str()), "malformed feature tag: {error:?}");
    }
    if let Some(repair) = error.repair {
        assert!(is_tag(repair.as_str()), "malformed repair tag: {error:?}");
    }
}

/// Repairs carry tags and messages, and strict mode makes none (ADR 0003).
fn assert_diagnostics(diagnostics: &[Diagnostic], recovery: Recovery) {
    assert!(
        recovery == Recovery::Repair || diagnostics.is_empty(),
        "strict mode reported repairs: {diagnostics:?}"
    );
    for diagnostic in diagnostics {
        assert!(
            is_tag(diagnostic.repair.as_str()) && !diagnostic.message.is_empty(),
            "malformed diagnostic: {diagnostic:?}"
        );
    }
}

/// `area.item`: a lowercase area, a dot and a printable item.
fn is_tag(tag: &str) -> bool {
    tag.split_once('.').is_some_and(|(area, item)| {
        !area.is_empty()
            && area
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && !item.is_empty()
            && item.bytes().all(|b| b.is_ascii_graphic())
    })
}
