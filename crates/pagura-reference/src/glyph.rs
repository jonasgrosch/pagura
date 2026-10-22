//! Which reference a glyph outline has (ADR 0004 §4): its true curve when
//! the renderer integrates a spline of it, its placed 1/256 polygon
//! otherwise. The admission is the renderer's, written here independently
//! from its definition; the true curve is integrated from a fine
//! flattening of the outline itself, never from the renderer's spline:
//!
//! - the linear map is regular and the outline has a curve;
//! - no subpath is a geometric point;
//! - the outline is certified: between any two vertex heights of its
//!   polygon the edges alternate in direction from the left, starting with
//!   one sign common to the whole outline (so the winding is 0 or that
//!   sign), and edges that overlap in height stay at least two tolerances
//!   apart horizontally at every vertex height of either, except at a
//!   vertex they share. The renderer certifies its spline, which lies
//!   within 2^-10 pixel of the curve, so the two agree but for parts of an
//!   outline that come within a few hundredths of a pixel of each other.

use crate::flatten::{TOLERANCE, glyph_polygon};
use pagura_core::{Limits, Result};
use pagura_raster::{Path, Point, Segment, Transform};

/// The reference of a glyph outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reference {
    /// The true curve.
    Curve,
    /// The placed polygon at the documented tolerance.
    Polygon,
}

/// The reference of `outline` under the translation-free map `linear`.
///
/// # Errors
///
/// Returns the flattener's `InvalidArgument` and `Limit` errors.
pub(crate) fn reference(outline: &Path, linear: Transform, limits: &Limits) -> Result<Reference> {
    let regular = linear.a * linear.d - linear.b * linear.c != 0.0;
    let curve = outline
        .segments()
        .iter()
        .any(|s| matches!(s, Segment::CubicTo { .. }));
    if !regular || !curve {
        return Ok(Reference::Polygon);
    }
    let Some(polygon) = glyph_polygon(outline, linear, limits)? else {
        return Ok(Reference::Polygon);
    };
    Ok(if certified(&polygon, 2.0 * TOLERANCE) {
        Reference::Curve
    } else {
        Reference::Polygon
    })
}

/// An edge from its lower to its upper end in y, with its direction.
#[derive(Debug, Clone, Copy)]
struct Edge {
    low: Point,
    high: Point,
    up: bool,
}

impl Edge {
    fn x(&self, y: f64) -> f64 {
        if y <= self.low.y {
            self.low.x
        } else if y >= self.high.y {
            self.high.x
        } else {
            self.low.x + (y - self.low.y) / (self.high.y - self.low.y) * (self.high.x - self.low.x)
        }
    }

    fn shares(&self, other: &Self, y: f64) -> bool {
        [self.low, self.high]
            .iter()
            .any(|p| p.y == y && (*p == other.low || *p == other.high))
    }
}

/// Whether the polygon's winding is 0 or one common sign of magnitude one
/// everywhere, with overlapping edges at least `margin` apart.
fn certified(polygon: &[Vec<Point>], margin: f64) -> bool {
    let mut edges = Vec::new();
    for contour in polygon {
        for (i, &a) in contour.iter().enumerate() {
            let b = contour[(i + 1) % contour.len()];
            if a.y != b.y {
                let (low, high, up) = if a.y < b.y {
                    (a, b, true)
                } else {
                    (b, a, false)
                };
                edges.push(Edge { low, high, up });
            }
        }
    }
    let mut heights: Vec<f64> = edges.iter().flat_map(|e| [e.low.y, e.high.y]).collect();
    heights.sort_by(f64::total_cmp);
    heights.dedup();
    let mut sign = None;
    for pair in heights.windows(2) {
        let middle = f64::midpoint(pair[0], pair[1]);
        // Heights an ulp apart bound no slab: their midpoint is one of them.
        if !(pair[0] < middle && middle < pair[1]) {
            continue;
        }
        let mut active: Vec<&Edge> = edges
            .iter()
            .filter(|e| e.low.y < middle && e.high.y > middle)
            .collect();
        active.sort_by(|a, b| a.x(middle).total_cmp(&b.x(middle)));
        let Some(first) = active.first() else {
            continue;
        };
        if *sign.get_or_insert(first.up) != first.up
            || active.windows(2).any(|w| w[0].up == w[1].up)
        {
            return false;
        }
    }
    for (i, a) in edges.iter().enumerate() {
        for b in &edges[i + 1..] {
            let (low, high) = (a.low.y.max(b.low.y), a.high.y.min(b.high.y));
            if low >= high {
                continue;
            }
            let mut side = None;
            for y in [a.low.y, a.high.y, b.low.y, b.high.y] {
                if y < low || y > high || a.shares(b, y) {
                    continue;
                }
                let gap = a.x(y) - b.x(y);
                if gap.abs() < margin || *side.get_or_insert(gap > 0.0) != (gap > 0.0) {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::PathBuilder;

    /// A ring of two squares: the outer one way, the inner the other.
    fn ring(inner: [f64; 2], reversed: bool) -> Path {
        let mut path = PathBuilder::new();
        path.polygon(
            [(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0)].map(|(x, y)| Point::new(x, y)),
        );
        let [lo, hi] = inner;
        let mut hole = [(lo, lo), (lo, hi), (hi, hi), (hi, lo)].map(|(x, y)| Point::new(x, y));
        if reversed {
            hole.reverse();
        }
        path.polygon(hole);
        path.finish()
    }

    #[test]
    fn certificates_need_one_sign_and_separation() {
        let polygon = |path: &Path| {
            glyph_polygon(path, Transform::IDENTITY, &Limits::default())
                .unwrap()
                .unwrap()
        };
        assert!(certified(
            &polygon(&ring([2.0, 6.0], false)),
            2.0 * TOLERANCE
        ));
        // Both squares wound the same way: winding 2 inside the hole.
        assert!(!certified(
            &polygon(&ring([2.0, 6.0], true)),
            2.0 * TOLERANCE
        ));
        // A hole closer than two tolerances to the outer edge.
        assert!(!certified(
            &polygon(&ring([0.005, 6.0], false)),
            2.0 * TOLERANCE
        ));
    }

    #[test]
    fn certified_curved_outlines_are_referenced_against_their_curve() {
        let limits = Limits::default();
        // A general cubic lens: certified, so its true curve.
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.cubic_to(
            Point::new(2.0, 20.0),
            Point::new(14.0, 24.0),
            Point::new(20.0, 0.0),
        )
        .unwrap();
        path.close();
        let lens = path.finish();
        assert_eq!(
            reference(&lens, Transform::IDENTITY, &limits).unwrap(),
            Reference::Curve
        );
        // A singular map keeps its polygon.
        let flat = Transform::new(1.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        assert_eq!(reference(&lens, flat, &limits).unwrap(), Reference::Polygon);
        // A self-crossing cubic winds both ways: its polygon.
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.cubic_to(
            Point::new(0.0, 20.0),
            Point::new(20.0, -20.0),
            Point::new(20.0, 0.0),
        )
        .unwrap();
        path.close();
        assert_eq!(
            reference(&path.finish(), Transform::IDENTITY, &limits).unwrap(),
            Reference::Polygon
        );
        // Straight outlines have no curve to integrate.
        assert_eq!(
            reference(&ring([2.0, 6.0], false), Transform::IDENTITY, &limits).unwrap(),
            Reference::Polygon
        );
    }
}
