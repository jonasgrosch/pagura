//! Helpers shared by the facade's test modules.

use pagura::{Document, OpenOptions, Pixmap, Recovery, Rgba8, Tile};
use pagura_testkit::Manifest;
use std::path::PathBuf;
use std::sync::Arc;

/// Opens `bytes` with the default options, which repair.
pub(crate) fn open(bytes: impl Into<Arc<[u8]>>) -> pagura::Result<Document> {
    Document::open(bytes, &OpenOptions::default())
}

/// Opens `bytes` in strict mode.
pub(crate) fn open_strict(bytes: impl Into<Arc<[u8]>>) -> pagura::Result<Document> {
    let options = OpenOptions {
        recovery: Recovery::Strict,
        ..OpenOptions::default()
    };
    Document::open(bytes, &options)
}

/// A binary PPM of `pixmap` over white, the format of the image oracles.
pub(crate) fn ppm(pixmap: &Pixmap) -> Vec<u8> {
    pagura_testkit::ppm(
        pixmap.width(),
        pixmap.height(),
        &pixmap.to_rgb_over(Rgba8::WHITE),
    )
}

/// Every classic case expected to be unsupported, as (fixture path, feature
/// tag).
#[expect(
    clippy::expect_used,
    reason = "generated unsupported cases name their fixture and feature"
)]
pub(crate) fn unsupported_cases() -> Vec<(PathBuf, String)> {
    let manifest = Manifest::load("classic");
    let cases: Vec<_> = manifest
        .cases()
        .filter(|case| case.string("expected") == Some("unsupported"))
        .map(|case| {
            let feature = case.string("feature").expect("a feature tag");
            (case.path().expect("a fixture path"), feature.to_owned())
        })
        .collect();
    assert_ne!(cases, [], "the classic manifest needs an unsupported case");
    cases
}

/// Asserts that `part`, rendered for `tile`, equals that region of `full`.
#[expect(clippy::unwrap_used, reason = "rows inside the asserted tile exist")]
pub(crate) fn assert_tile_matches(full: &Pixmap, part: &Pixmap, tile: Tile) {
    let start = usize::try_from(tile.x).unwrap() * 4;
    let end = start + usize::try_from(tile.width).unwrap() * 4;
    assert_eq!((part.width(), part.height()), (tile.width, tile.height));
    for row in 0..tile.height {
        let expected = &full.row(tile.y + row).unwrap()[start..end];
        assert_eq!(part.row(row).unwrap(), expected, "tile row {row}");
    }
}
