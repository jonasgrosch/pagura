//! Overlapping quadratic contours resolved once, as a covered set or as
//! resolved polygon sides: every placement keeps the bytes of the
//! per-placement polygon and its resolved rows.

use super::*;
use crate::band::shape::{Body, GlyphGeometry};
use crate::{Area, ClipShape, DeviceRect, ShapeOptions};
use pagura_core::Rect;

/// A circle of four quadratics, counterclockwise or clockwise.
fn blob(path: &mut PathBuilder, (cx, cy, r): (f64, f64, f64), reversed: bool) {
    let p = |x: f64, y: f64| Point::new(cx + x * r, cy + y * r);
    let sign = if reversed { -1.0 } else { 1.0 };
    path.move_to(p(1.0, 0.0));
    for [control, end] in [
        [(1.0, 1.0), (0.0, 1.0)],
        [(-1.0, 1.0), (-1.0, 0.0)],
        [(-1.0, -1.0), (0.0, -1.0)],
        [(1.0, -1.0), (1.0, 0.0)],
    ] {
        let control = p(control.0, sign * control.1);
        let end = p(end.0, sign * end.1);
        path.quad_to(control, end).unwrap();
    }
    path.close();
}

/// Two blobs overlapping in the middle rows, wound alike or opposite.
fn outline(first: bool, second: bool) -> Path {
    let mut path = PathBuilder::new();
    blob(&mut path, (4.0, 4.0, 3.0), first);
    blob(&mut path, (6.5, 7.5, 3.0), second);
    path.finish()
}

/// The polygon kind of an outline that keeps no covered set.
fn polygon_edges(outline: &Path, linear: [f64; 4], limits: &Limits) -> GlyphEdges {
    let [a, b, c, d] = linear;
    let transform = Transform::new(a, b, c, d, 0.0, 0.0);
    let mut bytes = Meter::new(limits.decoded_bytes, "glyph geometry byte budget exceeded");
    let kind = GlyphEdges::polygon_kind(outline, transform, limits, &mut bytes).unwrap();
    GlyphEdges {
        retained: bytes.used(),
        hull: control_hull(outline, transform),
        kind,
    }
}

/// The same edges placed through the polygon, resolving rows per placement.
fn per_placement(outline: &Path, linear: [f64; 4], limits: &Limits) -> Arc<GlyphEdges> {
    let mut edges = polygon_edges(outline, linear, limits);
    let Kind::Polygon { state, .. } = &mut edges.kind else {
        panic!("an uncertified polygon");
    };
    let PolygonState::Resolved { source, .. } = state else {
        panic!("resolved sides");
    };
    *state = PolygonState::Checked {
        source: Some(Arc::clone(source)),
    };
    Arc::new(edges)
}

#[test]
fn resolved_overlaps_place_the_bytes_of_resolving_each_placement() {
    let limits = Limits::default();
    let options = ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 32,
                y1: 32,
            },
            band_height: 16,
        },
        limits: &limits,
    };
    let prep = Prep::new(&options, None);
    for (first, second) in [(false, false), (true, true), (false, true)] {
        for linear in [[1.0, 0.0, 0.0, 1.0], [1.5, 0.0, 0.25, 1.25]] {
            let outline = outline(first, second);
            let edges = Arc::new(polygon_edges(&outline, linear, &limits));
            assert!(!edges.curved());
            assert!(matches!(
                &edges.kind,
                Kind::Polygon {
                    state: PolygonState::Resolved { .. },
                    ..
                }
            ));
            let old = per_placement(&outline, linear, &limits);
            for fraction in [0.0, 0.125, 0.3, 0.5, 0.71, 0.97] {
                let origin = Point::new(5.0 + fraction, 6.0 + (fraction * 7.0).fract());
                let shape = place(&edges, origin, &prep).unwrap().unwrap();
                let Body::Glyph(body) = &shape.body else {
                    panic!("resolved sides are shared");
                };
                assert_eq!(body.geometry, GlyphGeometry::Relative);
                let expected = place(&old, origin, &prep).unwrap().unwrap();
                let (actual, work) = crate::testing::raster(&options.area, Some(&shape), None);
                let (bytes, _) = crate::testing::raster(&options.area, Some(&expected), None);
                assert_eq!(actual, bytes, "{first} {second} {linear:?} {origin:?}");
                assert!(work <= shape.work());
            }
        }
    }
}

#[test]
fn clipped_placements_keep_the_polygon() {
    let limits = Limits::default();
    let options = ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 32,
                y1: 32,
            },
            band_height: 16,
        },
        limits: &limits,
    };
    let rectangle = Path::rect(Rect::from_corners(0.0, 0.0, 9.5, 32.0).unwrap());
    let clip = ClipShape::new(
        &[(&rectangle, FillRule::NonZero)],
        Transform::IDENTITY,
        &options,
    )
    .unwrap();
    let prep = Prep::new(&options, Some(&clip));
    let outline = outline(false, false);
    let edges = Arc::new(GlyphEdges::new(&outline, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let old = per_placement(&outline, [1.0, 0.0, 0.0, 1.0], &limits);
    let origin = Point::new(0.25, 0.625);
    let shape = place(&edges, origin, &prep).unwrap().unwrap();
    assert!(matches!(shape.body, Body::Edges(_)));
    let expected = place(&old, origin, &prep).unwrap().unwrap();
    assert_eq!(
        crate::testing::raster(&options.area, Some(&shape), Some(&clip)).0,
        crate::testing::raster(&options.area, Some(&expected), Some(&clip)).0
    );
}

/// Two tall overlapping bars with one small curved corner each: the row
/// resolver cuts their long sides at every row they overlap in.
fn bars() -> Path {
    let mut path = PathBuilder::new();
    for x in [0.0, 2.0] {
        path.move_to(Point::new(x, 0.0));
        path.line_to(Point::new(x + 4.0, 0.0)).unwrap();
        path.line_to(Point::new(x + 4.0, 29.9)).unwrap();
        path.quad_to(Point::new(x + 4.0, 30.0), Point::new(x + 3.9, 30.0))
            .unwrap();
        path.line_to(Point::new(x, 30.0)).unwrap();
        path.close();
    }
    path.finish()
}

#[test]
fn resolved_sides_are_retained_within_the_segment_budget() {
    let limits = Limits::default();
    let outline = bars();
    let edges = polygon_edges(&outline, [1.0, 0.0, 0.0, 1.0], &limits);
    let Kind::Polygon {
        polygon,
        state: PolygonState::Resolved { relative, .. },
        ..
    } = &edges.kind
    else {
        panic!("resolved sides");
    };
    let sides = relative.lines.len();
    assert!(sides > polygon.vertices());
    assert!(
        edges.retained_bytes()
            >= polygon.heap_bytes()
                + relative.lines.capacity() * size_of::<Line>()
                + size_of::<Relative>()
    );
    // Sides beyond the budget leave each placement resolving its own rows.
    let tight = Limits {
        path_segments: sides - 1,
        ..limits
    };
    let edges = polygon_edges(&outline, [1.0, 0.0, 0.0, 1.0], &tight);
    assert!(matches!(
        edges.kind,
        Kind::Polygon {
            state: PolygonState::Checked { .. },
            ..
        }
    ));
}

fn options(limits: &Limits) -> ShapeOptions<'_> {
    ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 32,
                y1: 32,
            },
            band_height: 16,
        },
        limits,
    }
}

/// Places `outline` at fractional origins through its covered set and
/// through the per-placement polygon, which must give the same bytes, and
/// returns the covered set's allowance.
fn same_bytes(outline: &Path, linear: [f64; 4]) -> f64 {
    let limits = Limits::default();
    let options = options(&limits);
    let prep = Prep::new(&options, None);
    let edges = Arc::new(GlyphEdges::new(outline, linear, &limits).unwrap());
    let Kind::Union { allowance, .. } = edges.kind else {
        panic!("a covered set");
    };
    assert!(edges.curved());
    let old = per_placement(outline, linear, &limits);
    for fraction in [0.0, 0.125, 0.3, 0.5, 0.71, 0.97] {
        let origin = Point::new(5.0 + fraction, 6.0 + (fraction * 7.0).fract());
        let shape = place(&edges, origin, &prep).unwrap().unwrap();
        let Body::Glyph(body) = &shape.body else {
            panic!("a covered set is shared");
        };
        assert_eq!(body.geometry, GlyphGeometry::Relative);
        let expected = place(&old, origin, &prep).unwrap().unwrap();
        let (actual, work) = crate::testing::raster(&options.area, Some(&shape), None);
        let (bytes, _) = crate::testing::raster(&options.area, Some(&expected), None);
        assert_eq!(actual, bytes, "{linear:?} {origin:?}");
        assert!(work <= shape.work());
    }
    allowance
}

#[test]
fn covered_sets_place_the_bytes_of_resolving_each_placement() {
    for (first, second) in [(false, false), (true, true), (false, true)] {
        for linear in [[1.0, 0.0, 0.0, 1.0], [1.5, 0.0, 0.25, 1.25]] {
            // The circles cross: the slabs around each crossing are halved
            // down to the least height, which leaves a tiny allowance.
            let allowance = same_bytes(&outline(first, second), linear);
            assert!(allowance > 0.0 && allowance < 1e-9, "{allowance}");
        }
    }
    // Overlapping bars keep the outer sides whole, with no allowance.
    assert_eq!(same_bytes(&bars(), [1.0, 0.0, 0.0, 1.0]), 0.0);
}

/// A bar with one small curved corner at `x`, `height` tall.
fn bar(path: &mut PathBuilder, x: f64, height: f64) {
    path.move_to(Point::new(x, 0.0));
    path.line_to(Point::new(x + 4.0, 0.0)).unwrap();
    path.line_to(Point::new(x + 4.0, height - 0.1)).unwrap();
    path.quad_to(Point::new(x + 4.0, height), Point::new(x + 3.9, height))
        .unwrap();
    path.line_to(Point::new(x, height)).unwrap();
    path.close();
}

#[test]
fn coincident_sides_are_kept_or_dropped_together() {
    // Bars sharing a side of different lengths, which winds 0 across it,
    // and a bar drawn twice, which winds 2 inside.
    for (second, height) in [(4.0, 7.0), (4.0, 10.0), (0.0, 10.0)] {
        let mut path = PathBuilder::new();
        bar(&mut path, 0.0, 10.0);
        bar(&mut path, second, height);
        same_bytes(&path.finish(), [1.0, 0.0, 0.0, 1.0]);
    }
}

/// A tall shape whose right side is `bumps` small curves, crossed by as
/// many overlapping bars: the side is kept between the bars only.
fn interrupted(bumps: usize) -> Path {
    let mut path = PathBuilder::new();
    let height = 2.0 * bumps as f64;
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(10.0, 0.0)).unwrap();
    for i in 0..bumps {
        let y = 2.0 * i as f64;
        path.quad_to(Point::new(10.5, y + 1.0), Point::new(10.0, y + 2.0))
            .unwrap();
    }
    path.line_to(Point::new(0.0, height)).unwrap();
    path.close();
    for i in 0..bumps {
        let y = 2.0 * i as f64;
        path.move_to(Point::new(8.0, y + 0.75));
        path.line_to(Point::new(12.0, y + 0.75)).unwrap();
        path.line_to(Point::new(12.0, y + 1.25)).unwrap();
        path.line_to(Point::new(8.0, y + 1.25)).unwrap();
        path.close();
    }
    path.finish()
}

#[test]
fn a_side_kept_in_many_parts_is_emitted_within_the_work_bound() {
    // The curved side is kept in 1,025 parts. Scanning its 2,048 pieces
    // for each part would take about 2.1 million steps, past the work
    // bound of about 0.4 million, and keep the polygon; visiting only each
    // part's own pieces takes about 9,000 with the sweep.
    let limits = Limits::default();
    let edges = GlyphEdges::new(&interrupted(1024), [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
    assert!(matches!(edges.kind, Kind::Union { .. }));
    assert!(edges.curved());
}

#[test]
fn chains_that_never_part_keep_the_polygon() {
    // Two contours abut along one curve, which the second one draws in two
    // halves: the curves never part and share no pieces, so halving their
    // slab exceeds its work bound.
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    path.quad_to(Point::new(0.0, 8.0), Point::new(8.0, 8.0))
        .unwrap();
    path.line_to(Point::new(8.0, 0.0)).unwrap();
    path.close();
    path.move_to(Point::new(8.0, 8.0));
    path.quad_to(Point::new(4.0, 8.0), Point::new(2.0, 6.0))
        .unwrap();
    path.quad_to(Point::new(0.0, 4.0), Point::new(0.0, 0.0))
        .unwrap();
    path.line_to(Point::new(-4.0, 8.0)).unwrap();
    path.close();
    let limits = Limits::default();
    let edges = GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
    assert!(matches!(
        edges.kind,
        Kind::Polygon {
            state: PolygonState::Checked { .. } | PolygonState::Resolved { .. },
            ..
        }
    ));
}
