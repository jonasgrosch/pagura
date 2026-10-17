//! Integration evidence for the additive vector pipeline.

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
                a.abs_diff(e) <= u8::from(name.starts_with("fractional")),
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
        ("text", "operator.Tj"),
        ("inline-image", "image.inline"),
        ("shading", "shading.paint"),
        ("text-state", "operator.Tf"),
        ("type3", "operator.d0"),
        ("alpha", "transparency.ca"),
        ("blend", "transparency.BM"),
        ("soft-mask", "transparency.SMask"),
        ("transfer", "extgstate.TR"),
        ("font-state", "extgstate.Font"),
        ("space-Separation", "colorspace.Separation"),
        ("space-DeviceN", "colorspace.DeviceN"),
        ("space-Lab", "colorspace.Lab"),
        ("space-CalRGB", "colorspace.CalRGB"),
        ("space-CalGray", "colorspace.CalGray"),
        ("pattern", "pattern.colorspace"),
        ("knockout", "transparency.knockout"),
        ("annotation", "annotation.appearance"),
        ("icc-range", "colorspace.ICCRange"),
    ] {
        let document = open(name);
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        assert_eq!(
            error.feature().map(pagura_core::Feature::as_str),
            Some(tag),
            "{name}: {error}"
        );
        if !matches!(name, "knockout" | "annotation") {
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
