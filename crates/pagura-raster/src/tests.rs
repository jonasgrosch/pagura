//! Independent generated-vector and API invariant tests.

use crate::coverage::{self, Scratch, Shape};
use crate::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Point, Rgba8, Stroke,
    Transform, fill_path, stroke_path,
};
use pagura_core::{ErrorKind, Limits, Result};
use std::sync::Arc;

fn vector(name: &str, tolerance: u8) {
    let text =
        String::from_utf8(pagura_testkit::fixture("raster", &format!("{name}.path"))).unwrap();
    let (path, rule, stroke, clips) = vector_geometry(&text);
    let expected = pagura_testkit::fixture("raster", &format!("{name}.pgm"));
    let expected = expected.strip_prefix(b"P5\n8 8\n255\n").unwrap();
    let (path, transform, rule) = if let Some(stroke) = stroke {
        let mut target = Pixmap::new(8, 8, Rgba8::TRANSPARENT, 64).unwrap();
        stroke_path(
            &mut target,
            &path,
            Transform::IDENTITY,
            &stroke,
            &Paint::Solid(Rgba8::BLACK),
            None,
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
        let (outline, transform) =
            crate::stroke_outline(&path, Transform::IDENTITY, &stroke, &Limits::default()).unwrap();
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
                    crate::GlyphGeometry::new(path, [t.a, t.b, t.c, t.d], &Limits::default())
                        .unwrap(),
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
        render(8, 8, shapes, &mut whole, (0, 0), &limits, &mut scratch).unwrap();
        assert_vector_samples(name, backend, &whole, expected, tolerance);
        assert_vector_tiles(name, backend, render, shapes, &whole, &limits, &mut scratch);
    }
}

fn vector_geometry(text: &str) -> (Path, FillRule, Option<Stroke>, Vec<Path>) {
    let mut builder = PathBuilder::new();
    let mut rule = FillRule::NonZero;
    let mut stroke = None;
    let mut clips = Vec::new();
    for line in text.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        let n = |i: usize| words[i].parse::<f64>().unwrap();
        match words[0] {
            "nonzero" => rule = FillRule::NonZero,
            "evenodd" => rule = FillRule::EvenOdd,
            "stroke" => {
                stroke = Some(Stroke {
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
                let stroke = stroke.as_mut().unwrap();
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
    (builder.finish(), rule, stroke, clips)
}

type CoverageRenderer =
    fn(u32, u32, &[Shape], &mut [u8], (u32, u32), &Limits, &mut Scratch) -> Result<()>;

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
            render(1, 1, shapes, &mut pixel, (x, y), limits, scratch).unwrap();
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
fn clipping_and_premultiplied_composition() {
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, 1.0, 1.0);
    let path = path.finish();
    let mut clip_path = PathBuilder::new();
    clip_path.rect(0.0, 0.0, 0.5, 1.0);
    let mut clip = Mask::from_path(
        1,
        1,
        &clip_path.finish(),
        Transform::IDENTITY,
        FillRule::NonZero,
        1,
    )
    .unwrap();
    let full = Mask::from_path(1, 1, &path, Transform::IDENTITY, FillRule::NonZero, 1).unwrap();
    clip.intersect(&full).unwrap();
    assert_eq!(clip.coverage(), &[128]);
    let mut target = Pixmap::new(1, 1, Rgba8::TRANSPARENT, 1).unwrap();
    fill_path(
        &mut target,
        &path,
        Transform::IDENTITY,
        FillRule::NonZero,
        &Paint::Solid(Rgba8::new(255, 0, 0, 128)),
        Some(&clip),
    )
    .unwrap();
    assert_eq!(target.data(), &[64, 0, 0, 64]);
}

#[test]
fn stroke_transform_hairline_dash_and_miter() {
    let mut builder = PathBuilder::new();
    builder
        .move_to(Point::new(1.0, 2.0))
        .line_to(Point::new(3.0, 2.0))
        .unwrap();
    let path = builder.finish();
    let transform = Transform::scale(2.0, 1.0);
    let mut target = Pixmap::new(8, 8, Rgba8::TRANSPARENT, 64).unwrap();
    stroke_path(
        &mut target,
        &path,
        transform,
        &Stroke {
            width: 2.0,
            cap: LineCap::ProjectingSquare,
            ..Stroke::default()
        },
        &Paint::Solid(Rgba8::BLACK),
        None,
    )
    .unwrap();
    // User-space square cap extends 1 unit, hence 2 device pixels in x.
    assert_eq!(
        target
            .row(1)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .collect::<Vec<_>>(),
        vec![255; 8]
    );
    let mut target = Pixmap::new(8, 8, Rgba8::TRANSPARENT, 64).unwrap();
    stroke_path(
        &mut target,
        &path,
        transform,
        &Stroke {
            width: 0.0,
            ..Stroke::default()
        },
        &Paint::Solid(Rgba8::BLACK),
        None,
    )
    .unwrap();
    assert_eq!(
        target
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u32::from(p[3]))
            .sum::<u32>(),
        4 * 256
    );
    let stroke = Stroke {
        width: 2.0,
        dash_array: vec![1.0, 1.0],
        ..Stroke::default()
    };
    let (outline, _) =
        crate::stroke::outline(&path, Transform::IDENTITY, &stroke, &Limits::default()).unwrap();
    let mask = Mask::from_path(8, 8, &outline, Transform::IDENTITY, FillRule::NonZero, 64).unwrap();
    assert_eq!(
        mask.coverage().iter().map(|&n| u32::from(n)).sum::<u32>(),
        2 * 255
    );
    for join in [LineJoin::Miter, LineJoin::Bevel, LineJoin::Round] {
        let mut p = PathBuilder::new();
        p.move_to(Point::new(1.0, 6.0))
            .line_to(Point::new(4.0, 2.0))
            .unwrap()
            .line_to(Point::new(7.0, 6.0))
            .unwrap();
        let (outline, _) = crate::stroke::outline(
            &p.finish(),
            Transform::IDENTITY,
            &Stroke {
                join,
                ..Stroke::default()
            },
            &Limits::default(),
        )
        .unwrap();
        assert!(!outline.is_empty());
    }
}

#[test]
fn invalid_geometry_and_budgets_return_errors() {
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(f64::NAN, 0.0));
    assert_eq!(
        Mask::from_path(
            1,
            1,
            &builder.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
            1
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        Mask::from_path(
            8,
            8,
            &Path::default(),
            Transform::IDENTITY,
            FillRule::NonZero,
            63
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
    let mut target = Pixmap::new(1, 1, Rgba8::WHITE, 1).unwrap();
    let stroke = Stroke {
        dash_array: vec![0.0],
        ..Stroke::default()
    };
    assert!(
        stroke_path(
            &mut target,
            &Path::default(),
            Transform::IDENTITY,
            &stroke,
            &Paint::Solid(Rgba8::BLACK),
            None
        )
        .is_err()
    );
}

#[test]
fn mask_intersection_uses_geometry_and_is_atomic() {
    let mut builder = PathBuilder::new();
    builder.rect(0.0, 0.0, 0.5, 1.0);
    let left = Mask::from_path(
        1,
        1,
        &builder.finish(),
        Transform::IDENTITY,
        FillRule::NonZero,
        1,
    )
    .unwrap();
    let mut intersection = left.clone();
    intersection.intersect(&left).unwrap();
    assert_eq!(intersection.coverage(), &[128]);
    builder.rect(0.5, 0.0, 0.5, 1.0);
    let right = Mask::from_path(
        1,
        1,
        &builder.finish(),
        Transform::IDENTITY,
        FillRule::NonZero,
        1,
    )
    .unwrap();
    intersection.intersect(&right).unwrap();
    assert_eq!(intersection.coverage(), &[0]);
    let mut current = left;
    for _ in 0..8 {
        let other = current.clone();
        current.intersect(&other).unwrap();
    }
    let previous = current.clone();
    assert_eq!(
        current.intersect(&previous).unwrap_err().kind,
        ErrorKind::Limit
    );
    assert_eq!(current, previous);
}

#[test]
fn large_compound_triangles_and_caller_budgets() {
    let mut builder = PathBuilder::new();
    for y in 0..200 {
        for x in 0..250 {
            let (x, y) = (f64::from(x * 2), f64::from(y * 2));
            builder
                .move_to(Point::new(x, y))
                .line_to(Point::new(x + 1.0, y))
                .unwrap()
                .line_to(Point::new(x, y + 1.0))
                .unwrap()
                .close();
        }
    }
    let path = builder.finish();
    let mask = Mask::from_path(
        500,
        400,
        &path,
        Transform::IDENTITY,
        FillRule::NonZero,
        200_000,
    )
    .unwrap();
    for (y, row) in mask.coverage().as_chunks::<500>().0.iter().enumerate() {
        for (x, &value) in row.iter().enumerate() {
            assert_eq!(value, if x % 2 == 0 && y % 2 == 0 { 128 } else { 0 });
        }
    }
    for limits in [
        Limits {
            path_segments: 10,
            ..Limits::default()
        },
        Limits {
            raster_work: 0,
            ..Limits::default()
        },
        Limits {
            mask_sources: 0,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            Mask::from_path_with_limits(
                500,
                400,
                &path,
                Transform::IDENTITY,
                FillRule::NonZero,
                limits,
                (0, 0)
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit
        );
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
