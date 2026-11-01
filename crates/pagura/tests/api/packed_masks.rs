//! Complete global-grid samples reconstructed across bounded physical surfaces.

use pagura::{Document, ErrorKind, Limits, OpenOptions, RenderContext, RenderOptions, Rgba8, Tile};
use pagura_testkit::fixture;
use std::sync::Arc;

#[test]
fn one_row_tiles_preserve_bilinear_neighbors_and_asymmetric_orientations() {
    for prefix in ["bilinear", "asymmetric-bilinear"] {
        for variant in ["normal", "mirror", "quarter-turn", "normal-inverse"] {
            let name = format!("{prefix}-{variant}");
            for capacity in [0, 4096, 64 * 1024 * 1024] {
                let options = OpenOptions {
                    limits: Limits {
                        pixels: 10,
                        ..Limits::default()
                    },
                    ..OpenOptions::default()
                };
                let document = Arc::new(
                    Document::open(
                        fixture("packed_mask_tiles", &format!("{name}.pdf")),
                        &options,
                    )
                    .unwrap(),
                );
                let context = RenderContext::with_store_capacity(document, None, capacity);
                let page = context.document().page(0).unwrap();
                // The virtual20x20canvas does not authorize a400pixel output.
                assert_eq!(
                    context
                        .render_page(page, &RenderOptions::from_dpi(180.0))
                        .unwrap_err()
                        .kind,
                    ErrorKind::Limit
                );
                for (background, suffix) in
                    [(Rgba8::WHITE, "white"), (Rgba8::TRANSPARENT, "transparent")]
                {
                    let options = RenderOptions {
                        background,
                        ..RenderOptions::from_dpi(180.0)
                    };
                    let expected =
                        fixture("packed_mask_tiles", &format!("{name}-rgba.{suffix}.rgba"));
                    assert_eq!(expected.len(), 20 * 20 * 4);
                    let mut assembled = Vec::new();
                    // Each requested output is10x1 and preserves the global
                    // sampling transform and the common mask's neighbor grid.
                    for y in 0..20 {
                        for x in [0, 10] {
                            let tile = context
                                .render_tile(
                                    page,
                                    &options,
                                    Tile {
                                        x,
                                        y,
                                        width: 10,
                                        height: 1,
                                    },
                                )
                                .unwrap();
                            assembled.extend_from_slice(tile.row(0).unwrap());
                        }
                    }
                    assert_eq!(assembled.len(), expected.len());
                    let maximum = assembled
                        .iter()
                        .zip(&expected)
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    assert!(
                        maximum <= 1,
                        "{name} {suffix} capacity{capacity}: maximum{maximum}"
                    );
                }
                assert!(context.retained_bytes() <= capacity);
                if capacity == 0 {
                    assert_eq!(context.retained_bytes(), 0);
                }
            }
        }
    }
}

#[test]
fn shared_objects_keep_whole_image_rejections_across_pages_and_store_orders() {
    for (name, soft_page) in [("soft-page-then-main", 0), ("main-page-then-soft", 1)] {
        for capacity in [0, 4096, 64 * 1024 * 1024] {
            for order in [[0, 1, 0], [1, 0, 1]] {
                let document = Arc::new(
                    Document::open(
                        fixture("packed_mask_tiles", &format!("{name}.pdf")),
                        &OpenOptions::default(),
                    )
                    .unwrap(),
                );
                let context = RenderContext::with_store_capacity(document, None, capacity);
                for index in order {
                    let page = context.document().page(index).unwrap();
                    let result = context.render_page(page, &RenderOptions::default());
                    if index == soft_page {
                        let actual = crate::support::ppm(&result.unwrap());
                        let expected = fixture("packed_mask_tiles", "soft-page-then-main.ppm");
                        let maximum = actual
                            .iter()
                            .zip(&expected)
                            .map(|(a, b)| a.abs_diff(*b))
                            .max()
                            .unwrap();
                        assert_eq!(actual.len(), expected.len());
                        assert!(
                            maximum <= 1,
                            "{name} page{index} capacity{capacity}: {maximum}"
                        );
                    } else {
                        assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
                    }
                }
                assert!(context.retained_bytes() <= capacity);
            }
        }
    }
    for (name, kind) in [
        ("soft-then-main", ErrorKind::Limit),
        ("main-then-soft", ErrorKind::Limit),
        ("soft-then-invalid-stencil", ErrorKind::Syntax),
        ("invalid-stencil-then-soft", ErrorKind::Syntax),
    ] {
        let document = Arc::new(
            Document::open(
                fixture("packed_mask_tiles", &format!("{name}.pdf")),
                &OpenOptions::default(),
            )
            .unwrap(),
        );
        let context = RenderContext::with_store_capacity(document, None, 64 * 1024 * 1024);
        let page = context.document().page(0).unwrap();
        // Recording encounters the refused use before any renderer can
        // publish a partial page, including after an admitted soft use.
        assert_eq!(context.scene(page).unwrap_err().kind, kind, "{name}");
        assert_eq!(
            context
                .render_page(page, &RenderOptions::default())
                .unwrap_err()
                .kind,
            kind,
            "{name}"
        );
    }
}

#[test]
fn object_generations_and_independent_documents_keep_distinct_mask_samples() {
    for capacity in [0, 4096, 64 * 1024 * 1024] {
        for (name, alpha) in [
            ("small-flate", 109),
            ("small-generation-1", 109),
            ("small-inverse", 146),
        ] {
            let options = OpenOptions {
                limits: Limits {
                    pixels: 10,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            };
            let document = Arc::new(
                Document::open(
                    fixture("packed_mask_tiles", &format!("{name}.pdf")),
                    &options,
                )
                .unwrap(),
            );
            let context = RenderContext::with_store_capacity(document, None, capacity);
            let page = context.document().page(0).unwrap();
            for background in [Rgba8::WHITE, Rgba8::TRANSPARENT] {
                let options = RenderOptions {
                    background,
                    ..RenderOptions::from_dpi(9.0)
                };
                for _ in 0..2 {
                    let pixmap = context.render_page(page, &options).unwrap();
                    let expected = if background == Rgba8::WHITE {
                        [255, 255 - alpha, 255 - alpha, 255]
                    } else {
                        [alpha, 0, 0, alpha]
                    };
                    assert_eq!(
                        pixmap.row(0).unwrap(),
                        expected.as_slice(),
                        "{name} capacity{capacity}"
                    );
                }
            }
        }
    }
}

#[test]
fn repeated_tiled_source_roots_preserve_pixels_across_dropped_scenes_and_pages() {
    for name in [
        "scene-forty-shared",
        "scene-forty-shared-mask-different-parents",
        "scene-forty-shared-two-pages",
    ] {
        for capacity in [0, 4096, 64 * 1024 * 1024] {
            let document = Arc::new(
                Document::open(
                    fixture("packed_mask_tiles", &format!("{name}.pdf")),
                    &OpenOptions::default(),
                )
                .unwrap(),
            );
            let context = RenderContext::with_store_capacity(document, None, capacity);
            for index in [0, context.document().page_count().unwrap() - 1, 0] {
                let page = context.document().page(index).unwrap();
                drop(context.scene(page).unwrap());
                for (background, suffix) in
                    [(Rgba8::WHITE, "white"), (Rgba8::TRANSPARENT, "transparent")]
                {
                    let options = RenderOptions {
                        background,
                        ..RenderOptions::default()
                    };
                    let actual = context.render_page(page, &options).unwrap();
                    let expected =
                        fixture("packed_mask_tiles", &format!("{name}-rgba.{suffix}.rgba"));
                    let rgba: Vec<_> = (0..actual.height())
                        .flat_map(|y| actual.row(y).unwrap().iter().copied())
                        .collect();
                    assert_eq!(rgba, expected, "{name} {suffix} page{index} cap{capacity}");
                }
            }
            assert!(context.retained_bytes() <= capacity);
            if capacity == 0 {
                assert_eq!(context.retained_bytes(), 0);
            }
        }
    }
}

#[test]
fn distinct_roots_whole_purposes_and_low_scene_limits_never_publish_partial_store() {
    for name in [
        "scene-forty-distinct-mask-roots",
        "scene-forty-shared-then-main",
        "scene-main-then-forty-shared",
    ] {
        for capacity in [0, 4096, 64 * 1024 * 1024] {
            let document = Arc::new(
                Document::open(
                    fixture("packed_mask_tiles", &format!("{name}.pdf")),
                    &OpenOptions::default(),
                )
                .unwrap(),
            );
            let context = RenderContext::with_store_capacity(document, None, capacity);
            let page = context.document().page(0).unwrap();
            let before = context.retained_bytes();
            assert_eq!(context.scene(page).unwrap_err().kind, ErrorKind::Limit);
            assert_eq!(context.retained_bytes(), before);
            assert_eq!(
                context
                    .render_page(page, &RenderOptions::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Limit,
            );
            assert_eq!(context.retained_bytes(), before);
        }
    }
    for bytes in [0, 1024, 1024 * 1024] {
        let document = Arc::new(
            Document::open(
                fixture("packed_mask_tiles", "scene-forty-shared.pdf"),
                &OpenOptions {
                    limits: Limits {
                        render_bytes: bytes,
                        ..Limits::default()
                    },
                    ..OpenOptions::default()
                },
            )
            .unwrap(),
        );
        let context = RenderContext::with_store_capacity(document, None, 64 * 1024 * 1024);
        let page = context.document().page(0).unwrap();
        assert_eq!(context.scene(page).unwrap_err().kind, ErrorKind::Limit);
        assert_eq!(context.retained_bytes(), 0);
    }
}
