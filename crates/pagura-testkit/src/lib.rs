#![forbid(unsafe_code)]
#![expect(clippy::panic, reason = "test support: failing loudly is the purpose")]
//! Test-only helpers shared by every crate's tests. Depends on `std` only,
//! so any crate can use it as a dev-dependency without cycles.

use std::path::PathBuf;

/// The workspace root directory.
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Path of a generated fixture, e.g. `fixture_path("classic", "rectangle.pdf")`.
pub fn fixture_path(area: &str, file: &str) -> PathBuf {
    workspace_root()
        .join("corpus/generated")
        .join(area)
        .join(file)
}

/// Contents of a generated fixture.
///
/// # Panics
///
/// Panics if the fixture does not exist; run `python3 tools/fixtures.py`.
pub fn fixture(area: &str, file: &str) -> Vec<u8> {
    let path = fixture_path(area, file);
    std::fs::read(&path).unwrap_or_else(|error| panic!("fixture {}: {error}", path.display()))
}

/// Encodes packed RGB8 pixels as a binary PPM (P6), the format of the
/// analytic oracles.
pub fn ppm(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let mut data = format!("P6\n{width} {height}\n255\n").into_bytes();
    data.extend_from_slice(rgb);
    data
}

/// Asserts byte equality, reporting the first difference instead of
/// printing both buffers.
///
/// # Panics
///
/// Panics if the buffers differ.
#[track_caller]
pub fn assert_bytes_eq(actual: &[u8], expected: &[u8], context: &str) {
    if actual == expected {
        return;
    }
    let first = actual
        .iter()
        .zip(expected)
        .position(|(a, e)| a != e)
        .unwrap_or_else(|| actual.len().min(expected.len()));
    panic!(
        "{context}: buffers differ at byte {first} (lengths {} and {}): {:?} vs {:?}",
        actual.len(),
        expected.len(),
        actual.get(first..(first + 8).min(actual.len())),
        expected.get(first..(first + 8).min(expected.len())),
    );
}
