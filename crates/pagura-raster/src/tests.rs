//! Independent generated vectors through every coverage backend.

use crate::coverage::{self, Scratch, Shape};
use crate::{
    FillRule, IntRect, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Point,
    Rasterizer, Rgba8, Stroke, Transform,
};
use pagura_core::{Limits, Rect, Result};
use std::sync::Arc;

fn area(x: u32, y: u32, width: u32, height: u32) -> IntRect {
    IntRect::new(x, y, width, height).unwrap()
}

fn vector(name: &str, tolerance: u8) {
    let text =
        String::from_utf8(pagura_testkit::fixture("raster", &format!("{name}.path"))).unwrap();
    let (path, rule, stroke, clips) = vector_geometry(&text);
    let expected = pagura_testkit::fixture("raster", &format!("{name}.pgm"));
    let expected = expected.strip_prefix(b"P5\n8 8\n255\n").unwrap();
    let (path, transform, rule) = if let Some(stroke) = stroke {
        let (outline, transform) =
            crate::stroke_outline(&path, Transform::IDENTITY, &stroke, &Limits::default()).unwrap();
        let mut target = Pixmap::new(8, 8, Rgba8::TRANSPARENT, 64).unwrap();
        let full = Path::rect(Rect::from_corners(0.0, 0.0, 8.0, 8.0).unwrap());
        let limits = Limits::default();
        let clip = Mask::from_path(
            area(0, 0, 8, 8),
            full,
            Transform::IDENTITY,
            FillRule::NonZero,
            limits,
        );
        Rasterizer::default()
            .fill_path(
                &mut target,
                outline.clone(),
                transform,
                FillRule::NonZero,
                &Paint::Solid(Rgba8::BLACK),
                &clip.unwrap(),
            )
            .unwrap();
        let actual: Vec<_> = target
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .collect();
        assert_vector_samples(name, "stroke API", &actual, expected, tolerance);
        (outline, transform, FillRule::NonZero)
    } else {
        (path, Transform::IDENTITY, rule)
    };
    // Fill and clips are intersected geometrically in one call. In particular,
    // the forced backend cannot admit a short first contour to the strip sweep.
    let shapes: Vec<_> = std::iter::once(Shape {
        geometry: coverage::Geometry::Path(Arc::new(path)),
        transform,
        rule,
    })
    .chain(clips.into_iter().map(|path| Shape {
        geometry: coverage::Geometry::Path(Arc::new(path)),
        transform: Transform::IDENTITY,
        rule,
    }))
    .collect();
    let glyphs: Vec<_> = shapes
        .iter()
        .map(|shape| {
            let coverage::Geometry::Path(path) = &shape.geometry else {
                unreachable!()
            };
            let t = shape.transform;
            Shape {
                geometry: coverage::Geometry::Glyph(Arc::new(
                    crate::GlyphGeometry::new(path, t, &Limits::default(), usize::MAX).unwrap(),
                )),
                transform: Transform::translate(t.e, t.f),
                rule: shape.rule,
            }
        })
        .collect();
    for (backend, render, shapes) in [
        (
            "path dispatch",
            coverage::coverage as CoverageRenderer,
            shapes.as_slice(),
        ),
        (
            "glyph dispatch",
            coverage::coverage as CoverageRenderer,
            glyphs.as_slice(),
        ),
        (
            "glyph accumulator",
            coverage::coverage_accumulated as CoverageRenderer,
            glyphs.as_slice(),
        ),
        (
            "accumulator",
            coverage::coverage_accumulated as CoverageRenderer,
            shapes.as_slice(),
        ),
    ] {
        let mut whole = [0; 64];
        let mut scratch = Scratch::default();
        let limits = Limits::default();
        render(area(0, 0, 8, 8), shapes, &mut whole, &limits, &mut scratch).unwrap();
        assert_vector_samples(name, backend, &whole, expected, tolerance);
        assert_vector_tiles(name, backend, render, shapes, &whole, &limits, &mut scratch);
    }
}

fn vector_geometry(text: &str) -> (Path, FillRule, Option<Stroke>, Vec<Path>) {
    let mut builder = PathBuilder::new();
    let mut rule = FillRule::NonZero;
    let mut stroke_style = None;
    let mut clips = Vec::new();
    for line in text.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        let n = |i: usize| words[i].parse::<f64>().unwrap();
        match words[0] {
            "nonzero" => rule = FillRule::NonZero,
            "evenodd" => rule = FillRule::EvenOdd,
            "stroke" => {
                stroke_style = Some(Stroke {
                    width: n(1),
                    cap: match words[2] {
                        "butt" => LineCap::Butt,
                        "round" => LineCap::Round,
                        _ => LineCap::ProjectingSquare,
                    },
                    join: match words[3] {
                        "round" => LineJoin::Round,
                        "bevel" => LineJoin::Bevel,
                        _ => LineJoin::Miter,
                    },
                    miter_limit: n(4),
                    ..Stroke::default()
                });
            }
            "clip" => clips.push(builder.finish()),
            "dash" => {
                let stroke = stroke_style.as_mut().unwrap();
                stroke.dash_phase = n(1);
                stroke.dash_array = words.iter().skip(2).map(|v| v.parse().unwrap()).collect();
            }
            "m" => {
                builder.move_to(Point::new(n(1), n(2)));
            }
            "l" => {
                builder.line_to(Point::new(n(1), n(2))).unwrap();
            }
            "c" => {
                builder
                    .cubic_to(
                        Point::new(n(1), n(2)),
                        Point::new(n(3), n(4)),
                        Point::new(n(5), n(6)),
                    )
                    .unwrap();
            }
            "h" => {
                builder.close();
            }
            _ => unreachable!(),
        }
    }
    (builder.finish(), rule, stroke_style, clips)
}

type CoverageRenderer = fn(IntRect, &[Shape], &mut [u8], &Limits, &mut Scratch) -> Result<()>;

fn assert_vector_samples(name: &str, backend: &str, actual: &[u8], expected: &[u8], tolerance: u8) {
    assert_eq!(actual.len(), expected.len(), "{name} {backend}");
    for (i, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            actual.abs_diff(expected) <= tolerance,
            "{name} {backend} pixel {i}: {actual} vs {expected}"
        );
    }
}

fn assert_vector_tiles(
    name: &str,
    backend: &str,
    render: CoverageRenderer,
    shapes: &[Shape],
    whole: &[u8],
    limits: &Limits,
    scratch: &mut Scratch,
) {
    for y in 0..8 {
        for x in 0..8 {
            let mut pixel = [0];
            render(area(x, y, 1, 1), shapes, &mut pixel, limits, scratch).unwrap();
            assert_eq!(
                pixel[0],
                whole[(y * 8 + x) as usize],
                "{name} {backend} tile {x},{y}"
            );
        }
    }
}

#[test]
fn independent_exact_polygon_oracles() {
    for name in [
        "triangle",
        "concave",
        "fractional-rectangle",
        "self-intersection",
        "overlap-nonzero",
        "overlap-evenodd",
        "square-dot",
        "zero-square-oblique",
        "join-miter",
        "join-bevel",
        "miter-limit",
        "closed-dash-seam",
    ] {
        vector(name, 1);
    }
}

#[test]
fn independent_curved_and_stroke_oracles() {
    for name in [
        "cap-butt",
        "cap-round",
        "cap-square",
        "round-dot",
        "cubic",
        "join-round",
        "dash-odd-phase",
        "zero-on-dash",
    ] {
        vector(name, 3);
    }
}

#[test]
fn independent_crossing_and_clip_sets() {
    for i in 0..16 {
        vector(&format!("crossing-set-{i}"), 1);
    }
}

#[test]
fn independent_winding_and_collapsing_contours() {
    for rule in ["nonzero", "evenodd"] {
        for name in [
            "boundary-triangle",
            "opposite-chains",
            "crossed-rules",
            "tiny-bowtie",
            "overlapping-contours",
            "coincident-opposite",
            "touching-contours",
            "same-clip",
            "nested-clips",
            "crossing-clips",
            "star-in-cell",
            "double-crossing-chains",
            "thin-rule",
            "coincident-same",
            "collinear",
            "huge-endpoints",
            "rounding-threshold",
            "translated-half-tie",
        ] {
            vector(&format!("winding-{name}-{rule}"), 1);
        }
        for case in 0..16 {
            vector(&format!("winding-random-{case}-{rule}"), 1);
        }
        for axis in ["horizontal", "vertical"] {
            vector(&format!("dense-micro-slivers-{axis}-{rule}"), 0);
        }
        for count in [127, 128, 129, 512, 513] {
            for axis in ["horizontal", "vertical"] {
                for direction in ["forward", "reverse"] {
                    vector(&format!("collapse-{axis}-{direction}-{count}-{rule}"), 0);
                }
            }
        }
    }
}

#[test]
fn independent_quadrilateral_component_oracles() {
    for count in [128, 129] {
        for rule in ["nonzero", "evenodd"] {
            vector(&format!("quad-union-{count}-{rule}"), 0);
        }
    }
}
