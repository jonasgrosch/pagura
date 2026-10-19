//! Integration evidence for the vector pipeline.

use crate::{RenderOptions, Tile, compile_page};
use pagura_core::{ErrorKind, Position, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_raster::Rgba8;

fn open(name: &str) -> Document {
    Document::open(
        pagura_testkit::fixture("content", &format!("{name}.pdf")),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap()
}

const RENDER: &[&str] = &[
    "knockout-opaque-rounding",
    "pattern",
    "shading",
    "annotation-flags",
    "DeviceN-duplicate-None",
    "stroke-adjusted-open-oblique",
    "dash-hairline-anisotropic-expanded",
    "dash-width-anisotropic-expanded",
    "icc-range",
    "icc-gray-media-white",
    "icc-gray-lab-pcs",
    "icc-mft2-4-Lab-v4",
    "icc-mft1-4-Lab-v4",
    "icc-mAB-full-Lab-2",
    "icc-intent-state",
    "icc-indexed-Lab-input",
    "icc-image-mft2",
    "icc-image-mft1",
    "icc-image-mAB",
    "icc-rgb-columns",
    "stitch-endpoint-encode-0",
    "stitch-endpoint-encode-1",
    "intent-RelativeColorimetric",
    "intent-AbsoluteColorimetric",
    "parameter-cap-low",
    "parameter-cap-high",
    "parameter-alpha-high",
    "marked-OC-other",
    "dash-width-expanded",
    "dash-hairline-rotated-expanded",
    "dash-width-rotated-expanded",
    "dash-hairline-oblique",
    "dash-phase-large-negative",
    "parameter-width-clipped",
    "parameter-miter-clipped",
    "parameter-flatness-high",
    "parameter-flatness-negative",
    "intent-unknown",
    "point-fill",
    "point-clip",
    "stroke-closed-point",
    "stroke-closed-duplicates",
    "stroke-square-point",
    "stroke-open-point",
    "stroke-adjusted-open-point",
    "space-array-DeviceGray",
    "space-array-DeviceRGB",
    "space-array-DeviceCMYK",
    "default-underlying-indexed",
    "default-underlying-tint",
    "stitch-endpoint-0",
    "stitch-endpoint-1",
    "membership-missing",
    "membership-empty",
    "membership-null",
    "membership-intent-AnyOn",
    "membership-intent-AllOn",
    "membership-intent-AnyOff",
    "membership-intent-AllOff",
    "form-hidden-body",
    "icc-initial-zero",
    "icc-1-identity",
    "icc-1-gamma",
    "icc-1-sampled",
    "icc-1-param0",
    "icc-1-param1",
    "icc-1-param2",
    "icc-1-param3",
    "icc-1-param4",
    "icc-3-identity",
    "icc-3-gamma",
    "icc-3-sampled",
    "icc-3-param0",
    "icc-3-param1",
    "icc-3-param2",
    "icc-3-param3",
    "icc-3-param4",
    "dash-hairline-scaled",
    "dash-hairline-expanded",
    "dash-hairline-rotated",
    "dash-hairline-anisotropic",
    "dash-width-scaled",
    "dash-width-rotated",
    "dash-width-anisotropic",
    "dash-phase-negative",
    "dash-phase-normalized",
    "dash-phase-cycle",
    "dash-odd-negative",
    "dash-odd-normalized",
    "dash-odd-cycle",
    "dash-empty-negative",
    "form-empty-group",
    "alpha-accumulation",
    "group-incoming-alpha",
    "soft-frozen-transform",
    "soft-crop-once",
    "group-alpha-shape-0",
    "group-alpha-shape-1",
    "annotation-sheared",
    "annotation-state-isolation",
    "annotation-link-borderless",
    "blend-isolated-page",
    "shading-axial",
    "shading-axial-clip",
    "shading-background-ignored",
    "shading-axial-extend",
    "shading-axial-empty",
    "shading-filter",
    "shading-radial",
    "shading-radial-extend",
    "shading-radial-reverse",
    "shading-cone",
    "shading-linear-cone",
    "shading-function",
    "pattern-shading",
    "pattern-anchored",
    "pattern-stroke",
    "pattern-background",
    "pattern-initial",
    "alpha",
    "alpha-stroke",
    "alpha-combined",
    "blend",
    "knockout",
    "group-DeviceGray",
    "group-DeviceCMYK",
    "group-colour-DeviceGray",
    "group-colour-DeviceCMYK",
    "halftone-continuous",
    "cie-CalRGB",
    "cie-CalRGB-matrix",
    "annotation-rotated",
    "annotation-reversed-rect",
    "annotation-print-flag",
    "annotation-no-print-flag",
    "annotation-no-view",
    "annotation-styling-ignored",
    "annotation-state",
    "annotation-undefined-state",
    "blend-Normal",
    "blend-Compatible",
    "blend-Multiply",
    "blend-Screen",
    "blend-Overlay",
    "blend-Darken",
    "blend-Lighten",
    "blend-ColorDodge",
    "blend-ColorBurn",
    "blend-HardLight",
    "blend-SoftLight",
    "blend-Difference",
    "blend-Exclusion",
    "blend-Hue",
    "blend-Saturation",
    "blend-Color",
    "blend-Luminosity",
    "group-i0-k0",
    "group-i0-k1",
    "group-i1-k0",
    "group-i1-k1",
    "soft-Alpha",
    "soft-Alpha-transfer",
    "soft-Luminosity",
    "soft-Luminosity-transfer",
    "decimal-stroke-half-tie",
    "usage-View",
    "usage-Print-ignored",
    "usage-no-OCGs",
    "usage-Zoom-on",
    "usage-Zoom-off",
    "usage-conjunction",
    "stroke-adjust-horizontal",
    "stroke-adjust-vertical",
    "function-exp-1",
    "function-exp-2",
    "function-stitch",
    "function-sampled",
    "function-calculator",
    "function-sampled-1",
    "function-sampled-2",
    "function-sampled-4",
    "function-sampled-8",
    "function-sampled-12",
    "function-sampled-16",
    "function-sampled-24",
    "function-sampled-32",
    "function-order3-small",
    "function-order3",
    "transfer-type2",
    "transfer-type4",
    "transfer-precedence",
    "transfer-array",
    "space-Separation",
    "space-DeviceN",
    "cie-CalGray",
    "cie-CalGray-gamma",
    "cie-Lab",
    "cie-indexed-Lab",
    "space-CalGray",
    "space-Lab",
    "rgb-page-group",
    "rgb-form-group",
    "icc-rgb-group",
    "rgb-device-hints",
    "compound-path",
    "nonzero",
    "ctm-during-path",
    "clip-after-paint",
    "clip-q-Q",
    "clip-is-not-restored-path",
    "stroke-square",
    "stroke-dash",
    "stroke-nonuniform",
    "fill-stroke-close",
    "cmyk",
    "indexed",
    "icc-alternate",
    "icc-default",
    "default-space",
    "compound-operands-split",
    "compatibility",
    "extgstate",
    "opaque-page-group",
    "rotate-90",
    "rotate-180",
    "rotate-270",
    "crop",
    "user-unit",
    "fractional",
    "fractional-clip",
    "fractional-clip-intersection",
    "form",
    "opaque-form-group",
    "form-resources",
    "optional-default",
    "optional-AnyOn",
    "optional-AllOn",
    "optional-AnyOff",
    "optional-AllOff",
    "optional-expression",
    "annotation",
    "space-CalRGB",
    "annotation-no-normal",
    "annotation-hidden",
];

#[test]
fn independent_content_pixels() {
    for &name in RENDER {
        let document = open(name);
        let list = compile_page(&document, document.page(0).unwrap())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let target = list
            .render(&RenderOptions::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let actual = pagura_testkit::ppm(
            target.width(),
            target.height(),
            &target.to_rgb_over(Rgba8::WHITE),
        );
        let expected = pagura_testkit::fixture("content", &format!("{name}.ppm"));
        assert_eq!(actual.len(), expected.len(), "{name}");
        for (i, (&a, &e)) in actual.iter().zip(&expected).enumerate() {
            assert!(
                a.abs_diff(e)
                    <= if name.starts_with("stroke-closed-") {
                        3
                    } else {
                        u8::from(
                            name == "knockout-opaque-rounding"
                                || name.starts_with("dash-")
                                || name.starts_with("fractional")
                                || name.starts_with("icc-")
                                || name.starts_with("default-underlying-")
                                || name.starts_with("stroke-closed-")
                                || name.starts_with("cie-")
                                || name == "annotation-sheared"
                                || name.starts_with("shading-")
                                || name.starts_with("pattern-")
                                || name.starts_with("blend-")
                                || name.starts_with("group-")
                                || name.starts_with("soft-")
                                || name.starts_with("function-")
                                || name == "decimal-stroke-half-tie",
                        )
                    },
                "{name} byte {i}: {a} vs {e}"
            );
        }
    }
}

#[test]
fn all_supported_fixtures_assemble_as_single_pixel_tiles() {
    for &name in RENDER {
        let document = open(name);
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let options = RenderOptions {
            scale: 1.25,
            background: Rgba8::new(17, 35, 61, 96),
            ..RenderOptions::default()
        };
        let full = list.render(&options).unwrap();
        for y in 0..full.height() {
            for x in 0..full.width() {
                let tile = list
                    .render_tile(
                        &options,
                        Tile {
                            x,
                            y,
                            width: 1,
                            height: 1,
                        },
                    )
                    .unwrap();
                let start = usize::try_from(x).unwrap() * 4;
                assert_eq!(
                    tile.data(),
                    &full.row(y).unwrap()[start..start + 4],
                    "{name} ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn unsupported_colour_reports_its_stream_and_object() {
    let document = open("cie-blackpoint");
    let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
    assert_eq!(
        error.feature().map(pagura_core::Feature::as_str),
        Some("colorspace.BlackPoint")
    );
    assert!(matches!(error.position, Position::Stream { .. }), "{error}");
    assert!(error.object.is_some());
}

#[test]
fn unsupported_forward_icc_transforms_identify_the_profile() {
    for name in ["icc-lut-mBA", "icc-lut8-XYZ"] {
        let document = open(name);
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let error = list.render(&RenderOptions::default()).unwrap_err();
        assert_eq!(
            error.feature().map(pagura_core::Feature::as_str),
            Some("colorspace.ICCBased")
        );
        assert_eq!(error.object, Some(pagura_core::ObjectId::new(5, 0)));
    }
}

#[test]
fn annex_a_validation_precedes_unsupported_and_unknown_is_syntax() {
    use pagura_core::Value;
    for (operator, operands) in [
        (b"Tj".as_slice(), vec![Value::Integer(1)]),
        (b"Tf", vec![Value::name(b"F")]),
        (b"cm", vec![]),
        (b"q", vec![Value::Integer(1)]),
        (b"d", vec![Value::Integer(1), Value::Integer(1)]),
    ] {
        assert_eq!(
            crate::operators::validate(operator, &operands)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    assert_eq!(
        crate::operators::validate(b"Tj", &[Value::String(vec![])]).unwrap(),
        Some(crate::operators::Implemented)
    );
    assert!(
        crate::operators::validate(b"future", &[])
            .unwrap()
            .is_none()
    );
}

#[test]
fn raster_and_interpreter_limits_and_invalid_arguments() {
    for limits in [
        pagura_core::Limits {
            path_segments: 0,
            ..pagura_core::Limits::default()
        },
        pagura_core::Limits {
            display_commands: 0,
            ..pagura_core::Limits::default()
        },
    ] {
        let document = Document::open(
            pagura_testkit::fixture("content", "compound-path.pdf"),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            compile_page(&document, document.page(0).unwrap())
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
    let document = Document::open(
        pagura_testkit::fixture("content", "compound-path.pdf"),
        &OpenOptions {
            limits: pagura_core::Limits {
                operations: 2,
                ..pagura_core::Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        compile_page(&document, document.page(0).unwrap())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let document = open("fractional");
    let list = compile_page(&document, document.page(0).unwrap()).unwrap();
    assert_eq!(
        list.device_size(&RenderOptions::from_dpi(f64::INFINITY))
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        list.device_size(&RenderOptions::from_dpi(f64::MAX))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert_eq!(
        list.render_tile(
            &RenderOptions::default(),
            Tile {
                x: u32::MAX,
                y: 0,
                width: 1,
                height: 1
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn legacy_aligned_oracles_also_match_the_vector_pipeline() {
    for name in [
        "blank",
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
    ] {
        let document = Document::open(
            pagura_testkit::fixture("classic", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let target = list.render(&RenderOptions::default()).unwrap();
        let actual = pagura_testkit::ppm(
            target.width(),
            target.height(),
            &target.to_rgb_over(Rgba8::WHITE),
        );
        assert!(
            actual == pagura_testkit::fixture("classic", &format!("{name}.ppm")),
            "{name}"
        );
    }
}

#[test]
fn malformed_content_and_form_cycles_are_precise_errors() {
    for name in [
        "transfer",
        "soft-mask",
        "syntax-BG-default",
        "syntax-UCR-default",
        "syntax-Lab-arity",
        "syntax-unknown",
        "syntax-arity",
        "syntax-type",
        "syntax-Q",
        "syntax-q-unclosed",
        "syntax-l",
        "syntax-TJ",
        "syntax-pending-clip",
        "syntax-reference",
        "syntax-zero-dash",
    ] {
        let document = open(name);
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
        assert!(
            matches!(error.position, Position::Stream { .. }),
            "{name}: {error}"
        );
    }
    let document = open("recursive-form");
    assert_eq!(
        compile_page(&document, document.page(0).unwrap())
            .unwrap_err()
            .kind,
        ErrorKind::Cycle
    );
}

#[test]
fn invalid_function_graphs_and_evaluation_errors_preserve_context() {
    for (name, kind) in [
        ("function-truncated", ErrorKind::Syntax),
        ("function-dimension", ErrorKind::Syntax),
        ("function-host-action", ErrorKind::Syntax),
        ("function-undefined", ErrorKind::Syntax),
        ("function-cycle", ErrorKind::Cycle),
    ] {
        let document = open(name);
        let error = compile_page(&document, document.page(0).unwrap())
            .and_then(|list| list.render(&RenderOptions::default()))
            .unwrap_err();
        assert_eq!(error.kind, kind, "{name}: {error}");
        assert_eq!(
            error.object,
            Some(pagura_core::ObjectId::new(5, 0)),
            "{name}: {error}"
        );
    }
}

#[test]
fn decimal_stroke_half_tie_has_independent_half_up_rounding() {
    let document = open("decimal-stroke-half-tie");
    let image = compile_page(&document, document.page(0).unwrap())
        .unwrap()
        .render(&RenderOptions::default())
        .unwrap();
    // A width-six user pen under .06 scaling covers exactly .30 of this row.
    // round(.30 * 255) = 77, hence black over white is RGB 178.
    let row = image.row(4).unwrap();
    for x in 2..6 {
        assert_eq!(&row[x * 4..x * 4 + 4], &[178, 178, 178, 255]);
    }
}

#[test]
fn invalid_appearances_masks_and_shadings_have_precise_errors() {
    for (name, kind) in [
        ("annotation-invalid-state", ErrorKind::Syntax),
        ("annotation-singular-matrix", ErrorKind::Syntax),
        ("soft-invalid-backdrop", ErrorKind::Syntax),
        ("soft-missing-space", ErrorKind::Syntax),
        ("soft-recursive", ErrorKind::Cycle),
        ("shading-negative-radius", ErrorKind::Syntax),
        ("shading-function-domain", ErrorKind::Syntax),
    ] {
        let document = open(name);
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        assert_eq!(error.kind, kind, "{name}: {error}");
        assert!(error.object.is_some(), "{name}: {error}");
    }
}

#[test]
fn compositing_memory_work_and_shared_function_budgets_are_bounded() {
    for (name, limits) in [
        (
            "group-i1-k0",
            pagura_core::Limits {
                decoded_bytes: 1000,
                ..pagura_core::Limits::default()
            },
        ),
        (
            "shading-axial",
            pagura_core::Limits {
                operations: 2,
                ..pagura_core::Limits::default()
            },
        ),
    ] {
        let document = Document::open(
            pagura_testkit::fixture("content", &format!("{name}.pdf")),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let error = list.render(&RenderOptions::default()).expect_err(name);
        assert_eq!(error.kind, ErrorKind::Limit, "{name}: {error}");
    }
    // Preparation and rendering each enforce their page work budget.
    let document = Document::open(
        pagura_testkit::fixture("content", "group-i1-k0.pdf"),
        &OpenOptions {
            limits: pagura_core::Limits {
                raster_work: 1,
                ..pagura_core::Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert!(error.object.is_some());
    let document = open("group-i1-k0");
    let mut list = compile_page(&document, document.page(0).unwrap()).unwrap();
    list.limits.raster_work = 1;
    assert_eq!(
        list.render(&RenderOptions::default()).unwrap_err().kind,
        ErrorKind::Limit
    );
    // The F32 page now fits the former F64-sized threshold. Keep a
    // successful admission beside the smaller-budget rejection above.
    let document = Document::open(
        pagura_testkit::fixture("content", "group-i1-k0.pdf"),
        &OpenOptions {
            limits: pagura_core::Limits {
                decoded_bytes: 3600,
                ..pagura_core::Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    compile_page(&document, document.page(0).unwrap())
        .unwrap()
        .render(&RenderOptions::default())
        .unwrap();
    for name in ["group-i1-k0", "soft-Alpha", "annotation"] {
        let document = Document::open(
            pagura_testkit::fixture("content", &format!("{name}.pdf")),
            &OpenOptions {
                limits: pagura_core::Limits {
                    display_commands: 0,
                    ..pagura_core::Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{name}: {error}");
    }
}

#[test]
fn empty_group_has_no_painted_marks() {
    let document = open("form-empty-group");
    let list = compile_page(&document, document.page(0).unwrap()).unwrap();
    assert_eq!(list.len(), 0);
    assert!(list.is_empty());
}

#[test]
fn equivalent_dash_spaces_and_phases_match_exactly() {
    for (a, b) in [
        ("dash-hairline-scaled", "dash-hairline-expanded"),
        ("dash-width-scaled", "dash-width-expanded"),
        ("dash-hairline-rotated", "dash-hairline-rotated-expanded"),
        ("dash-width-rotated", "dash-width-rotated-expanded"),
        (
            "dash-hairline-anisotropic",
            "dash-hairline-anisotropic-expanded",
        ),
        ("dash-width-anisotropic", "dash-width-anisotropic-expanded"),
        ("dash-phase-negative", "dash-phase-normalized"),
        ("dash-phase-negative", "dash-phase-cycle"),
        ("dash-phase-negative", "dash-phase-large-negative"),
        ("dash-odd-negative", "dash-odd-normalized"),
        ("dash-odd-negative", "dash-odd-cycle"),
    ] {
        let render = |name| {
            let d = open(name);
            compile_page(&d, d.page(0).unwrap())
                .unwrap()
                .render(&RenderOptions::default())
                .unwrap()
        };
        assert_eq!(render(a).data(), render(b).data(), "{a} vs {b}");
    }
}

#[test]
fn required_path_colour_and_configuration_validation() {
    for name in [
        "path-undefined-stroke",
        "path-undefined-close-stroke",
        "path-undefined-fill",
        "path-undefined-fill-alias",
        "path-undefined-parity-fill",
        "path-undefined-fill-stroke",
        "path-undefined-parity-fill-stroke",
        "path-undefined-close-fill-stroke",
        "path-undefined-close-parity-fill-stroke",
        "path-undefined-end",
        "path-construction-after-clip",
        "default-illegal-indexed",
        "default-illegal-Lab",
        "space-nested-alias",
        "indexed-excess",
        "tint-simple-operator",
        "DeviceN-forbidden-All",
        "DeviceN-All-after-None",
        "function-domain-negative",
        "function-domain-fractional",
        "configuration-invalid-base-off",
        "configuration-invalid-intent-empty",
        "icc-simple-operator",
        "icc-invalid-data",
    ] {
        let d = open(name);
        let error = compile_page(&d, d.page(0).unwrap()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
        assert!(error.object.is_some(), "{name}: {error}");
    }
}

#[test]
fn mesh_pattern_and_adjusted_geometry_match_independent_oracles() {
    for name in [
        "adjust-oblique",
        "adjust-affine-pen",
        "adjust-arch",
        "tiling-coloured",
        "tiling-underlying",
        "tiling-ignored-colour",
        "tiling-ignored-inline",
        "tiling-alpha-once",
        "tiling-blend",
        "mesh-triangle",
        "mesh-lattice",
        "mesh-patch-6",
        "mesh-patch-7",
        "mesh-coons-curved",
        "mesh-tensor-interior",
        "mesh-folded",
        "mesh-patch-reuse-6-1",
        "mesh-patch-reuse-7-3",
        "annotation-icon-Note",
        "annotation-icon-Key",
        "annotation-link-border",
        "ink-All",
        "ink-None-knockout",
        "ink-All-cmyk",
        "ink-DeviceN-mixed",
    ] {
        let document = open(name);
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let pixels = list
            .render(&RenderOptions::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let actual = pagura_testkit::ppm(
            pixels.width(),
            pixels.height(),
            &pixels.to_rgb_over(Rgba8::WHITE),
        );
        let expected = pagura_testkit::fixture("content", &format!("{name}.ppm"));
        let tolerance = if name == "adjust-arch" || name.starts_with("annotation-icon-") {
            3
        } else {
            1
        };
        assert_eq!(actual.len(), expected.len(), "{name}");
        for (index, (&a, &b)) in actual.iter().zip(&expected).enumerate() {
            assert!(
                a.abs_diff(b) <= tolerance,
                "{name} byte {index}: {a} vs {b}"
            );
        }
    }
}

#[test]
fn mesh_patterns_and_view_fitting_preserve_global_tile_coordinates() {
    for name in [
        "adjust-arch",
        "tiling-coloured",
        "tiling-alpha-once",
        "tiling-blend",
        "mesh-folded",
        "mesh-tensor-interior",
        "annotation-view-24-72-90",
    ] {
        let document = open(name);
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let options = RenderOptions {
            scale: 1.25,
            background: Rgba8::new(17, 35, 61, 96),
            ..RenderOptions::default()
        };
        let full = list.render(&options).unwrap();
        for y in 0..full.height() {
            for x in 0..full.width() {
                let piece = list
                    .render_tile(
                        &options,
                        Tile {
                            x,
                            y,
                            width: 1,
                            height: 1,
                        },
                    )
                    .unwrap();
                let offset = usize::try_from((y * full.width() + x) * 4).unwrap();
                assert_eq!(
                    piece.data(),
                    &full.data()[offset..offset + 4],
                    "{name} at {x},{y}"
                );
            }
        }
    }
}

#[test]
fn group_bounds_and_page_chunks_fit_a_small_surface_budget() {
    let document = Document::open(
        pagura_testkit::fixture("content", "group-small-clip.pdf"),
        &OpenOptions {
            limits: pagura_core::Limits {
                decoded_bytes: 4 * 1024 * 1024,
                ..Default::default()
            },
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let list = compile_page(&document, document.page(0).unwrap()).unwrap();
    let options = RenderOptions::default();
    let full = list.render(&options).unwrap();
    let actual = pagura_testkit::ppm(full.width(), full.height(), &full.to_rgb_over(Rgba8::WHITE));
    pagura_testkit::assert_bytes_eq(
        &actual,
        &pagura_testkit::fixture("content", "group-small-clip.ppm"),
        "bounded group",
    );
    let tile = Tile {
        x: 253,
        y: 253,
        width: 6,
        height: 6,
    };
    let piece = list.render_tile(&options, tile).unwrap();
    for y in 0..tile.height {
        for x in 0..tile.width {
            let offset = usize::try_from(((tile.y + y) * full.width() + tile.x + x) * 4).unwrap();
            let local = usize::try_from((y * tile.width + x) * 4).unwrap();
            assert_eq!(
                &piece.data()[local..local + 4],
                &full.data()[offset..offset + 4]
            );
        }
    }
}

#[test]
fn empty_fill_boundaries_match_oracles_and_device_tiles() {
    for (name, tolerance) in [
        ("zero-width-rectangle", 0),
        ("zero-width-rectangle-reversed", 0),
        ("zero-height-rectangle", 0),
        ("zero-height-rectangle-reversed", 0),
        ("integral-boundary", 0),
        ("collinear-triangle", 0),
        ("oblique-collinear-triangle", 1),
        ("open-segment-evenodd", 0),
        ("collinear-cubic", 0),
        ("singular-transform", 0),
        ("multiple-subpaths", 0),
        ("point-and-segment", 0),
        ("point-pair-evenodd", 0),
        ("point-pair-nonzero", 0),
        ("half-alpha", 0),
        ("trailing-move", 0),
        ("stroke-state-independent", 0),
        ("degenerate-clip", 0),
        ("degenerate-clip-evenodd", 0),
        ("zero-area-clip-content", 0),
        ("zero-width-pattern-bbox", 0),
        ("zero-size-pattern-bbox", 0),
        ("zero-width-shading-bbox", 0),
        ("zero-width-shading-pattern-bbox", 0),
        ("zero-width-shading-pattern-background", 0),
        ("zero-size-shading-bbox", 0),
        ("zero-size-shading-pattern-bbox", 0),
        ("zero-size-shading-pattern-background", 0),
        ("paint-before-clip", 0),
        ("hairline-clipped-geometrically", 0),
        ("cancelled-evenodd", 0),
        ("cancelled-nonzero-subdivided", 0),
        ("positive-sliver", 0),
        ("tiny-nonpoint-curve", 0),
        ("positive-bow-tie", 0),
        ("transparent-compositor", 0),
    ] {
        let document = Document::open(
            pagura_testkit::fixture("zero_area", &format!("{name}.pdf")),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let options = RenderOptions::default();
        let full = list.render(&options).unwrap();
        let actual =
            pagura_testkit::ppm(full.width(), full.height(), &full.to_rgb_over(Rgba8::WHITE));
        let expected = pagura_testkit::fixture("zero_area", &format!("{name}.ppm"));
        assert_eq!(actual.len(), expected.len(), "{name}");
        for (i, (&a, &e)) in actual.iter().zip(&expected).enumerate() {
            assert!(a.abs_diff(e) <= tolerance, "{name} byte {i}: {a} vs {e}");
        }
        for y in 0..8 {
            for x in 0..8 {
                let tile = list
                    .render_tile(
                        &options,
                        Tile {
                            x,
                            y,
                            width: 1,
                            height: 1,
                        },
                    )
                    .unwrap();
                let i = usize::try_from((y * 8 + x) * 4).unwrap();
                assert_eq!(tile.data(), &full.data()[i..i + 4], "{name} ({x},{y})");
            }
        }
    }
}

#[test]
fn image_tiles_preserve_global_coordinates_at_large_scales() {
    for turn in 0..4 {
        let document = Document::open(
            pagura_testkit::fixture("paint", &format!("image-quarter-turn-{turn}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let options = RenderOptions {
            scale: 41.1,
            background: Rgba8::new(17, 35, 61, 96),
            ..RenderOptions::default()
        };
        let full = list.render(&options).unwrap();
        for y in (0..full.height()).step_by(133) {
            for x in (0..full.width()).step_by(197) {
                let tile = Tile {
                    x,
                    y,
                    width: (full.width() - x).min(197),
                    height: (full.height() - y).min(133),
                };
                let piece = list.render_tile(&options, tile).unwrap();
                for row in 0..tile.height {
                    let start = usize::try_from(((y + row) * full.width() + x) * 4).unwrap();
                    let local = usize::try_from(row * tile.width * 4).unwrap();
                    let bytes = usize::try_from(tile.width * 4).unwrap();
                    assert_eq!(
                        &piece.data()[local..local + bytes],
                        &full.data()[start..start + bytes],
                        "quarter turn {turn}, at {x},{y}, row {row}"
                    );
                }
            }
        }
    }
}

#[test]
fn image_reduction_half_levels_choose_a_nearest_level() {
    // The independently generated 2x2 means are 3.5, 5.5, 7.5, 15.5,
    // 17.5 and 19.5. Colour-key replacement makes only the first mean 67.25.
    for name in [
        "image-area-separable",
        "image-area-separable-colour-key",
        "image-area-tie-1024-159",
    ] {
        let document = Document::open(
            pagura_testkit::fixture("paint", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let list = compile_page(&document, document.page(0).unwrap()).unwrap();
        let output = list.render(&RenderOptions::default()).unwrap();
        let actual = output.to_rgb_over(Rgba8::WHITE);
        let oracle = pagura_testkit::fixture("paint", &format!("{name}.ppm"));
        let expected = oracle.splitn(4, |&b| b == b'\n').nth(3).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (&a, &e) in actual.iter().zip(expected) {
            let halfway = if name == "image-area-tie-1024-159" {
                true
            } else {
                matches!(e, 4 | 6 | 8 | 16 | 18 | 20)
            };
            if halfway {
                assert!(a == e || a == e - 1, "{name}: {a} near {e}-0.5");
            } else {
                assert_eq!(a, e, "{name}: exact non-half level");
            }
        }
    }
}

/// A one-page PDF drawing `jpeg` (`width` by `height` DeviceRGB samples) by
/// the content `placement`, on a `side` by `side` point page.
fn jpeg_page(jpeg: &[u8], (width, height): (u32, u32), side: u32, placement: &str) -> Vec<u8> {
    let content = format!("q {placement} cm /Im Do Q");
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {side} {side}] \
             /Resources << /XObject << /Im 4 0 R >> >> /Contents 5 0 R >>"
        )
        .into_bytes(),
    ];
    let mut image = format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
        jpeg.len()
    )
    .into_bytes();
    image.extend_from_slice(jpeg);
    image.extend_from_slice(b"\nendstream");
    objects.push(image);
    objects.push(
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
        .into_bytes(),
    );
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

#[test]
fn drawn_size_jpeg_is_approximate_and_switchable_to_the_exact_path() {
    use crate::Accuracy;
    let jpeg = pagura_testkit::fixture("image", "jpeg-interleaved-420.in");
    // 17 samples over 4.25 device pixels keep four cells per block axis,
    // over 8.5 pixels all eight; a quarter turn and a flip change nothing.
    for (placement, reduced) in [
        ("4.25 0 0 4.25 0.5 0.25", true),
        ("0 4.25 -4.25 0 5 0.5", true),
        ("8.5 0 0 -8.5 0.25 9", false),
    ] {
        let document = Document::open(
            jpeg_page(&jpeg, (17, 17), 10, placement),
            &OpenOptions::default(),
        )
        .unwrap();
        let page = document.page(0).unwrap();
        let exact = RenderOptions::default();
        let drawn = RenderOptions {
            accuracy: Accuracy::RowFragments,
            ..RenderOptions::default()
        };
        let difference = |a: &pagura_raster::Pixmap, b: &pagura_raster::Pixmap| {
            a.data()
                .iter()
                .zip(b.data())
                .map(|(&a, &b)| a.abs_diff(b))
                .max()
                .unwrap()
        };
        // A separately compiled list keeps every sample.
        let unreduced = compile_page(&document, page)
            .unwrap()
            .render(&drawn)
            .unwrap();
        let reference = crate::render_page(&document, page, &exact).unwrap();
        let approximate = crate::render_page(&document, page, &drawn).unwrap();
        let cells = difference(&approximate, &unreduced);
        assert_eq!(cells > 0, reduced, "{placement}: {cells}");
        let worst = difference(&approximate, &reference);
        assert!(worst <= 24, "{placement}: {worst}");
        let context = crate::RenderContext::new(std::sync::Arc::new(
            Document::open(
                jpeg_page(&jpeg, (17, 17), 10, placement),
                &OpenOptions::default(),
            )
            .unwrap(),
        ));
        let shared = context.document().page(0).unwrap();
        assert_eq!(
            context.render_page(shared, &drawn).unwrap().data(),
            approximate.data()
        );
        assert_eq!(
            context.render_page(shared, &exact).unwrap().data(),
            reference.data()
        );
        for y in 0..approximate.height() {
            for x in 0..approximate.width() {
                let tile = Tile {
                    x,
                    y,
                    width: 1,
                    height: 1,
                };
                let pixel = crate::render_tile(&document, page, &drawn, tile).unwrap();
                let i = usize::try_from((y * approximate.width() + x) * 4).unwrap();
                assert_eq!(
                    pixel.data(),
                    &approximate.data()[i..i + 4],
                    "{placement} ({x},{y})"
                );
            }
        }
    }
}
