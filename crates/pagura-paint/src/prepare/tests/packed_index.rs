//! Frozen pre-code Fraction pixels and private unexecuted-query boundaries.

use crate::source::ImageSampler;
use crate::{RenderRequest, Scratch, prepare};
use pagura_core::{ErrorKind, Limits, Store};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::Interpreter;
use pagura_raster::Rgba8;
use pagura_testkit::fixture;

pub(super) fn scene(name: &str, limits: &Limits) -> pagura_interpret::Scene {
    let document = Document::open(
        fixture("packed_mask_tiles", &format!("{name}.pdf")),
        &OpenOptions {
            limits: *limits,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    Interpreter::new(&document)
        .scene(document.page(0).unwrap())
        .unwrap()
}

pub(super) fn pixels(prepared: &crate::PreparedScene) -> Vec<u8> {
    let mut scratch = Scratch::default();
    let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
    let mut out = Vec::new();
    rendering.render(&mut out);
    super::assert_within_charges(&rendering, prepared.charges());
    out
}

#[test]
fn indexed_minification_preserves_independent_full_pixels_without_preparation_queries() {
    for (name, background, suffix) in [
        ("minified-forty-rows", Rgba8::WHITE, "white"),
        ("minified-forty-rows", Rgba8::TRANSPARENT, "transparent"),
        ("minified-forty-mirror", Rgba8::WHITE, "white"),
        ("minified-forty-quarter", Rgba8::WHITE, "white"),
    ] {
        let source = scene(name, &Limits::default());
        let mut request = RenderRequest::page(&source.page, 1.0).unwrap();
        request.background = background;
        ImageSampler::reset_tiled_test_counts();
        let prepared = prepare(&source, &request, &Store::new(0), &Limits::default()).unwrap();
        assert_eq!(
            ImageSampler::tiled_test_counts(),
            (0, 1),
            "{name}: no query during preparation"
        );
        assert!(prepared.charges().render_work() > 900_000_000);
        let actual = pixels(&prepared);
        let expected = fixture("packed_mask_tiles", &format!("{name}-rgba.{suffix}.rgba"));
        assert_eq!(actual.len(), expected.len());
        assert!(
            actual
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.abs_diff(*b) <= 1),
            "{name}: independent pixels"
        );
        assert!(ImageSampler::tiled_test_counts().0 > 0);
    }
}

#[test]
fn old_small_footprint_routes_fit_lower_work_and_memory_without_an_index() {
    for (name, limits, expected) in [
        (
            "highwidth-short-crop",
            Limits {
                render_bytes: 16 * 1024 * 1024,
                ..Limits::default()
            },
            "highwidth-short-crop-rgba.white.rgba",
        ),
        (
            "highwidth-short-crop",
            Limits {
                render_work: 100_000_000,
                ..Limits::default()
            },
            "highwidth-short-crop-rgba.white.rgba",
        ),
        (
            "default-large",
            Limits {
                render_work: 300_000_000,
                ..Limits::default()
            },
            "default-large-rgba.white.rgba",
        ),
        (
            "default-large",
            Limits {
                render_bytes: 32 * 1024 * 1024,
                ..Limits::default()
            },
            "default-large-rgba.white.rgba",
        ),
        (
            "scene-forty-shared",
            Limits::default(),
            "scene-forty-shared-rgba.white.rgba",
        ),
    ] {
        let source = scene(name, &Limits::default());
        let request = RenderRequest::page(&source.page, 1.0).unwrap();
        ImageSampler::reset_tiled_test_counts();
        let prepared = prepare(&source, &request, &Store::new(0), &limits).unwrap();
        assert_eq!(ImageSampler::tiled_test_counts(), (0, 0));
        let actual = pixels(&prepared);
        let expected = fixture("packed_mask_tiles", expected);
        assert_eq!(actual.len(), expected.len());
        assert!(actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1));
    }
}

#[test]
fn original_work_and_late_cumulative_failures_never_execute_opacity_queries() {
    for (original, caller) in [
        (
            Limits {
                render_work: 500_000_000,
                ..Limits::default()
            },
            Limits::default(),
        ),
        (
            Limits::default(),
            Limits {
                render_work: 1_400_000_000,
                ..Limits::default()
            },
        ),
    ] {
        let source = scene("minified-forty-rows", &original);
        let request = RenderRequest::page(&source.page, 1.0).unwrap();
        ImageSampler::reset_tiled_test_counts();
        let store = Store::new(0);
        let error = prepare(&source, &request, &store, &caller).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(ImageSampler::tiled_test_counts().0, 0);
        assert_eq!(store.retained_bytes(), 0);
        if original.render_work == 500_000_000 {
            assert_eq!(error.message, "render work budget exceeded");
            assert_eq!(ImageSampler::tiled_test_counts().1, 0);
        } else {
            assert_eq!(
                ImageSampler::tiled_test_counts().1,
                1,
                "late failure does not refund executed construction"
            );
        }
    }
}
