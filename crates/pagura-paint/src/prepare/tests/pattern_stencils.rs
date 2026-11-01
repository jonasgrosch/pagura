//! Independent geometric fixtures plus preparation bounds and invariance.

use crate::{RenderRequest, Scratch};
use pagura_core::{ErrorKind, Limits, Store};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::{Interpreter, Scene, SceneRecorder};
use pagura_raster::DeviceRect;
use pagura_testkit::{Manifest, fixture};
use std::sync::Arc;

fn scene(name: &str) -> Scene {
    scene_in("paint_stencils", name)
}

fn scene_in(area: &str, name: &str) -> Scene {
    assert_eq!(
        pagura_testkit::workspace_root().canonicalize().unwrap(),
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap(),
        "fixture helper belongs to the tested workspace",
    );
    let document = Document::open(
        fixture(area, &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
    .unwrap();
    let mut recorder = SceneRecorder::new(Limits::default());
    Interpreter::new(&document)
        .run(document.page(0).unwrap(), &mut recorder)
        .unwrap();
    recorder.finish().unwrap()
}

#[test]
fn patterned_stencil_oracles_and_negative_controls() {
    for area in ["paint_stencils", "paint_nested_stencils"] {
        for case in Manifest::load(area).cases() {
            let name = case.name();
            let scene = scene_in(area, name);
            let mut request = RenderRequest::page(&scene.page, 1.0).unwrap();
            if scene.print_intent {
                request.overprint = crate::OverprintMode::Simulate;
            }
            let store = Store::new(1 << 20);
            let prepared = crate::prepare(&scene, &request, &store, &Limits::default());
            if case.string("expected") == Some("unsupported") {
                assert_eq!(
                    prepared.unwrap_err().feature(),
                    Some(pagura_interpret::features::PATTERN_IMAGE_MASK)
                );
                continue;
            }
            let prepared = prepared.unwrap_or_else(|error| panic!("{name}: {error}"));
            let expected = fixture(area, &format!("{name}.ppm"));
            let expected = expected.splitn(5, u8::is_ascii_whitespace).nth(4).unwrap();
            let mut scratch = Scratch::default();
            let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
            let mut actual = Vec::new();
            rendering.render(&mut actual);
            super::assert_within_charges(&rendering, prepared.charges());
            assert_eq!(
                actual.len() / 4,
                expected.len() / 3,
                "{name}: complete raster dimensions"
            );
            let maximum = actual
                .as_chunks::<4>()
                .0
                .iter()
                .zip(expected.as_chunks::<3>().0.iter())
                .flat_map(|(actual, expected)| actual[..3].iter().zip(expected))
                .map(|(actual, expected)| actual.abs_diff(*expected))
                .max()
                .unwrap();
            assert!(maximum <= 1, "{name}: maximum channel error {maximum}");
        }
    }
}

#[test]
fn stencil_geometry_obeys_path_clip_work_and_memory_caps() {
    let scene = scene("colored-aligned");
    let request = RenderRequest::page(&scene.page, 1.0).unwrap();
    for limits in [
        Limits {
            path_segments: 14,
            ..Limits::default()
        },
        Limits {
            mask_sources: 1,
            ..Limits::default()
        },
        Limits {
            render_work: 0,
            ..Limits::default()
        },
        Limits {
            render_bytes: 0,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 0,
            ..Limits::default()
        },
    ] {
        let store = Store::new(0);
        let error = crate::prepare(&scene, &request, &store, &limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{limits:?}: {error}");
    }
    let store = Store::new(1 << 20);
    crate::prepare(&scene, &request, &store, &Limits::default()).unwrap();
    for limits in [
        Limits {
            path_segments: 14,
            ..Limits::default()
        },
        Limits {
            mask_sources: 1,
            ..Limits::default()
        },
        Limits {
            render_work: 0,
            ..Limits::default()
        },
        Limits {
            render_bytes: 0,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            crate::prepare(&scene, &request, &store, &limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit,
            "warm store: {limits:?}"
        );
    }
}

#[test]
fn graphics_soft_mask_errors_precede_stencil_decoder_errors() {
    for name in [
        "colored-aligned",
        "colored-interpolate",
        "shading-interpolate",
    ] {
        stencil_mask_error_precedence(name);
    }
}

fn stencil_mask_error_precedence(name: &str) {
    use crate::testing::Build;
    use pagura_color::Components;
    use pagura_interpret::{GroupAttributes, MaskKind, Node};

    let mut build = Build::new(16.0, 16.0);
    build.scene = scene(name);
    let (mask, _) = build.mask(MaskKind::Alpha, GroupAttributes::default(), None);
    build.scene.masks[0].backdrop = Some(Components::from_slice(&[0.0, 0.0]));
    let state = build.state(|state| state.soft_mask = Some(mask));
    for node in &mut build.scene.nodes {
        if let Node::Image(image) = node {
            image.mark.state = state;
        }
    }
    let scene = &build.scene;
    let request = RenderRequest::page(&scene.page, 1.0).unwrap();
    let limits = Limits {
        decoded_bytes: 0,
        ..Limits::default()
    };
    let error = crate::prepare(scene, &request, &Store::new(0), &limits).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(
        error.message,
        "soft-mask backdrop has the wrong component count"
    );
}

#[test]
fn patterned_stencil_preserves_cmyk_zero_colorants_during_overprint() {
    stencil_overprint("colored-matched-stencil");
}

#[test]
fn interpolated_stencil_preserves_cmyk_zero_colorants_during_overprint() {
    stencil_overprint("interpolate-matched-colored-pattern");
}

fn stencil_overprint(name: &str) {
    use crate::testing::{Build, mark};
    use pagura_color::ColorSpace;
    use pagura_interpret::{GroupId, Paint, StateId};

    let mut build = Build::new(16.0, 16.0);
    build.scene = scene(name);
    build.simulate = true;
    for state in &mut build.scene.states {
        let mut value = (**state).clone();
        value.overprint.enabled = true;
        value.overprint.nonzero = true;
        *state = Arc::new(value);
    }
    for paint in &mut build.scene.paints {
        if let Paint::Solid(color) = &**paint
            && matches!(*color.space, ColorSpace::DeviceRgb)
        {
            let mut color = color.clone();
            color.space = Arc::new(ColorSpace::DeviceCmyk);
            color.components = pagura_color::Components::from_slice(&[0.0, 1.0, 1.0, 0.0]);
            *paint = Arc::new(Paint::Solid(color));
        }
    }
    let cyan = build.color(ColorSpace::DeviceCmyk, &[1.0, 0.0, 0.0, 0.0]);
    build.rect(GroupId(0), [0.0, 0.0, 16.0, 16.0], cyan, mark(StateId(0)));
    build.scene.groups[0].children.rotate_right(1);
    let mut request = RenderRequest::page(&build.scene.page, 1.0).unwrap();
    request.overprint = crate::OverprintMode::Simulate;
    let actual = build.render(&request).unwrap();
    let expected = fixture("paint_stencils", &format!("{name}.ppm"));
    let expected = &expected[b"P6\n16 16\n255\n".len()..];
    for (actual, red) in actual
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<3>().0.iter())
    {
        // C stays at 1 under OPM1; stencil ink adds M/Y. The independent
        // red oracle gives the M/Y coverage, so only its red channel changes.
        assert_eq!(actual[0], 0);
        assert!(actual[1].abs_diff(red[1]) <= 1 && actual[2].abs_diff(red[2]) <= 1);
        assert_eq!(actual[3], 255);
    }
}

#[test]
fn stencil_clip_is_invariant_across_bands_regions_and_warm_stores() {
    stencil_invariance("paint_stencils", "colored-fractional-clip");
}

#[test]
fn bilinear_stencil_is_invariant_across_bands_regions_and_warm_stores() {
    for name in [
        "colored-interpolate-fractional-clip",
        "shading-interpolate-fractional-clip",
        "colored-different-interpolated-masks",
        "colored-interpolate-knockout-ais-false",
        "shading-interpolate-gradient-transform",
        "shading-interpolate-devicen-gradient-overprint-multiply",
        "tiling-interpolate-separation-constant-overprint-screen",
    ] {
        stencil_invariance("paint_stencils", name);
    }
}

fn stencil_invariance(area: &str, name: &str) {
    let scene = scene_in(area, name);
    let mut whole = RenderRequest::page(&scene.page, 1.0).unwrap();
    if scene.print_intent {
        whole.overprint = crate::OverprintMode::Simulate;
    }
    let mut expected = Vec::new();
    let store = Store::new(1 << 20);
    let prepared = crate::prepare(&scene, &whole, &store, &Limits::default()).unwrap();
    prepared
        .with_scratch(&mut Scratch::default())
        .unwrap()
        .render(&mut expected);
    for capacity in [0, 1 << 20] {
        let store = Store::new(capacity);
        for height in [1, 3, 16, 256] {
            for region in [
                whole.region,
                DeviceRect {
                    x0: 3,
                    y0: 4,
                    x1: 13,
                    y1: 12,
                },
            ] {
                let mut request = whole;
                request.region = region;
                request.band_height = height;
                for _ in 0..2 {
                    let prepared =
                        crate::prepare(&scene, &request, &store, &Limits::default()).unwrap();
                    let mut scratch = Scratch::default();
                    let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
                    let mut actual = Vec::new();
                    rendering.render(&mut actual);
                    super::assert_within_charges(&rendering, prepared.charges());
                    let wanted: Vec<_> = (region.y0..region.y1)
                        .flat_map(|y| {
                            let start = usize::try_from((y * 16 + region.x0) * 4).unwrap();
                            let end = start + usize::try_from(region.width() * 4).unwrap();
                            expected[start..end].iter().copied()
                        })
                        .collect();
                    assert_eq!(actual, wanted, "height {height}, region {region:?}");
                }
            }
        }
    }
}

#[test]
fn bilinear_stencil_reserves_source_group_work_and_bytes_before_rendering() {
    for name in [
        "colored-interpolate",
        "uncolored-interpolate",
        "shading-interpolate",
    ] {
        let scene = scene(name);
        let mut request = RenderRequest::page(&scene.page, 1.0).unwrap();
        if scene.print_intent {
            request.overprint = crate::OverprintMode::Simulate;
        }
        for capacity in [0, 1 << 20] {
            let store = Store::new(capacity);
            crate::prepare(&scene, &request, &store, &Limits::default()).unwrap();
            for limits in [
                Limits {
                    path_segments: 4,
                    ..Limits::default()
                },
                Limits {
                    display_commands: 1,
                    ..Limits::default()
                },
                Limits {
                    render_work: 0,
                    ..Limits::default()
                },
                Limits {
                    render_bytes: 0,
                    ..Limits::default()
                },
            ] {
                assert_eq!(
                    crate::prepare(&scene, &request, &store, &limits)
                        .unwrap_err()
                        .kind,
                    ErrorKind::Limit,
                    "{name}: capacity {capacity}: {limits:?}"
                );
            }
        }
        assert_eq!(
            crate::prepare(
                &scene,
                &request,
                &Store::new(0),
                &Limits {
                    decoded_bytes: 0,
                    ..Limits::default()
                }
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit,
            "{name}: cold decode budget"
        );
    }
}

#[test]
fn nested_stencil_is_invariant_across_bands_regions_and_warm_stores() {
    for name in ["fill-eight-levels", "stroke-hairline"] {
        stencil_invariance("paint_nested_stencils", name);
    }
}

#[test]
fn nested_stencil_geometry_has_bounded_ancestor_chains() {
    for name in ["fill-three-levels", "stroke-butt"] {
        let scene = scene_in("paint_nested_stencils", name);
        let request = RenderRequest::page(&scene.page, 1.0).unwrap();
        for capacity in [0, 1 << 20] {
            let store = Store::new(capacity);
            crate::prepare(&scene, &request, &store, &Limits::default()).unwrap();
            for limits in [
                Limits {
                    mask_sources: 2,
                    ..Limits::default()
                },
                Limits {
                    render_work: 0,
                    ..Limits::default()
                },
                Limits {
                    render_bytes: 0,
                    ..Limits::default()
                },
            ] {
                let error = crate::prepare(&scene, &request, &store, &limits).unwrap_err();
                assert_eq!(error.kind, ErrorKind::Limit, "{name}: {limits:?}: {error}");
            }
        }
    }
}
