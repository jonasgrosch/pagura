//! Rendering against independent analytic oracles.

use crate::support::{self, open, ppm};
use pagura::{Document, ErrorKind, OpenOptions, RenderOptions, Tile};
use pagura_testkit::{Json, Manifest, assert_bytes_eq, fixture};

#[test]
fn nested_tiling_under_a_patterned_stencil_fails_before_any_output_in_both_modes() {
    use pagura::{Recovery, RenderContext};
    use std::sync::Arc;

    for recovery in [Recovery::Strict, Recovery::Repair] {
        let document = Arc::new(
            Document::open(
                fixture("paint_stencils", "tiling-nested-ordinary-refused.pdf"),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap(),
        );
        let context = RenderContext::new(document);
        let page = context.document().page(0).unwrap();
        let scene = context.scene(page).unwrap();
        let mut called = false;
        let result = context.render_bands(
            &scene,
            &RenderOptions::default(),
            None,
            |_| false,
            |_, _| {
                called = true;
                Ok::<(), ()>(())
            },
        );
        let error = result.unwrap_err();
        assert_eq!(
            error.feature().map(pagura::Feature::as_str),
            Some("pattern.image-mask")
        );
        assert!(!called, "unsupported geometry reached the output consumer");
        assert_eq!(
            context
                .render_page(page, &RenderOptions::default())
                .unwrap_err()
                .feature(),
            error.feature()
        );
    }
}

/// Every classic case with an image oracle renders to it, exactly or within
/// the tolerance its manifest entry records (ADR 0004).
#[test]
fn pixels_match_independent_oracles_within_their_recorded_tolerance() {
    let manifest = Manifest::load("classic");
    let cases: Vec<_> = manifest
        .cases()
        .filter(|case| case.string("expected") == Some("render"))
        .collect();
    assert!(cases.len() >= 18, "classic render cases missing");
    for case in cases {
        let name = case.name();
        let document = open(fixture("classic", &format!("{name}.pdf"))).unwrap();
        let dpi = case.get("dpi").and_then(Json::as_f64).unwrap();
        let pixmap = pagura::render_page(
            &document,
            document.page(0).unwrap(),
            &RenderOptions::from_dpi(dpi),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        let actual = ppm(&pixmap);
        let expected = fixture("classic", &format!("{name}.ppm"));
        match case.get("tolerance").and_then(Json::as_u64).unwrap_or(0) {
            0 => assert_bytes_eq(&actual, &expected, name),
            tolerance => {
                assert_eq!(actual.len(), expected.len(), "{name}");
                let worst = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, e)| u64::from(a.abs_diff(*e)))
                    .max()
                    .unwrap();
                assert!(worst <= tolerance, "{name}: differs by {worst} levels");
            }
        }
    }
}

#[test]
fn unsupported_features_are_tagged_errors_not_partial_images() {
    for (path, feature) in support::unsupported_cases() {
        let document = open(std::fs::read(&path).unwrap()).unwrap();
        let error = pagura::render_page(
            &document,
            document.page(0).unwrap(),
            &RenderOptions::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.feature().map(pagura::Feature::as_str),
            Some(feature.as_str()),
            "{}: {error}",
            path.display()
        );
    }
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
    support::assert_tile_matches(&full, &part, tile);
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
