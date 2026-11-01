//! Rendering against independent analytic oracles.

use crate::support::{self, open, ppm};
use pagura::{Document, ErrorKind, OpenOptions, RenderOptions, Tile};
use pagura_testkit::{Json, Manifest, assert_bytes_eq, fixture};

#[test]
fn nested_patterned_glyphs_preserve_true_curves_in_both_modes() {
    use pagura::{Recovery, RenderContext};
    use std::sync::Arc;
    for name in ["glyph-quadratic-refused", "glyph-tiny-quadratics-refused"] {
        let expected = fixture("paint_nested_stencils", &format!("{name}.ppm"));
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Arc::new(
                Document::open(
                    fixture("paint_nested_stencils", &format!("{name}.pdf")),
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
            let rendered = context
                .render_page(page, &RenderOptions::default())
                .unwrap();
            assert_eq!(ppm(&rendered), expected);
            let mut called = false;
            context
                .render_bands(
                    &scene,
                    &RenderOptions::default(),
                    None,
                    |_| false,
                    |_, _| {
                        called = true;
                        Ok::<(), ()>(())
                    },
                )
                .unwrap()
                .unwrap();
            assert!(called);
        }
    }
}

#[test]
fn nested_curved_stencil_manifest_matches_independent_oracles() {
    let manifest = Manifest::load("paint_curved_stencils");
    let mut count = 0;
    for case in manifest.cases() {
        if case.string("expected") != Some("render") {
            continue;
        }
        count += 1;
        let name = case.name();
        let document = open(fixture("paint_curved_stencils", &format!("{name}.pdf"))).unwrap();
        let actual = ppm(&pagura::render_page(
            &document,
            document.page(0).unwrap(),
            &RenderOptions::default(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}")));
        let expected = fixture("paint_curved_stencils", &format!("{name}.ppm"));
        assert_eq!(actual.len(), expected.len(), "{name}");
        let tolerance = case.get("tolerance").and_then(Json::as_u64).unwrap_or(0);
        let maximum = actual
            .iter()
            .zip(&expected)
            .map(|(a, e)| u64::from(a.abs_diff(*e)))
            .max()
            .unwrap_or(0);
        assert!(
            maximum <= tolerance,
            "{name}: maximum channel error {maximum}"
        );
    }
    assert!(count >= 36);
}

#[test]
fn nested_curved_proof_work_is_reserved_before_streaming() {
    use pagura::{Recovery, RenderContext};
    use std::sync::Arc;
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let document = Arc::new(
            Document::open(
                fixture("paint_curved_stencils", "stress-256-three-curves.pdf"),
                &OpenOptions {
                    recovery,
                    limits: pagura::Limits {
                        render_work: 1_000_000,
                        ..pagura::Limits::default()
                    },
                    ..OpenOptions::default()
                },
            )
            .unwrap(),
        );
        let context = RenderContext::new(document);
        let scene = context.scene(context.document().page(0).unwrap()).unwrap();
        let mut called = false;
        let error = context
            .render_bands(
                &scene,
                &RenderOptions::default(),
                None,
                |_| false,
                |_, _| {
                    called = true;
                    Ok::<(), ()>(())
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.message, "render work budget exceeded");
        assert!(!called);
    }
}

#[test]
fn nested_tiling_preserves_the_independent_half_area_in_both_modes() {
    use pagura::Recovery;

    let expected = fixture("paint_stencils", "tiling-flat-half-area.ppm");
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let document = Document::open(
            fixture("paint_stencils", "tiling-nested-ordinary-refused.pdf"),
            &OpenOptions {
                recovery,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let actual = ppm(&pagura::render_page(
            &document,
            document.page(0).unwrap(),
            &RenderOptions::default(),
        )
        .unwrap());
        assert_eq!(actual.len(), expected.len());
        assert!(
            actual
                .iter()
                .zip(&expected)
                .all(|(a, e)| a.abs_diff(*e) <= 1)
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
