//! Analytic shading geometry (8.7.4.5.2–8.7.4.5.4): the function input at
//! a point of shading space, or a miss.

use super::{Input, Sampler};
use pagura_raster::{Point, Transform};

/// An axial shading (Type 2).
#[derive(Debug, Clone)]
pub(super) struct Axial {
    pub(super) start: Point,
    pub(super) delta: Point,
    /// `|delta|²`, the parameter's denominator.
    pub(super) denominator: f64,
    pub(super) domain: [f64; 2],
    pub(super) extend: [bool; 2],
}

/// A radial shading (Type 3).
#[derive(Debug, Clone)]
pub(super) struct Radial {
    start: Point,
    delta: Point,
    radius: f64,
    change: f64,
    /// `|delta|² − change²`, the quadratic's leading coefficient.
    leading: f64,
    /// `radius · change` and `radius²`, as the coefficients use them.
    radius_change: f64,
    radius_squared: f64,
    domain: [f64; 2],
    extend: [bool; 2],
}

/// A function-based shading (Type 1).
#[derive(Debug, Clone)]
pub(super) struct FunctionDomain {
    /// Shading space to the function's domain space: `Matrix` inverted.
    pub(super) inverse: Transform,
    /// `[x0, x1, y0, y1]`.
    pub(super) domain: [f64; 4],
}

impl Axial {
    pub(super) fn new(start: Point, end: Point, domain: [f64; 2], extend: [bool; 2]) -> Self {
        let delta = Point::new(end.x - start.x, end.y - start.y);
        Self {
            start,
            delta,
            denominator: delta.x * delta.x + delta.y * delta.y,
            domain,
            extend,
        }
    }

    /// The parameter `t` of the projection onto the axis, before `Extend`.
    pub(super) fn parameter(&self, point: Point) -> f64 {
        ((point.x - self.start.x) * self.delta.x + (point.y - self.start.y) * self.delta.y)
            / self.denominator
    }

    /// The function input for an extended parameter.
    pub(super) fn input(&self, t: f64) -> Option<f64> {
        let t = parameter(t, self.extend)?;
        Some(self.domain[0] + t * (self.domain[1] - self.domain[0]))
    }

    /// Bounds of every function input a sample takes.
    pub(super) fn inputs(&self) -> [f64; 2] {
        inputs(self.domain)
    }
}

impl Sampler for Axial {
    fn sample(&self, point: Point, input: &mut Input) -> bool {
        if self.denominator == 0.0 {
            return false;
        }
        let Some(x) = self.input(self.parameter(point)) else {
            return false;
        };
        input[0] = x;
        true
    }
}

impl Radial {
    pub(super) fn new(
        (start, start_radius): (Point, f64),
        (end, end_radius): (Point, f64),
        domain: [f64; 2],
        extend: [bool; 2],
    ) -> Self {
        let delta = Point::new(end.x - start.x, end.y - start.y);
        let change = end_radius - start_radius;
        Self {
            start,
            delta,
            radius: start_radius,
            change,
            leading: delta.x * delta.x + delta.y * delta.y - change * change,
            radius_change: start_radius * change,
            radius_squared: start_radius * start_radius,
            domain,
            extend,
        }
    }
}

impl Radial {
    /// Bounds of every function input a sample takes.
    pub(super) fn inputs(&self) -> [f64; 2] {
        inputs(self.domain)
    }
}

impl Sampler for Radial {
    fn sample(&self, point: Point, input: &mut Input) -> bool {
        if self.radius == 0.0 && self.change == 0.0 {
            return false;
        }
        let x = point.x - self.start.x;
        let y = point.y - self.start.y;
        let b = -2.0 * (x * self.delta.x + y * self.delta.y + self.radius_change);
        let c = x * x + y * y - self.radius_squared;
        let t = roots(self.leading, b, c)
            .into_iter()
            .filter(|t| t.is_finite() && self.radius + self.change * t >= 0.0)
            .filter_map(|t| parameter(t, self.extend))
            .max_by(f64::total_cmp);
        let Some(t) = t else {
            return false;
        };
        input[0] = self.domain[0] + t * (self.domain[1] - self.domain[0]);
        true
    }
}

impl Sampler for FunctionDomain {
    fn sample(&self, point: Point, input: &mut Input) -> bool {
        let p = point * self.inverse;
        let d = self.domain;
        if p.x < d[0] || p.x > d[1] || p.y < d[2] || p.y > d[3] {
            return false;
        }
        input[0] = p.x;
        input[1] = p.y;
        true
    }
}

/// Bounds in IEEE total order of `d0 + t·(d1 − d0)` for every extended
/// parameter `t`, which lies in `−0..=1`: the inputs rise or fall with `t`.
fn inputs(domain: [f64; 2]) -> [f64; 2] {
    let mut ends = [-0.0, 0.0, 1.0].map(|t| domain[0] + t * (domain[1] - domain[0]));
    ends.sort_by(f64::total_cmp);
    [ends[0], ends[2]]
}

/// `t` clipped to `0..=1` where `Extend` allows it, or `None`.
pub(super) fn parameter(t: f64, extend: [bool; 2]) -> Option<f64> {
    (t.is_finite() && (t >= 0.0 || extend[0]) && (t <= 1.0 || extend[1])).then(|| t.clamp(0.0, 1.0))
}

/// The roots of `a·t² + b·t + c` by the stable formula; the linear case
/// avoids division by zero, and no real root gives NaN.
fn roots(a: f64, b: f64, c: f64) -> [f64; 2] {
    if a == 0.0 {
        return if b == 0.0 { [f64::NAN; 2] } else { [-c / b; 2] };
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return [f64::NAN; 2];
    }
    let q = -0.5 * (b + discriminant.sqrt().copysign(b));
    if q == 0.0 {
        [-b / (2.0 * a); 2]
    } else {
        [q / a, c / q]
    }
}
