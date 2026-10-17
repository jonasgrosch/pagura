#![forbid(unsafe_code)]
//! Rendering against independent analytic oracles.

use pagura::{Document, OpenOptions, Pixmap, Rgba8};
use pagura_testkit::fixture;

fn open(bytes: impl Into<std::sync::Arc<[u8]>>) -> pagura::Result<Document> {
    Document::open(bytes, &OpenOptions::default())
}

fn ppm(pixmap: &Pixmap) -> Vec<u8> {
    pagura_testkit::ppm(
        pixmap.width(),
        pixmap.height(),
        &pixmap.to_rgb_over(Rgba8::WHITE),
    )
}
use pagura::{ErrorKind, RenderOptions, Tile};

const RENDER_CASES: [&str; 17] = [
    "blank",
    "colorspace-unsupported",
    "alpha-unsupported",
    "rectangle",
    "graphics-state",
    "negative-rectangle",
    "inherited-box",
    "clipped-to-page",
    "many-rectangles",
    "content-array-split",
    "many-operands",
    "normalized-media-box",
    "undefined-reference-is-null",
    "trailing-bytes",
    "rotate-90-blank",
    "opaque-page-group-blank",
    "two-pages",
];

const UNSUPPORTED_CASES: [(&str, &str); 1] = [("text-unsupported", "operator.BT")];

#[test]
fn exact_pixels_match_independent_oracles() {
    for name in RENDER_CASES {
        let document = open(fixture("classic", &format!("{name}.pdf"))).unwrap();
        let pixmap = pagura::render_page(
            &document,
            document.page(0).unwrap(),
            &RenderOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(
            ppm(&pixmap) == fixture("classic", &format!("{name}.ppm")),
            "{name}"
        );
    }
}

#[test]
fn unsupported_features_are_tagged_errors_not_partial_images() {
    for (name, feature) in UNSUPPORTED_CASES {
        let document = open(fixture("classic", &format!("{name}.pdf"))).unwrap();
        let error = pagura::render_page(
            &document,
            document.page(0).unwrap(),
            &RenderOptions::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.feature().map(pagura::Feature::as_str),
            Some(feature),
            "{name}: {error}"
        );
    }
}

#[test]
fn fractional_edges_match_exact_coverage_within_one_level() {
    let document = open(fixture("classic", "fractional-rectangle.pdf")).unwrap();
    let pixmap = pagura::render_page(
        &document,
        document.page(0).unwrap(),
        &RenderOptions::default(),
    )
    .unwrap();
    let actual = ppm(&pixmap);
    let expected = fixture("classic", "fractional-rectangle.ppm");
    assert_eq!(actual.len(), expected.len());
    let worst = actual
        .iter()
        .zip(&expected)
        .map(|(a, e)| a.abs_diff(*e))
        .max()
        .unwrap();
    assert!(worst <= 1, "coverage differs by {worst} levels");
}

#[test]
fn tiles_assemble_into_the_full_page() {
    let document = open(fixture("classic", "many-rectangles.pdf")).unwrap();
    let page = document.page(0).unwrap();
    let options = RenderOptions::default();
    let full = pagura::render_page(&document, page, &options).unwrap();
    let tile = Tile {
        x: 96,
        y: 160,
        width: 128,
        height: 64,
    };
    let part = pagura::render_tile(&document, page, &options, tile).unwrap();
    for row in 0..tile.height {
        let start = usize::try_from(tile.x).unwrap() * 4;
        let expected = &full.row(tile.y + row).unwrap()[start..start + 128 * 4];
        assert_eq!(part.row(row).unwrap(), expected, "row {row}");
    }
    let outside = Tile {
        x: 500,
        y: 0,
        width: 64,
        height: 64,
    };
    let error = pagura::render_tile(&document, page, &options, outside).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
}

#[test]
fn pixel_budget_and_invalid_scale_are_enforced() {
    let options = OpenOptions {
        limits: pagura::Limits {
            pixels: 100,
            ..pagura::Limits::default()
        },
        ..OpenOptions::default()
    };
    let document = Document::open(fixture("classic", "rectangle.pdf"), &options).unwrap();
    let page = document.page(0).unwrap();
    let error = pagura::render_page(&document, page, &RenderOptions::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    let error = pagura::render_page(&document, page, &RenderOptions::from_dpi(0.0)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
}
