//! Shadings (8.7.4.5) sampled at points of shading space, through
//! `pagura-color`'s functions.
//!
//! A shading without a function (which only meshes may have) takes its
//! parametric value or vertex colour as its colour components.

use pagura_core::{Error, Result};
use pagura_interpret::{MeshVertex, Patch, Shading, ShadingFunction, ShadingKind};
use pagura_raster::{Point, Transform};

mod patch;

/// A shading ready to sample.
#[derive(Debug)]
pub(crate) struct Sampler<'a> {
    shading: &'a Shading,
    /// For function-based shadings, shading space to the function's domain.
    inverse: Transform,
}

impl<'a> Sampler<'a> {
    /// Prepares `shading`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a function-based shading with a
    /// singular matrix.
    pub(crate) fn new(shading: &'a Shading) -> Result<Self> {
        let inverse = match &shading.kind {
            ShadingKind::Function { matrix, .. } => matrix.inverse().ok_or(
                Error::invalid_argument("the reference has no singular shading matrix"),
            )?,
            ShadingKind::Axial { .. }
            | ShadingKind::Radial { .. }
            | ShadingKind::Triangles(_)
            | ShadingKind::Patches(_) => Transform::IDENTITY,
            _ => return Err(Error::invalid_argument("the reference has no such shading")),
        };
        Ok(Self { shading, inverse })
    }

    /// The shading.
    pub(crate) fn shading(&self) -> &'a Shading {
        self.shading
    }

    /// Writes the colour components at `point` (shading space) into
    /// `output` and returns whether the shading paints there.
    pub(crate) fn sample(&self, point: Point, output: &mut [f64]) -> bool {
        let mut input = [0.0; pagura_color::MAX_COMPONENTS];
        let count = match &self.shading.kind {
            ShadingKind::Function { domain, .. } => {
                let p = point * self.inverse;
                if p.x < domain[0] || p.x > domain[1] || p.y < domain[2] || p.y > domain[3] {
                    return false;
                }
                input[..2].copy_from_slice(&[p.x, p.y]);
                2
            }
            ShadingKind::Axial {
                start,
                end,
                domain,
                extend,
            } => {
                let Some(t) = axial(*start, *end, *extend, point) else {
                    return false;
                };
                input[0] = domain[0] + t * (domain[1] - domain[0]);
                1
            }
            ShadingKind::Radial {
                start,
                start_radius,
                end,
                end_radius,
                domain,
                extend,
            } => {
                let circles = Circles {
                    start: *start,
                    start_radius: *start_radius,
                    end: *end,
                    end_radius: *end_radius,
                };
                let Some(t) = circles.parameter(*extend, point) else {
                    return false;
                };
                input[0] = domain[0] + t * (domain[1] - domain[0]);
                1
            }
            ShadingKind::Triangles(triangles) => {
                let Some(count) = triangle(triangles, point, &mut input) else {
                    return false;
                };
                count
            }
            ShadingKind::Patches(patches) => {
                let Some(count) = patch_colour(patches, point, &mut input) else {
                    return false;
                };
                count
            }
            _ => return false,
        };
        self.evaluate(&input[..count], output);
        true
    }

    fn evaluate(&self, input: &[f64], output: &mut [f64]) {
        let n = self.shading.space.components().min(output.len());
        match &self.shading.function {
            None => {
                for (out, &v) in output.iter_mut().zip(input).take(n) {
                    *out = v;
                }
            }
            Some(ShadingFunction::Single(function)) => {
                let outputs = function.outputs().min(output.len());
                function.evaluate(input, &mut output[..outputs]);
            }
            Some(ShadingFunction::PerComponent(functions)) => {
                for (function, out) in functions.iter().zip(output.iter_mut()).take(n) {
                    function.evaluate(input, std::slice::from_mut(out));
                }
            }
        }
    }
}

/// The parameter `t` of an axial shading at `point`, extended and clamped
/// to `0..=1`; `None` where the shading does not paint.
fn axial(start: Point, end: Point, extend: [bool; 2], point: Point) -> Option<f64> {
    let delta = end - start;
    let denominator = delta.dot(delta);
    if denominator == 0.0 {
        return None;
    }
    parameter((point - start).dot(delta) / denominator, extend)
}

/// A `t` the extension rules admit, clamped to `0..=1`.
fn parameter(t: f64, extend: [bool; 2]) -> Option<f64> {
    (t.is_finite() && (t >= 0.0 || extend[0]) && (t <= 1.0 || extend[1])).then(|| t.clamp(0.0, 1.0))
}

/// The circles of a radial shading.
struct Circles {
    start: Point,
    start_radius: f64,
    end: Point,
    end_radius: f64,
}

impl Circles {
    /// The largest `t` whose circle passes through `point` with a
    /// non-negative radius (8.7.4.5.4).
    fn parameter(&self, extend: [bool; 2], point: Point) -> Option<f64> {
        let delta = self.end - self.start;
        let change = self.end_radius - self.start_radius;
        if self.start_radius == 0.0 && change == 0.0 {
            return None;
        }
        let p = point - self.start;
        let a = delta.dot(delta) - change * change;
        let b = -2.0 * (p.dot(delta) + self.start_radius * change);
        let c = p.dot(p) - self.start_radius * self.start_radius;
        roots(a, b, c)
            .into_iter()
            .filter(|t| t.is_finite() && self.start_radius + change * t >= 0.0)
            .filter_map(|t| parameter(t, extend))
            .max_by(f64::total_cmp)
    }
}

/// The real roots of `a t² + b t + c`, NaN where there are none.
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

/// The interpolated colour of the last triangle that contains `point`.
fn triangle(triangles: &[[MeshVertex; 3]], point: Point, out: &mut [f64]) -> Option<usize> {
    let cross = |a: Point, b: Point, c: Point| (b - a).cross(c - a);
    triangles.iter().rev().find_map(|v| {
        let (p0, p1, p2) = (v[0].point, v[1].point, v[2].point);
        let area = cross(p0, p1, p2);
        if area == 0.0 {
            return None;
        }
        let w1 = cross(p0, point, p2) / area;
        let w2 = cross(p0, p1, point) / area;
        let w0 = 1.0 - w1 - w2;
        if w0.min(w1).min(w2) < -1e-12 {
            return None;
        }
        let n = v[0]
            .color
            .len()
            .min(v[1].color.len())
            .min(v[2].color.len())
            .min(out.len());
        for (i, out) in out.iter_mut().enumerate().take(n) {
            *out = w0 * v[0].color[i] + w1 * v[1].color[i] + w2 * v[2].color[i];
        }
        Some(n)
    })
}

/// The interpolated colour of the last patch that covers `point`.
fn patch_colour(patches: &[Patch], point: Point, out: &mut [f64]) -> Option<usize> {
    patches.iter().rev().find_map(|patch| {
        let (u, v) = patch::locate(&patch.points, point)?;
        let c = &patch.colors;
        let n = c
            .iter()
            .map(pagura_color::Components::len)
            .min()
            .unwrap_or(0)
            .min(out.len());
        for (i, out) in out.iter_mut().enumerate().take(n) {
            *out = (1.0 - u) * (1.0 - v) * c[0][i]
                + (1.0 - u) * v * c[1][i]
                + u * v * c[2][i]
                + u * (1.0 - v) * c[3][i];
        }
        Some(n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axial_parameters_extend_and_clamp() {
        let (start, end) = (Point::new(0.0, 0.0), Point::new(4.0, 0.0));
        assert_eq!(
            axial(start, end, [false; 2], Point::new(1.0, 7.0)),
            Some(0.25)
        );
        assert_eq!(axial(start, end, [false; 2], Point::new(-1.0, 0.0)), None);
        assert_eq!(
            axial(start, end, [true, false], Point::new(-1.0, 0.0)),
            Some(0.0)
        );
        assert_eq!(
            axial(start, end, [false, true], Point::new(5.0, 0.0)),
            Some(1.0)
        );
    }

    #[test]
    fn the_last_containing_triangle_interpolates_its_vertices() {
        let vertex = |x: f64, y: f64, c: f64| MeshVertex {
            point: Point::new(x, y),
            color: pagura_color::Components::from_slice(&[c]),
        };
        let triangles = [
            [
                vertex(0.0, 0.0, 0.0),
                vertex(4.0, 0.0, 1.0),
                vertex(0.0, 4.0, 0.5),
            ],
            [
                vertex(0.0, 0.0, 1.0),
                vertex(1.0, 0.0, 1.0),
                vertex(0.0, 1.0, 1.0),
            ],
        ];
        let mut out = [0.0];
        // Barycentric weights 1/2, 1/4, 1/4 at (1, 1).
        assert_eq!(
            triangle(&triangles, Point::new(1.0, 1.0), &mut out),
            Some(1)
        );
        assert!((out[0] - (0.25 + 0.125)).abs() < 1e-15);
        // The later triangle wins where both contain the point.
        triangle(&triangles, Point::new(0.25, 0.25), &mut out);
        assert_eq!(out[0], 1.0);
        assert_eq!(triangle(&triangles, Point::new(3.0, 3.0), &mut out), None);
    }

    #[test]
    fn radial_parameters_take_the_largest_circle() {
        let circles = Circles {
            start: Point::new(0.0, 0.0),
            start_radius: 0.0,
            end: Point::new(0.0, 0.0),
            end_radius: 4.0,
        };
        assert_eq!(
            circles.parameter([false; 2], Point::new(3.0, 0.0)),
            Some(0.75)
        );
        assert_eq!(circles.parameter([false; 2], Point::new(5.0, 0.0)), None);
        assert_eq!(
            circles.parameter([false, true], Point::new(5.0, 0.0)),
            Some(1.0)
        );
    }
}
