//! Analytic quadratic coverage (ISO 32000-2, 8.5.2.2 and 8.5.3.3).
//!
//! Curves are split at both coordinate extrema. Each monotone piece crosses a
//! pixel boundary at most once. Integrating x(t)y'(t) between those crossings
//! gives the cell area, without approximating the curve by a polygon.
//!
//! Only admitted glyphs reach this module. Its sweep is separate from the line
//! sweep, so fills without curves keep exactly the line sweep's code.

use super::{Edge, Geometry, Row, Shape, index};
use crate::Point;
use pagura_core::Result;

pub(crate) mod build;
mod intersections;
mod sweep;

pub(super) use sweep::Sweep;

/// One monotone boundary piece: a line, or a quadratic whose chord is `edge`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Piece {
    edge: Edge,
    control: Option<Point>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Quadratic {
    points: [Point; 3],
}

impl Piece {
    fn line(edge: Edge) -> Self {
        Self {
            edge,
            control: None,
        }
    }

    fn quadratic(self) -> Option<Quadratic> {
        self.control.map(|control| Quadratic {
            points: [self.edge.a, control, self.edge.b],
        })
    }

    pub(crate) fn is_curved(self) -> bool {
        self.control.is_some()
    }

    pub(crate) fn endpoints(self) -> [Point; 2] {
        [self.edge.a, self.edge.b]
    }

    fn x(self, y: f64) -> f64 {
        self.quadratic()
            .map_or_else(|| self.edge.x(y), |curve| curve.at_y(y).x)
    }
}

impl Quadratic {
    fn at(self, t: f64) -> Point {
        let [a, b, c] = self.points;
        lerp(lerp(a, b, t), lerp(b, c, t), t)
    }

    fn coefficients(self) -> [Point; 3] {
        let [a, b, c] = self.points;
        [a, scale(sub(b, a), 2.0), sub(sub(c, b), sub(b, a))]
    }

    fn interval(self, lo: f64, hi: f64) -> Self {
        let [a, b, c] = self.points;
        let tangent = lerp(sub(b, a), sub(c, b), lo);
        let start = self.at(lo);
        Self {
            points: [start, add(start, scale(tangent, hi - lo)), self.at(hi)],
        }
    }

    fn parameter(self, value: f64, x_axis: bool) -> f64 {
        let axis = |p: Point| if x_axis { p.x } else { p.y };
        let [p, q, r] = self.points.map(axis);
        if p == r {
            return 0.0;
        }
        // Reflect descending coordinates: the initial derivative is then
        // nonnegative, so this root avoids subtracting nearly equal numbers.
        let sign = (r - p).signum();
        let delta = ((value - p) * sign).clamp(0.0, (r - p).abs());
        if delta == 0.0 {
            return 0.0;
        }
        if delta == (r - p).abs() {
            return 1.0;
        }
        let linear = 2.0 * (q - p) * sign;
        let square = ((r - q) - (q - p)) * sign;
        (2.0 * delta / (linear + (linear * linear + 4.0 * square * delta).max(0.0).sqrt()))
            .clamp(0.0, 1.0)
    }

    fn at_y(self, y: f64) -> Point {
        self.at(self.parameter(y, false))
    }

    // A local polynomial avoids subtracting large antiderivatives on a page
    // translated far from the origin. This is integral (x - column) dy.
    fn integral(self, column: f64) -> f64 {
        let [p, v, a] = self.coefficients();
        (p.x - column) * (self.points[2].y - p.y)
            + v.x * (v.y * 0.5 + a.y * (2.0 / 3.0))
            + a.x * (v.y / 3.0 + a.y * 0.5)
    }

    fn cell(self, lo: f64, hi: f64, column: f64) -> f64 {
        let a = self.at(lo);
        let b = self.at(hi);
        if a.x == b.x {
            return (b.y - a.y) * (a.x - column).clamp(0.0, 1.0);
        }
        let left = self.parameter(column, true).clamp(lo, hi);
        let right = self.parameter(column + 1.0, true).clamp(lo, hi);
        let (start, end, full) = if a.x < b.x {
            (left, right, b.y - self.at(right).y)
        } else {
            (right, left, self.at(right).y - a.y)
        };
        full + self.interval(start, end).integral(column)
    }
}

// The quadratic counterpart of `Row::edge`: the same cover differences, with
// each touched cell's area integrated under the curve instead of its chord.
fn integrate(row: &mut Row, curve: Quadratic, lo: f64, hi: f64, sign: i32) -> Result<()> {
    if sign == 0 {
        return Ok(());
    }
    let start_t = curve.parameter(lo, false);
    // Rounding can invert the parameters of nearly equal heights.
    let end_t = curve.parameter(hi, false).max(start_t);
    let a = curve.at(start_t).x - row.origin;
    let b = curve.at(end_t).x - row.origin;
    let start = index(a.min(b).floor(), row.areas.len());
    let end = index(a.max(b).ceil(), row.areas.len());
    let area = (hi - lo) * f64::from(sign);
    row.charge(end.saturating_sub(start) + 1)?;
    if start > 0 {
        if let Some(first) = row.covers.first_mut() {
            *first += area;
            row.touched[0] |= 1;
        }
        if let Some(after) = row.covers.get_mut(start) {
            *after -= area;
            row.touched[start / 64] |= 1 << (start % 64);
        }
    }
    for x in start..end {
        let column = x as f64 + row.origin;
        row.areas[x] += f64::from(sign) * curve.cell(start_t, end_t, column);
        row.touched[x / 64] |= 1 << (x % 64);
    }
    Ok(())
}

fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}
fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}
fn scale(a: Point, value: f64) -> Point {
    Point::new(a.x * value, a.y * value)
}
fn lerp(a: Point, b: Point, t: f64) -> Point {
    add(a, scale(sub(b, a), t))
}
fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}

// Retained geometry, rather than quantized mask bytes or tile bounds, decides.
pub(crate) fn rectangular_clips(shapes: &[Shape]) -> bool {
    shapes.iter().all(|clip| clip.rectangle().is_some())
}

// Only the painted glyph is eligible; the complete retained clip stack must be
// rectangular. Selection uses canonical geometry and never viewport bounds.
pub(super) fn anchor(shapes: &[Shape]) -> Option<Point> {
    let shape = shapes.first()?;
    let Geometry::Glyph(glyph) = &shape.geometry else {
        return None;
    };
    glyph.curves()?;
    let t = shape.transform;
    if [t.a, t.b, t.c, t.d] != [1.0, 0.0, 0.0, 1.0]
        || !t.is_finite()
        || !rectangular_clips(&shapes[1..])
    {
        return None;
    }
    Some(Point::new(t.e.floor(), t.f.floor()))
}

#[cfg(test)]
mod tests;
