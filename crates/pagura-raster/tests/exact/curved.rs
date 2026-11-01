//! Independent rational areas for inherited true-quadratic glyph clips.

use crate::support::{area, options, raster};
use pagura_core::{ErrorKind, Limits};
use pagura_raster::{
    ClipShape, ClipSource, FillRule, GlyphEdges, Path, PathBuilder, Point, Shape, Transform,
};
use std::sync::Arc;

fn cap() -> Path {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    path.quad_to(Point::new(0.5, 1.0), Point::new(1.0, 0.0))
        .unwrap();
    path.close();
    path.finish()
}

fn rectangle(x: f64, y: f64, w: f64, h: f64) -> Path {
    let mut path = PathBuilder::new();
    path.rect(x, y, w, h);
    path.finish()
}

#[test]
fn nested_quadratic_clip_joint_rational_areas() {
    // y=2*x*(1-x): its integral is1/3; over[0,1/4] it is5/96.
    let limits = Limits::default();
    let opts = options(area(0, 0, 1, 1, 1), &limits);
    let cap = cap();
    let clip = ClipShape::from_sources(
        &[
            ClipSource::glyph(&cap, Transform::IDENTITY),
            ClipSource::glyph(&cap, Transform::IDENTITY),
        ],
        &opts,
    )
    .unwrap();
    assert_eq!(raster(&opts.area, None, Some(&clip)).0, vec![85]);
    let quarter = rectangle(0.0, 0.0, 0.25, 1.0);
    let shape = Shape::fill_with_clip(
        &quarter,
        Transform::IDENTITY,
        FillRule::NonZero,
        Some(&clip),
        &opts,
    )
    .unwrap()
    .unwrap();
    let (pixels, work) = raster(&opts.area, Some(&shape), Some(&clip));
    assert_eq!(pixels, vec![13]);
    assert!(work <= shape.work());
    let edges = Arc::new(GlyphEdges::new(&cap, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let glyph = Shape::glyph_with_clip(&edges, Point::default(), Some(&clip), &opts)
        .unwrap()
        .unwrap();
    assert_eq!(raster(&opts.area, Some(&glyph), Some(&clip)).0, vec![85]);
}

#[test]
fn nested_containing_curve_preserves_straight_halfpixel_ties() {
    let limits = Limits::default();
    let cap = cap();
    for origin in [0_u32, 7, 31] {
        let opts = options(area(origin, origin, origin + 1, origin + 1, 1), &limits);
        let o = f64::from(origin);
        // This pixel lies strictly inside the cap scaled/translated around it.
        let transform = Transform::new(8.0, 0.0, 0.0, 8.0, o - 3.5, o - 1.0);
        let clip = ClipShape::from_sources(&[ClipSource::glyph(&cap, transform)], &opts).unwrap();
        for leaf in [rectangle(o, o, 0.5, 1.0), {
            let mut p = PathBuilder::new();
            p.polygon([
                Point::new(o, o),
                Point::new(o + 1.0, o),
                Point::new(o, o + 1.0),
            ]);
            p.finish()
        }] {
            let shape = Shape::fill_with_clip(
                &leaf,
                Transform::IDENTITY,
                FillRule::NonZero,
                Some(&clip),
                &opts,
            )
            .unwrap()
            .unwrap();
            assert_eq!(raster(&opts.area, Some(&shape), Some(&clip)).0, vec![128]);
        }
    }
}

#[test]
fn nested_curved_clips_keep_points_and_validate_before_empty_culling() {
    let limits = Limits::default();
    let opts = options(area(0, 0, 1, 1, 1), &limits);
    let cap = cap();
    let clip = ClipShape::from_sources(
        &[ClipSource::glyph(
            &cap,
            Transform::new(8.0, 0.0, 0.0, 8.0, -3.5, -1.0),
        )],
        &opts,
    )
    .unwrap();
    let mut point = PathBuilder::new();
    point.move_to(Point::new(0.25, 0.25));
    point.close();
    let edges = Arc::new(GlyphEdges::new(&point.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let shape = Shape::glyph_with_clip(&edges, Point::default(), Some(&clip), &opts)
        .unwrap()
        .unwrap();
    assert_eq!(raster(&opts.area, Some(&shape), Some(&clip)).0, vec![255]);
    for origin in [
        Point::new(f64::NAN, 0.0),
        Point::new(f64::INFINITY, 0.0),
        Point::new(1e13, 0.0),
    ] {
        assert!(Shape::glyph_with_clip(&edges, origin, Some(&clip), &opts).is_err());
    }
    let tight = Limits {
        render_bytes: 0,
        ..limits
    };
    let error = ClipShape::from_sources(
        &[ClipSource::glyph(&cap, Transform::IDENTITY)],
        &options(opts.area, &tight),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}

#[test]
fn nested_curved_clip_512_pixel_cap_obeys_default_limits() {
    let limits = Limits::default();
    let opts = options(area(0, 0, 512, 512, 16), &limits);
    let cap = cap();
    let clip = ClipShape::from_sources(
        &[ClipSource::glyph(
            &cap,
            Transform::new(512.0, 0.0, 0.0, 512.0, 0.0, 0.0),
        )],
        &opts,
    )
    .unwrap();
    assert!(clip.retained_bytes() < limits.render_bytes);
    let (_, work) = raster(&opts.area, None, Some(&clip));
    assert!(work <= clip.work());
}

#[test]
fn nested_genuine_cubics_keep_the_direct_glyph_polygon_and_point_policy() {
    let limits = Limits::default();
    let opts = options(area(0, 0, 8, 8, 3), &limits);
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    path.cubic_to(
        Point::new(0.0, 1.0),
        Point::new(1.0, 0.25),
        Point::new(1.0, 0.0),
    )
    .unwrap();
    path.close();
    path.move_to(Point::new(1.7, 1.7));
    path.close();
    let path = path.finish();
    let transform = Transform::new(2.0, 0.0, 0.0, 2.0, 1.125, 1.375);
    let crop = rectangle(0.0, 0.0, 8.0, 8.0);
    let ordinary =
        ClipShape::new(&[(&crop, FillRule::NonZero)], Transform::IDENTITY, &opts).unwrap();
    let edges = Arc::new(GlyphEdges::new(&path, [2.0, 0.0, 0.0, 2.0], &limits).unwrap());
    let direct = Shape::glyph_with_clip(
        &edges,
        Point::new(transform.e, transform.f),
        Some(&ordinary),
        &opts,
    )
    .unwrap()
    .unwrap();
    let expected = raster(&opts.area, Some(&direct), Some(&ordinary)).0;
    let inherited = ClipShape::from_sources(
        &[
            ClipSource::fill(&crop, FillRule::NonZero, Transform::IDENTITY),
            ClipSource::glyph(&path, transform),
        ],
        &opts,
    )
    .unwrap();
    assert_eq!(raster(&opts.area, None, Some(&inherited)).0, expected);
    assert_eq!(expected[4 * 8 + 4], 255);
}
