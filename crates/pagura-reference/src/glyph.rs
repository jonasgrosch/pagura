//! Which reference a glyph outline has (ADR 0004 §4): its true curve when
//! the renderer integrates its quadratics analytically, its placed 1/256
//! polygon otherwise. The classification is the renderer's, written here
//! independently from its definition:
//!
//! - the outline has at most 32 segments, at least one curve, and every
//!   curve is an exact quadratic under the linear map (third difference at
//!   most 2^-24 pixels per coordinate);
//! - no subpath is a geometric point;
//! - its monotone pieces (non-horizontal lines and quadratic pieces split at
//!   their x and y extrema) number at most 32 and at most a quarter of the
//!   polygon's vertices less one;
//! - the polygon is certified: between any two vertex heights its edges
//!   alternate in direction from the left, starting with one sign common to
//!   the whole outline (so the winding is 0 or that sign), and edges that
//!   overlap in height stay at least two tolerances apart horizontally at
//!   every vertex height of either, except at a vertex they share.

use crate::flatten::{TOLERANCE, glyph_polygon};
use pagura_core::{Limits, Result};
use pagura_raster::{Path, Point, Segment, Transform};

/// A cubic whose third difference is at most this many pixels per
/// coordinate is an exact quadratic.
const REDUCIBLE: f64 = 1.0 / 16_777_216.0;

/// The most segments and monotone pieces an analytic outline has.
const MAX_PIECES: usize = 32;

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
    let curve = outline.segments().len() <= MAX_PIECES
        && outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. }));
    if !curve {
        return Ok(Reference::Polygon);
    }
    let Some(polygon) = glyph_polygon(outline, linear, limits)? else {
        return Ok(Reference::Polygon);
    };
    let vertices: usize = polygon.iter().map(Vec::len).sum();
    let cap = (vertices.saturating_sub(1) / 4).min(MAX_PIECES);
    let admitted = pieces(outline, linear).is_some_and(|(count, curved)| curved && count <= cap)
        && certified(&polygon, 2.0 * TOLERANCE);
    Ok(if admitted {
        Reference::Curve
    } else {
        Reference::Polygon
    })
}

/// The number of monotone pieces of `outline` and whether one is curved;
/// `None` when a curve is not an exact quadratic.
fn pieces(outline: &Path, linear: Transform) -> Option<(usize, bool)> {
    let mut count = 0;
    let mut curved = false;
    let line = |a: Point, b: Point, count: &mut usize| *count += usize::from(a.y != b.y);
    let (mut start, mut current) = (Point::default(), Point::default());
    let mut open = false;
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                if open {
                    line(current, start, &mut count);
                }
                start = p * linear;
                current = start;
                open = false;
            }
            Segment::LineTo(p) => {
                if !open {
                    start = current;
                }
                let p = p * linear;
                line(current, p, &mut count);
                current = p;
                open = true;
            }
            Segment::CubicTo { c1, c2, end } => {
                if !open {
                    start = current;
                }
                let [a, b, c, d] = [current, c1 * linear, c2 * linear, end * linear];
                let difference = Point::new(
                    -a.x + 3.0 * b.x - 3.0 * c.x + d.x,
                    -a.y + 3.0 * b.y - 3.0 * c.y + d.y,
                );
                if difference.x.abs() > REDUCIBLE || difference.y.abs() > REDUCIBLE {
                    return None;
                }
                let control = Point::new(
                    f64::midpoint(a.x, d.x) + 0.75 * ((b.x - a.x) + (c.x - d.x)),
                    f64::midpoint(a.y, d.y) + 0.75 * ((b.y - a.y) + (c.y - d.y)),
                );
                let (n, curve) = quadratic_pieces([a, control, d]);
                count += n;
                curved |= curve;
                current = d;
                open = true;
            }
            Segment::Close => {
                if open {
                    line(current, start, &mut count);
                    current = start;
                    open = false;
                }
            }
        }
    }
    if open {
        line(current, start, &mut count);
    }
    Some((count, curved))
}

/// The monotone pieces of a quadratic that are not horizontal, and whether
/// they are curved (a collinear quadratic gives straight pieces).
fn quadratic_pieces([p0, p1, p2]: [Point; 3]) -> (usize, bool) {
    let velocity = (p1 - p0) * 2.0;
    let acceleration = p0 - p1 * 2.0 + p2;
    let at = |t: f64| p0 + velocity * t + acceleration * (t * t);
    let mut events = vec![0.0, 1.0];
    for (v, a) in [(velocity.x, acceleration.x), (velocity.y, acceleration.y)] {
        if a != 0.0 {
            let t = -v / (2.0 * a);
            if t > 0.0 && t < 1.0 {
                events.push(t);
            }
        }
    }
    events.sort_by(f64::total_cmp);
    events.dedup();
    let pieces = events
        .windows(2)
        .filter(|t| at(t[0]).y != at(t[1]).y)
        .count();
    (pieces, velocity.cross(acceleration) != 0.0 && pieces > 0)
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
    fn small_quadratic_outlines_are_referenced_against_their_curve() {
        // A lens of two quadratics as elevated cubics, large enough that
        // its polygon has many vertices per piece.
        let elevated = |p0: Point, q: Point, p2: Point| {
            let c1 = p0 + (q - p0) * (2.0 / 3.0);
            let c2 = p2 + (q - p2) * (2.0 / 3.0);
            (c1, c2, p2)
        };
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        let (c1, c2, end) = elevated(
            Point::new(0.0, 0.0),
            Point::new(10.0, 20.0),
            Point::new(20.0, 0.0),
        );
        path.cubic_to(c1, c2, end).unwrap();
        let (c1, c2, end) = elevated(
            Point::new(20.0, 0.0),
            Point::new(10.0, -20.0),
            Point::new(0.0, 0.0),
        );
        path.cubic_to(c1, c2, end).unwrap();
        path.close();
        let lens = path.finish();
        let limits = Limits::default();
        assert_eq!(
            reference(&lens, Transform::IDENTITY, &limits).unwrap(),
            Reference::Curve
        );
        // A general cubic keeps its polygon.
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
