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
                    <= u8::from(
                        name.starts_with("fractional")
                            || name.starts_with("cie-")
                            || name == "annotation-sheared"
                            || name.starts_with("shading-")
                            || name.starts_with("pattern-")
                            || name.starts_with("blend-")
                            || name.starts_with("group-")
                            || name.starts_with("soft-")
                            || name.starts_with("function-")
                            || name == "decimal-stroke-half-tie"
                    ),
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
fn unsupported_tags_and_stream_positions() {
    for (name, tag) in [
        ("shading", "shading.mesh"),
        ("pattern", "pattern.tiling"),
        ("annotation-flags", "annotation.flags"),
        ("icc-range", "colorspace.ICCRange"),
        ("cie-blackpoint", "colorspace.BlackPoint"),
    ] {
        let document = open(name);
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        assert_eq!(
            error.feature().map(pagura_core::Feature::as_str),
            Some(tag),
            "{name}: {error}"
        );
        if !matches!(
            name,
            "knockout" | "annotation-flags" | "group-DeviceGray" | "group-DeviceCMYK"
        ) {
            assert!(
                matches!(error.position, Position::Stream { .. }),
                "{name}: {error}"
            );
        }
        assert!(error.object.is_some());
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
        crate::operators::validate(b"Tj", &[Value::String(vec![])])
            .unwrap()
            .unwrap()
            .as_str(),
        "operator.Tj"
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
        "syntax-negative-width",
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
        (
            "function-order3",
            ErrorKind::Unsupported(crate::features::FUNCTION_CUBIC),
        ),
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
                decoded_bytes: 3600,
                ..pagura_core::Limits::default()
            },
        ),
        (
            "group-i1-k0",
            pagura_core::Limits {
                raster_work: 1,
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
