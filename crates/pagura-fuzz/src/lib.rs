#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Drivers of Pagura's coverage-guided fuzz targets.
//!
//! Each driver feeds one input through public APIs and checks what every
//! input must keep: no panic, either a structured error or a result that
//! honours its documented contract, and work bounded by [`limits`]. The
//! libFuzzer entry points in `fuzz/fuzz_targets/` are one line each; the
//! stable replay test `tests/regressions.rs` runs the stored findings in
//! `fuzz/regressions/` through this same code, so a stored input means the
//! same thing in both places. This crate is test tooling and never a
//! dependency of a product crate.
//!
//! Inputs with several parts split them at [`wrapper::SEPARATOR`]; a leading
//! selector byte picks the format, decoder or recovery mode where a target
//! has more than one. `fuzz/README.md` documents each layout.

use pagura_core::{Diagnostic, Error, Limits, Recovery, Result};

pub mod cmap;
pub mod content;
pub mod document;
pub mod extraction;
pub mod filters;
pub mod font;
pub mod forms;
pub mod host;
pub mod image;
pub mod navigation;
pub mod render;
pub mod security;
pub mod syntax;
pub mod text;
pub mod wrapper;
pub mod write;

/// A driver: runs one input, panicking only when a property is violated.
pub type Driver = fn(&[u8]);

/// Every target, by the name `cargo fuzz` and `fuzz/regressions/` use.
pub const TARGETS: [(&str, Driver); 15] = [
    ("syntax", syntax::run),
    ("filters", filters::run),
    ("document", document::run),
    ("render", render::run),
    ("font", font::run),
    ("image", image::run),
    ("text", text::run),
    ("content", content::run),
    ("host", host::run),
    ("navigation", navigation::run),
    ("extraction", extraction::run),
    ("write", write::run),
    ("forms", forms::run),
    ("cmap", cmap::run),
    ("security", security::run),
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
        function_work: 50_000_000,
        // The default ratio: 32 units per pixel at the pixel cap.
        decode_work: 32 << 18,
        mask_sources: 32,
        display_commands: 10_000,
        font_work: 100_000,
        font_entries: 10_000,
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

/// Diagnostics carry stable tags and messages; Strict never repairs (ADR 0003).
fn assert_diagnostics(diagnostics: &[Diagnostic], recovery: Recovery) {
    assert!(
        recovery == Recovery::Repair || diagnostics.iter().all(|item| item.repair().is_none()),
        "strict mode reported repairs: {diagnostics:?}"
    );
    for diagnostic in diagnostics {
        assert!(
            is_tag(diagnostic.tag()) && !diagnostic.message.is_empty(),
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

#[cfg(test)]
mod tests {
    use super::assert_diagnostics;
    use pagura::{Diagnostic, Position, Recovery};
    use pagura_core::repairs as core;
    use pagura_text::repairs as text;

    /// Strict accepts information and rejects every repair, whatever the
    /// tags (ADR 0003).
    #[test]
    fn strict_accepts_information_and_rejects_every_repair() {
        let information = [
            pagura_text::information::SUBSTITUTED,
            pagura_text::information::CID_GLYPH_MISSING,
        ]
        .map(|tag| Diagnostic::information(tag, "provider information"));
        assert_diagnostics(&information, Recovery::Strict);
        let text = [
            text::TOUNICODE_INVALID,
            text::CMAP_CODESPACE,
            text::DESCRIPTOR_WEIGHT,
        ];
        for &repair in core::ALL.iter().chain(&text) {
            let mut diagnostics = information.to_vec();
            diagnostics.push(Diagnostic::new(repair, "malformed input").at(Position::Buffer(7)));
            assert_diagnostics(&diagnostics, Recovery::Repair);
            assert!(
                std::panic::catch_unwind(|| assert_diagnostics(&diagnostics, Recovery::Strict))
                    .is_err(),
                "Strict accepted {repair}"
            );
        }
    }
}
