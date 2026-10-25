//! Allocation budgets for owned geometry prepared from borrowed paths.

#![forbid(unsafe_code)]

use pagura_core::{ErrorKind, Limits, Meter};
use pagura_raster::{
    Adjustment, Area, ClipShape, DeviceRect, FillRule, GlyphEdges, PathBuilder, Point, Shape,
    ShapeOptions, Stroke, Transform,
};

#[test]
fn transformed_paths_reserve_their_copy_before_mapping() {
    let mut path = PathBuilder::new();
    path.rect(1.0, 2.0, 3.0, 4.0);
    let path = path.finish();
    let needed = size_of_val(path.segments());
    let mut refused = Meter::new(needed - 1, "copy bytes");
    let error = path
        .transformed_with_bytes(Transform::IDENTITY, &mut refused)
        .unwrap_err();
    assert_eq!(
        (error.kind, error.message, refused.used()),
        (ErrorKind::Limit, "copy bytes", 0)
    );
    let mut exact = Meter::new(needed, "copy bytes");
    let transform = Transform::new(2.0, -1.0, 0.5, 3.0, -4.0, 7.0);
    let moved = path.transformed_with_bytes(transform, &mut exact).unwrap();
    assert_eq!(moved, path.transformed(transform));
    assert_eq!(exact.used(), moved.retained_bytes());
}

#[test]
fn geometry_factories_refuse_without_an_allocation_allowance() {
    let mut path = PathBuilder::new();
    path.polygon([
        Point::new(0.0, 0.0),
        Point::new(6.0, 1.0),
        Point::new(2.0, 7.0),
    ]);
    let path = path.finish();
    let limits = Limits {
        render_bytes: 0,
        ..Limits::default()
    };
    let options = ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 8,
            },
            band_height: 4,
        },
        limits: &limits,
        scratch: None,
    };
    let fill = Shape::fill(&path, Transform::IDENTITY, FillRule::NonZero, &options).unwrap_err();
    let clip =
        ClipShape::new(&[(&path, FillRule::NonZero)], Transform::IDENTITY, &options).unwrap_err();
    for adjustment in [Adjustment::None, Adjustment::Adjusted] {
        let stroke = Shape::stroke(
            &path,
            &Stroke::default(),
            Transform::IDENTITY,
            Transform::IDENTITY,
            adjustment,
            &options,
        )
        .unwrap_err();
        assert_eq!(
            (stroke.kind, stroke.message),
            (ErrorKind::Limit, "render memory budget exceeded")
        );
    }
    for error in [fill, clip] {
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, "render memory budget exceeded")
        );
    }
}

#[test]
fn source_segment_bound_precedes_construction_allocations() {
    let mut path = PathBuilder::new();
    for x in 0..4096 {
        path.rect(f64::from(x), 0.0, 1.0, 1.0);
    }
    let limits = Limits {
        path_segments: 32,
        decoded_bytes: 0,
        ..Limits::default()
    };
    let error = GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap_err();
    assert_eq!(
        (error.kind, error.message),
        (ErrorKind::Limit, "path segment budget exceeded")
    );
}
