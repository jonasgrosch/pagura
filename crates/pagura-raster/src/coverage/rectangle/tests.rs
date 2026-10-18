use super::*;
use crate::{FillRule, LineCap, Mask, PathBuilder, Point, Stroke, Transform, stroke_outline};
use std::sync::Arc;

fn shape(points: &[Point], close: bool) -> Shape {
    let mut path = PathBuilder::new();
    path.move_to(points[0]);
    for &point in &points[1..] {
        path.line_to(point).unwrap();
    }
    if close {
        path.close();
    }
    Shape {
        geometry: Geometry::Path(Arc::new(path.finish())),
        transform: Transform::IDENTITY,
        rule: FillRule::NonZero,
    }
}

fn path(shape: &Shape) -> &crate::Path {
    let Geometry::Path(path) = &shape.geometry else {
        panic!("expected path geometry")
    };
    path
}

fn mask(shape: &Shape, size: u32, origin: (u32, u32)) -> Mask {
    Mask::from_path_at(
        size,
        size,
        path(shape),
        shape.transform,
        shape.rule,
        64,
        origin,
    )
    .unwrap()
}

#[test]
fn explicit_and_implicit_closing_in_both_directions_and_transforms() {
    let corners = [
        Point::new(0.25, 0.5),
        Point::new(1.75, 0.5),
        Point::new(1.75, 1.5),
        Point::new(0.25, 1.5),
    ];
    for reverse in [false, true] {
        for repeat in [false, true] {
            for close in [false, true] {
                let mut points = corners.to_vec();
                if reverse {
                    points.reverse();
                }
                if repeat {
                    points.push(points[0]);
                }
                for transform in [
                    Transform::IDENTITY,
                    Transform::new(0.0, 1.0, -1.0, 0.0, 2.0, 0.0),
                    Transform::new(-1.0, 0.0, 0.0, -1.0, 2.0, 2.0),
                ] {
                    let mut rectangle = shape(&points, close);
                    rectangle.transform = transform;
                    assert!(rectangle.rectangle().is_some());
                    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                        rectangle.rule = rule;
                        // Rotation swaps 3/4 by 1/2 overlaps; each corner is 3/8.
                        assert_eq!(mask(&rectangle, 2, (0, 0)).coverage(), [96; 4]);
                    }
                }
            }
        }
    }
}

#[test]
fn rational_interval_product_on_an_eighth_pixel_lattice() {
    // Integer arithmetic is an independent exact area oracle: both overlaps
    // have denominator 8, so the product has denominator 64, rounded once.
    for left in -3..19 {
        for top in -3..19 {
            for (width, height) in [(1, 1), (7, 9), (19, 17), (32, 32)] {
                let right = left + width;
                let bottom = top + height;
                let mut path = PathBuilder::new();
                path.rect(
                    f64::from(left) / 8.0,
                    f64::from(top) / 8.0,
                    f64::from(width) / 8.0,
                    f64::from(height) / 8.0,
                );
                let actual = Mask::from_path(
                    4,
                    4,
                    &path.finish(),
                    Transform::IDENTITY,
                    FillRule::NonZero,
                    16,
                )
                .unwrap();
                for y in 0..4 {
                    for x in 0..4 {
                        let dx = (right.min(x * 8 + 8) - left.max(x * 8)).max(0);
                        let dy = (bottom.min(y * 8 + 8) - top.max(y * 8)).max(0);
                        let expected = u8::try_from((dx * dy * 255 + 32) / 64).unwrap();
                        assert_eq!(
                            actual.coverage()[usize::try_from(y * 4 + x).unwrap()],
                            expected
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn fractional_clip_intersection_and_global_tiles() {
    let mut a = PathBuilder::new();
    a.rect(10.25, 20.125, 2.5, 2.75);
    let mut b = PathBuilder::new();
    b.rect(11.125, 19.0, 2.0, 3.5);
    let a = a.finish();
    let b = b.finish();
    let mut whole = Mask::from_path_at(
        4,
        4,
        &a,
        Transform::IDENTITY,
        FillRule::NonZero,
        16,
        (10, 20),
    )
    .unwrap();
    let clip = Mask::from_path_at(
        4,
        4,
        &b,
        Transform::IDENTITY,
        FillRule::EvenOdd,
        16,
        (10, 20),
    )
    .unwrap();
    whole.intersect(&clip).unwrap();
    // Intersected bounds [11 1/8, 12 3/4] x [20 1/8, 22 1/2].
    assert_eq!(
        whole.coverage(),
        [0, 195, 167, 0, 0, 223, 191, 0, 0, 112, 96, 0, 0, 0, 0, 0]
    );
    for y in 0..4 {
        for x in 0..4 {
            let origin = (10 + x, 20 + y);
            let mut tile =
                Mask::from_path_at(1, 1, &a, Transform::IDENTITY, FillRule::NonZero, 1, origin)
                    .unwrap();
            tile.intersect(
                &Mask::from_path_at(1, 1, &b, Transform::IDENTITY, FillRule::EvenOdd, 1, origin)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                tile.coverage()[0],
                whole.coverage()[usize::try_from(y * 4 + x).unwrap()]
            );
        }
    }
}

#[test]
fn rectangle_clips_preserve_holes_and_disconnected_contours() {
    let mut path = PathBuilder::new();
    path.rect(-1.0, -1.0, 5.0, 5.0).rect(0.5, 0.5, 2.0, 2.0);
    let mut clipped = Mask::from_path(
        3,
        3,
        &path.finish(),
        Transform::IDENTITY,
        FillRule::EvenOdd,
        9,
    )
    .unwrap();
    let mut path = PathBuilder::new();
    path.rect(0.25, 0.25, 2.5, 2.5);
    clipped
        .intersect(
            &Mask::from_path(
                3,
                3,
                &path.finish(),
                Transform::IDENTITY,
                FillRule::NonZero,
                9,
            )
            .unwrap(),
        )
        .unwrap();
    // Outer 3/4 corner overlap minus the hole's 1/2 corner overlap.
    assert_eq!(clipped.coverage(), [80, 64, 80, 64, 0, 64, 80, 64, 80]);
}

#[test]
fn only_single_nondegenerate_device_rectangles_take_the_closed_form() {
    let points = [
        Point::new(0.0, 0.0),
        Point::new(1.0, 0.0),
        Point::new(1.0, 1.0),
        Point::new(0.0, 1.0),
    ];
    let mut rectangle = shape(&points, true);
    rectangle.transform = Transform::new(1.0, 0.125, 0.0, 1.0, 0.0, 0.0);
    assert!(rectangle.rectangle().is_none());
    rectangle.transform = Transform::scale(0.0, 1.0);
    assert!(rectangle.rectangle().is_none());
    rectangle.transform = Transform::translate(f64::INFINITY, 0.0);
    assert!(rectangle.rectangle().is_none());
    assert!(
        Mask::from_path(
            2,
            2,
            path(&rectangle),
            rectangle.transform,
            rectangle.rule,
            4
        )
        .is_err()
    );
    rectangle.transform = Transform::translate(1e13, 0.0);
    assert!(rectangle.rectangle().is_none());
    assert!(
        Mask::from_path(
            2,
            2,
            path(&rectangle),
            rectangle.transform,
            rectangle.rule,
            4
        )
        .is_err()
    );
    let point = shape(&[Point::new(0.25, 0.25); 4], true);
    assert!(point.rectangle().is_none());
    assert_eq!(mask(&point, 2, (0, 0)).coverage(), [255, 0, 0, 0]);
}

#[test]
fn butt_capped_axis_aligned_strokes_already_form_rectangles() {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.25, 1.0))
        .line_to(Point::new(1.75, 1.0))
        .unwrap();
    let stroke = Stroke {
        width: 1.0,
        cap: LineCap::Butt,
        ..Stroke::default()
    };
    let (outline, transform) = stroke_outline(
        &path.finish(),
        Transform::IDENTITY,
        &stroke,
        &Limits::default(),
    )
    .unwrap();
    let rectangle = Shape {
        geometry: Geometry::Path(Arc::new(outline)),
        transform,
        rule: FillRule::NonZero,
    };
    assert!(rectangle.rectangle().is_some());
    assert_eq!(mask(&rectangle, 2, (0, 0)).coverage(), [96; 4]);
}

#[test]
fn canonical_glyph_rectangles_use_closed_form_area_but_points_do_not() {
    let original = shape(
        &[
            Point::new(0.25, 0.5),
            Point::new(1.75, 0.5),
            Point::new(1.75, 1.5),
            Point::new(0.25, 1.5),
        ],
        true,
    );
    let glyph = Shape {
        geometry: Geometry::Glyph(Arc::new(
            crate::GlyphGeometry::new(path(&original), [1.0, 0.0, 0.0, 1.0], &Limits::default())
                .unwrap(),
        )),
        transform: Transform::IDENTITY,
        rule: FillRule::NonZero,
    };
    assert!(glyph.rectangle().is_some());
    let mut out = [0; 4];
    crate::coverage::coverage(
        2,
        2,
        &[glyph],
        &mut out,
        (0, 0),
        &Limits::default(),
        &mut crate::coverage::Scratch::default(),
    )
    .unwrap();
    assert_eq!(out, [96; 4]);
    let original = shape(&[Point::new(0.25, 0.25); 4], true);
    let glyph = Shape {
        geometry: Geometry::Glyph(Arc::new(
            crate::GlyphGeometry::new(path(&original), [1.0, 0.0, 0.0, 1.0], &Limits::default())
                .unwrap(),
        )),
        transform: Transform::IDENTITY,
        rule: FillRule::NonZero,
    };
    assert!(glyph.rectangle().is_none());
    let mut out = [0; 4];
    crate::coverage::coverage(
        2,
        2,
        &[glyph],
        &mut out,
        (0, 0),
        &Limits::default(),
        &mut crate::coverage::Scratch::default(),
    )
    .unwrap();
    assert_eq!(out, [255, 0, 0, 0]);
}
