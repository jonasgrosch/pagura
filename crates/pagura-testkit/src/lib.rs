#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
#![expect(clippy::panic, reason = "test support: failing loudly is the purpose")]
//! Test-only helpers shared by every crate's tests. Depends on `std` only,
//! so any crate can use it as a dev-dependency without cycles.
//!
//! - Fixtures: [`fixture`], [`fixture_text`], [`fixture_names`],
//!   [`open_fixture`] and [`Manifest`] for `corpus/manifests/<area>.json`.
//! - Data: [`Json`], [`hex`], [`unhex`], [`unhex_array`], [`ppm`].
//! - Assertions: [`assert_bytes_eq`].
//! - Host protocol frames: [`frame`] and [`split_frames`].

mod frames;
mod json;
mod manifest;

pub use frames::{Frame, frame, split_frames};
pub use json::{Json, JsonError};
pub use manifest::{Case, Manifest};

use std::fmt::Display;
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
#[track_caller]
pub fn fixture(area: &str, file: &str) -> Vec<u8> {
    let path = fixture_path(area, file);
    std::fs::read(&path).unwrap_or_else(|error| panic!("fixture {}: {error}", path.display()))
}

/// Contents of a generated text fixture, such as expected output or test
/// vectors.
///
/// # Panics
///
/// Panics if the fixture does not exist or is not UTF-8.
#[track_caller]
pub fn fixture_text(area: &str, file: &str) -> String {
    let path = fixture_path(area, file);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("fixture {}: {error}", path.display()))
}

/// The names (file stems) of an area's fixtures with `extension`, sorted;
/// for example `fixture_names("codec", "params")`.
///
/// # Panics
///
/// Panics if the area directory cannot be read or a name is not UTF-8.
#[track_caller]
pub fn fixture_names(area: &str, extension: &str) -> Vec<String> {
    let directory = fixture_path(area, "");
    let entries = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("fixtures {}: {error}", directory.display()));
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("fixtures {}: {error}", directory.display()))
                .path()
        })
        .filter(|path| path.extension().is_some_and(|found| found == extension))
        .map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_else(|| panic!("fixture name {} is not UTF-8", path.display()))
                .to_owned()
        })
        .collect();
    names.sort();
    names
}

/// Opens a generated fixture with `open` (for example a closure calling
/// `Document::open` with the options a test needs) and returns the result.
///
/// # Panics
///
/// Panics if the fixture is missing or `open` fails, naming the fixture and
/// the error.
#[track_caller]
pub fn open_fixture<T, E: Display>(
    area: &str,
    file: &str,
    open: impl FnOnce(Vec<u8>) -> Result<T, E>,
) -> T {
    open(fixture(area, file)).unwrap_or_else(|error| panic!("{area}/{file}: {error}"))
}

/// Encodes packed RGB8 pixels as a binary PPM (P6), the format of the
/// analytic oracles.
pub fn ppm(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let mut data = format!("P6\n{width} {height}\n255\n").into_bytes();
    data.extend_from_slice(rgb);
    data
}

/// Lowercase hexadecimal, two digits per byte.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&byte| [byte >> 4, byte & 15])
        .map(|digit| char::from(DIGITS[usize::from(digit)]))
        .collect()
}

/// Decodes hexadecimal digits of either case, ignoring ASCII white space
/// between them, as test vectors are often written.
///
/// # Panics
///
/// Panics on a character that is neither a digit nor white space, or an odd
/// number of digits.
#[track_caller]
pub fn unhex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .map(|c| {
            c.to_digit(16)
                .and_then(|digit| u8::try_from(digit).ok())
                .unwrap_or_else(|| panic!("{c:?} is not a hexadecimal digit in {text:?}"))
        })
        .collect();
    let (pairs, odd) = digits.as_chunks::<2>();
    assert!(
        odd.is_empty(),
        "odd number of hexadecimal digits in {text:?}"
    );
    pairs.iter().map(|&[high, low]| (high << 4) | low).collect()
}

/// [`unhex`] into an array of exactly `N` bytes.
///
/// # Panics
///
/// As [`unhex`], or if the digits do not encode exactly `N` bytes.
#[track_caller]
pub fn unhex_array<const N: usize>(text: &str) -> [u8; N] {
    let bytes = unhex(text);
    <[u8; N]>::try_from(bytes.as_slice())
        .unwrap_or_else(|_| panic!("{text:?} encodes {} bytes, not {N}", bytes.len()))
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

#[cfg(test)]
mod tests {
    use super::{
        assert_bytes_eq, fixture_names, fixture_text, hex, open_fixture, unhex, unhex_array,
    };

    #[test]
    fn hex_round_trips_and_tolerates_case_and_spacing() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(unhex("000FA5ff"), [0x00, 0x0f, 0xa5, 0xff]);
        assert_eq!(unhex("de ad\n be\tef"), [0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(unhex(""), []);
        assert_eq!(unhex_array::<2>("cafe"), [0xca, 0xfe]);
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(unhex(&hex(&bytes)), bytes);
    }

    #[test]
    #[should_panic(expected = "odd number of hexadecimal digits")]
    fn odd_hex_fails_loudly() {
        unhex("abc");
    }

    #[test]
    #[should_panic(expected = "is not a hexadecimal digit")]
    fn invalid_hex_fails_loudly() {
        unhex("0g");
    }

    #[test]
    #[should_panic(expected = "encodes 1 bytes, not 2")]
    fn wrong_array_length_fails_loudly() {
        unhex_array::<2>("ff");
    }

    #[test]
    fn fixture_listing_is_sorted_and_filtered_by_extension() {
        let names = fixture_names("classic", "pdf");
        assert!(names.contains(&"rectangle".to_owned()));
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            fixture_names("classic", "no-such-extension"),
            Vec::<String>::new()
        );
        let first = &fixture_names("codec", "params")[0];
        assert_ne!(fixture_text("codec", &format!("{first}.params")), "");
    }

    #[test]
    fn opening_reports_the_fixture_and_error() {
        let length = open_fixture("classic", "rectangle.pdf", |bytes| {
            Ok::<_, String>(bytes.len())
        });
        assert!(length > 0);
        let failure = std::panic::catch_unwind(|| {
            open_fixture("classic", "rectangle.pdf", |_| {
                Err::<(), _>("not a document")
            });
        })
        .unwrap_err();
        let message = failure.downcast_ref::<String>().unwrap();
        assert_eq!(message, "classic/rectangle.pdf: not a document");
    }

    #[test]
    #[should_panic(expected = "differ at byte 2")]
    fn byte_differences_name_the_first_offset() {
        assert_bytes_eq(b"abcd", b"abxd", "test");
    }
}
