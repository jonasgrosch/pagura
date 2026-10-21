//! Glyph outlines flattened to edges under a linear map. Curves become
//! chords within a device tolerance ([`Flattening::Chords`]), or two chords
//! that keep each piece's signed area ([`Flattening::Balanced`]); neither is
//! exact (ADR 0004).

use pagura_core::{Error, Limits, Result};
use pagura_raster::{Path, Point, Segment, Transform};

/// Byte coverage at a device-pixel offset, owning its levels or borrowing
/// them from shared storage.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Coverage<D = Vec<u8>> {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: usize,
    pub(crate) data: D,
}

/// Coverage borrowing its levels.
pub(crate) type View<'a> = Coverage<&'a [u8]>;

impl<D: AsRef<[u8]>> Coverage<D> {
    /// The same coverage, borrowing its levels.
    pub(super) fn view(&self) -> View<'_> {
        Coverage {
            x: self.x,
            y: self.y,
            width: self.width,
            data: self.data.as_ref(),
        }
    }
}

/// How curves become edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Flattening {
    /// Recursive subdivision until each piece's endpoint chord lies within
    /// this device tolerance.
    Chords(f64),
    /// The same subdivision, then each piece becomes two chords through a
    /// vertex placed so that they enclose the piece's signed area.
    Balanced(f64),
}

/// A path flattened once under a linear map, without its translation.
#[derive(Debug)]
pub(super) struct Geometry {
    pub(super) edges: Vec<(Point, Point)>,
    pub(super) min: Point,
    pub(super) max: Point,
}

impl Geometry {
    pub(super) fn new(
        path: &Path,
        transform: Transform,
        flattening: Flattening,
        limits: Limits,
    ) -> Result<Self> {
        let transform = Transform {
            e: 0.0,
            f: 0.0,
            ..transform
        };
        let (mut min, mut max) = bounds(path, transform, limits)?;
        let flatten_limits = Limits {
            path_segments: limits
                .path_segments
                .min(limits.decoded_bytes / (2 * size_of::<(Point, Point)>())),
            ..limits
        };
        let edges = flattened_edges(path, transform, flattening, &flatten_limits)?;
        edges
            .len()
            .checked_mul(size_of::<(Point, Point)>())
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or(Error::limit("glyph geometry allocation budget exceeded"))?;
        if matches!(flattening, Flattening::Balanced(_)) {
            // A balanced vertex may leave the control polygon's bounds.
            min = Point::new(f64::INFINITY, f64::INFINITY);
            max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
            for p in edges.iter().flat_map(|&(a, b)| [a, b]) {
                if !p.is_finite() || p.x.abs().max(p.y.abs()) > 1e8 {
                    return Err(Error::limit("coverage coordinate range exceeded"));
                }
                min.x = min.x.min(p.x);
                min.y = min.y.min(p.y);
                max.x = max.x.max(p.x);
                max.y = max.y.max(p.y);
            }
        }
        Ok(Self { edges, min, max })
    }
}

fn bounds(path: &Path, transform: Transform, limits: Limits) -> Result<(Point, Point)> {
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    if path.segments().len() > limits.path_segments || !transform.is_finite() {
        return Err(Error::limit("coverage geometry budget exceeded"));
    }
    for segment in path.segments() {
        let points: &[Point] = match segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => std::slice::from_ref(p),
            Segment::CubicTo {
                c1: a,
                c2: b,
                end: c,
            } => &[*a, *b, *c],
            Segment::Close => &[],
        };
        for &point in points {
            let p = point * transform;
            if !p.is_finite() || p.x.abs().max(p.y.abs()) > 1e8 {
                return Err(Error::limit("coverage coordinate range exceeded"));
            }
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
    }
    Ok((min, max))
}

/// Device points snap to a 1/4096-pixel grid, so last-bit differences in
/// upstream geometry (stroke joins use platform `sin`, `cos` and `hypot`)
/// cannot move a coverage rounding boundary between platforms.
fn snap(p: Point) -> Point {
    const GRID: f64 = 4096.0;
    Point::new((p.x * GRID).round() / GRID, (p.y * GRID).round() / GRID)
}

fn flattened_edges(
    path: &Path,
    local: Transform,
    flattening: Flattening,
    limits: &Limits,
) -> Result<Vec<(Point, Point)>> {
    let mut edges = Vec::new();
    let mut current = Point::default();
    let mut first = None;
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                if let Some(start) = first {
                    edges.push((current, start));
                }
                current = snap(p * local);
                first = Some(current);
            }
            Segment::LineTo(p) => {
                let end = snap(p * local);
                edges.push((current, end));
                current = end;
            }
            Segment::CubicTo {
                c1: a,
                c2: b,
                end: c,
            } => {
                flatten(
                    [current, snap(a * local), snap(b * local), snap(c * local)],
                    flattening,
                    0,
                    &mut edges,
                    limits,
                )?;
                current = snap(c * local);
            }
            Segment::Close => {
                if let Some(start) = first {
                    edges.push((current, start));
                    current = start;
                }
            }
        }
        if edges.len() > limits.path_segments {
            return Err(Error::limit("coverage edge budget exceeded"));
        }
    }
    if let Some(start) = first {
        edges.push((current, start));
    }
    if edges.len() > limits.path_segments {
        return Err(Error::limit("coverage edge budget exceeded"));
    }
    Ok(edges)
}

fn flatten(
    p: [Point; 4],
    flattening: Flattening,
    depth: usize,
    edges: &mut Vec<(Point, Point)>,
    limits: &Limits,
) -> Result<()> {
    if edges.len() >= limits.path_segments || depth > limits.depth.min(64) {
        return Err(Error::limit("coverage flattening budget exceeded"));
    }
    let (Flattening::Chords(tolerance) | Flattening::Balanced(tolerance)) = flattening;
    let balanced = matches!(flattening, Flattening::Balanced(_));
    let [a, b, c, d] = p;
    let dx = d.x - a.x;
    let dy = d.y - a.y;
    let len = dx * dx + dy * dy;
    let distance = |p: Point| {
        let cross = (p.x - a.x) * dy - (p.y - a.y) * dx;
        cross * cross
    };
    let flat = if len < 1e-20 {
        [b, c]
            .iter()
            .all(|p| (p.x - a.x).abs().max((p.y - a.y).abs()) <= tolerance)
    } else {
        distance(b).max(distance(c)) <= tolerance * tolerance * len
            && [b, c].iter().all(|p| {
                let t = (p.x - a.x) * dx + (p.y - a.y) * dy;
                t >= 0.0 && t <= len
            })
    };
    // A balanced piece needs a chord to move its vertex across, so a closed
    // loop subdivides until it is a point.
    let flat = flat && (!balanced || len >= 1e-20 || (a == b && a == c && a == d));
    if flat {
        if balanced && len >= 1e-20 {
            if edges.len().saturating_add(2) > limits.path_segments {
                return Err(Error::limit("coverage flattening budget exceeded"));
            }
            let middle = balanced_vertex(p);
            edges.push((a, middle));
            edges.push((middle, d));
        } else {
            edges.push((a, d));
        }
        return Ok(());
    }
    let mid = |a: Point, b: Point| Point::new(a.x.midpoint(b.x), a.y.midpoint(b.y));
    let ab = mid(a, b);
    let bc = mid(b, c);
    let cd = mid(c, d);
    let abc = mid(ab, bc);
    let bcd = mid(bc, cd);
    let m = mid(abc, bcd);
    flatten([a, ab, abc, m], flattening, depth + 1, edges, limits)?;
    flatten([m, bcd, cd, d], flattening, depth + 1, edges, limits)
}

/// The vertex of two chords from `a` to `d` that enclose the cubic's signed
/// area against the chord `ad`. Integrating the Bernstein basis gives that
/// area; the vertex starts at the curve's midpoint and moves along the
/// chord's normal until its triangle has it.
fn balanced_vertex([a, b, c, d]: [Point; 4]) -> Point {
    let relative = |p: Point| Point::new(p.x - a.x, p.y - a.y);
    let (b, c, d) = (relative(b), relative(c), relative(d));
    let cross = |p: Point, q: Point| p.x * q.y - p.y * q.x;
    let area = ((cross(b, c) + cross(b, d)) * 3.0 + cross(c, d) * 6.0) / 20.0;
    let middle = Point::new(
        (3.0 * b.x + 3.0 * c.x + d.x) / 8.0,
        (3.0 * b.y + 3.0 * c.y + d.y) / 8.0,
    );
    let shift = 2.0 * (cross(middle, d) * 0.5 - area) / (d.x * d.x + d.y * d.y);
    Point::new(a.x + middle.x - d.y * shift, a.y + middle.y + d.x * shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_pieces_keep_independent_polynomial_areas_within_edge_budgets() {
        let shoelace = |edges: &[(Point, Point)]| -> f64 {
            edges
                .iter()
                .map(|(a, b)| (a.x * b.y - a.y * b.x) * 0.5)
                .sum()
        };
        let a = Point::new(0.0, 0.0);
        let b = Point::new(0.0, 1.0);
        let c = Point::new(1.0, 1.0);
        let d = Point::new(1.0, 0.0);
        // x = 3t^2 - 2t^3, y = 3t - 3t^2 encloses 3/5 with its chord.
        let middle = balanced_vertex([a, b, c, d]);
        assert!((middle.x - 0.5).abs() < 1e-12 && (middle.y - 1.2).abs() < 1e-12);
        let mut edges = Vec::new();
        let balanced = Flattening::Balanced(1.0);
        flatten([a, b, c, d], balanced, 0, &mut edges, &Limits::default()).unwrap();
        assert!((shoelace(&edges) + 0.6).abs() < 1e-12);
        let one = Limits {
            path_segments: 1,
            ..Limits::default()
        };
        assert!(flatten([a, b, c, d], balanced, 0, &mut Vec::new(), &one).is_err());
        // The closed cubic with controls (0,1) and (1,0) encloses 3/20.
        edges.clear();
        flatten([a, b, d, a], balanced, 0, &mut edges, &Limits::default()).unwrap();
        assert!((shoelace(&edges) + 0.15).abs() < 1e-12);
    }
}
