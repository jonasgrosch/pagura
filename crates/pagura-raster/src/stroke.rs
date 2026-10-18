//! Stroke outlines in user space (ISO 32000-2, 8.4.3 and 8.5.3).
//!
//! Positively oriented component polygons form a union under nonzero fill.
//! Flattening is measured after the CTM; constructing the pen before the CTM
//! preserves elliptical pens under nonuniform transforms.

use crate::coverage::RASTER_WORK;
use crate::flatten::{TOLERANCE, bounded, flatten};
use crate::{LineCap, LineJoin, Path, PathBuilder, Point, Stroke, Transform};
use pagura_core::{Error, Limits, Meter, Result};
use std::f64::consts::TAU;

/// A dash retains its tangent even when its length is zero (8.5.3.2).
pub(crate) struct Run {
    pub(crate) points: Vec<Point>,
    pub(crate) closed: bool,
    tangent: Point,
}
impl Run {
    pub(crate) fn transformed(mut self, transform: Transform) -> Self {
        for point in &mut self.points {
            *point = *point * transform;
        }
        // Scale the linear part before normalization to avoid overflow even
        // when a large finite CTM maps a very small user-space segment.
        let scale = transform
            .a
            .abs()
            .max(transform.b.abs())
            .max(transform.c.abs())
            .max(transform.d.abs());
        if scale > 0.0 {
            let x = (transform.a / scale) * self.tangent.x + (transform.c / scale) * self.tangent.y;
            let y = (transform.b / scale) * self.tangent.x + (transform.d / scale) * self.tangent.y;
            let tangent = Point::new(x, y);
            let length = tangent.length();
            if length > 0.0 {
                self.tangent = tangent / length;
            }
        }
        self
    }
}

pub(crate) fn outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    validate(stroke, limits)?;
    // Dash distances belong to user space even for a device-width hairline
    // (8.4.3.6). Flatten with a device-space error bound, split there, and only
    // then transform each run before constructing the hairline pen.
    let contours = flatten(path, transform, limits)?;
    let hairline = stroke.width == 0.0;
    let pen_transform = if hairline {
        Transform::IDENTITY
    } else {
        transform
    };
    let width = if hairline { 1.0 } else { stroke.width };
    let mut builder = PathBuilder::new();
    let radius = width * 0.5;
    let stretch = pen_transform.a.hypot(pen_transform.b) + pen_transform.c.hypot(pen_transform.d);
    let tolerance = if stretch == 0.0 {
        TOLERANCE
    } else {
        TOLERANCE / stretch
    };
    let mut pen = Pen::new(&mut builder, radius, tolerance, stroke, limits);
    for contour in contours {
        if !contour.strokeable(stroke.cap) {
            continue;
        }
        let runs = dash(contour.points, contour.closed, stroke, limits)?;
        for run in runs {
            let run = if hairline {
                run.transformed(transform)
            } else {
                run
            };
            pen.draw(&run)?;
        }
    }
    Ok((builder.finish(), pen_transform))
}

pub(crate) fn validate(stroke: &Stroke, limits: &Limits) -> Result<()> {
    if !stroke.width.is_finite()
        || stroke.width < 0.0
        || !stroke.miter_limit.is_finite()
        || stroke.miter_limit < 1.0
        || !stroke.dash_phase.is_finite()
        || stroke.dash_array.iter().any(|&n| !n.is_finite() || n < 0.0)
    {
        return Err(Error::invalid_argument("invalid stroke parameters"));
    }
    if stroke.dash_array.len() > limits.path_segments {
        return Err(Error::limit("dash array budget exceeded"));
    }
    if !stroke.dash_array.is_empty() && stroke.dash_array.iter().all(|&n| n == 0.0) {
        return Err(Error::invalid_argument("dash array is all zero"));
    }
    Ok(())
}

pub(crate) fn dash(
    mut points: Vec<Point>,
    closed: bool,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<Vec<Run>> {
    if stroke.dash_array.is_empty() {
        return Ok(vec![Run {
            points,
            closed,
            tangent: Point::new(1.0, 0.0),
        }]);
    }
    if closed && let Some(&first) = points.first() {
        points.push(first);
    }
    let cycle_count = stroke
        .dash_array
        .len()
        .checked_mul(2)
        .ok_or(Error::limit("dash cycle size overflow"))?;
    let length = 2.0 * stroke.dash_array.iter().sum::<f64>();
    if !length.is_finite() {
        return Err(Error::limit("dash cycle exceeds numeric range"));
    }
    // Twice the array sum restores both the length and on/off parity, including
    // odd arrays (8.4.3.6). Euclidean reduction handles large negative phases
    // without repeated increments. An empty array returned before this step.
    let phase = stroke.dash_phase.rem_euclid(length);
    let mut phase = if phase == length { 0.0 } else { phase };
    let mut index = 0;
    while phase > 0.0 && phase >= dash_length(stroke, index) {
        phase -= dash_length(stroke, index);
        index = (index + 1) % cycle_count;
    }
    let initial_on = index % 2 == 0;
    let mut remaining = dash_length(stroke, index) - phase;
    let mut result = Vec::new();
    let mut run = Vec::new();
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let mut tangent = Point::new(1.0, 0.0);
    for pair in points.windows(2) {
        let [a, b] = [pair[0], pair[1]];
        let distance = (b - a).length();
        if distance == 0.0 {
            if index % 2 == 0 {
                run.extend([a, b]);
            }
            continue;
        }
        tangent = (b - a) / distance;
        let mut offset = 0.0;
        while offset < distance || remaining == 0.0 {
            work.charge(1)?;
            if result.len() + run.len() > limits.path_segments {
                return Err(Error::limit("dashed segment budget exceeded"));
            }
            let step = remaining.min(distance - offset);
            let end = offset + step;
            if step > 0.0 && end == offset {
                return Err(Error::limit("dash lengths below numeric resolution"));
            }
            let interpolate = |d: f64| a.lerp(b, d / distance);
            if index % 2 == 0 {
                if run.is_empty() {
                    run.push(interpolate(offset));
                }
                run.push(interpolate(end));
            }
            offset = end;
            remaining -= step;
            if remaining == 0.0 {
                if index % 2 == 0 && !run.is_empty() {
                    result.push(Run {
                        points: std::mem::take(&mut run),
                        closed: false,
                        tangent,
                    });
                }
                index = (index + 1) % cycle_count;
                remaining = dash_length(stroke, index);
            }
        }
    }
    if !run.is_empty() {
        result.push(Run {
            points: run,
            closed: false,
            tangent,
        });
    }
    join_seam(&mut result, &points, closed && initial_on);
    Ok(result)
}

fn join_seam(result: &mut Vec<Run>, points: &[Point], closed: bool) {
    // A dash crossing the closing vertex has a join, not a pair of caps.
    let seam = points.first();
    let starts = result.first().and_then(|run| run.points.first()) == seam;
    let ends = result.last().and_then(|run| run.points.last()) == seam;
    if closed && starts && ends && !result.is_empty() {
        if result.len() == 1 {
            if let Some(run) = result.first_mut() {
                run.closed = true;
                run.points.pop();
            }
        } else if let Some(mut tail) = result.pop()
            && let Some(head) = result.first_mut()
        {
            tail.points.extend(head.points.iter().skip(1).copied());
            *head = tail;
        }
    }
}

fn dash_length(stroke: &Stroke, index: usize) -> f64 {
    stroke
        .dash_array
        .get(index % stroke.dash_array.len())
        .copied()
        .unwrap_or(0.0)
}

/// A circular pen of `radius` that traces each run's component polygons
/// (segment bodies, joins and caps) into an outline.
pub(crate) struct Pen<'a> {
    out: &'a mut PathBuilder,
    radius: f64,
    /// Flattening tolerance for arcs, in pen space.
    tolerance: f64,
    stroke: &'a Stroke,
    limits: &'a Limits,
    /// Vertex buffer reused by round joins and round caps.
    arc: Vec<Point>,
}

impl<'a> Pen<'a> {
    pub(crate) fn new(
        out: &'a mut PathBuilder,
        radius: f64,
        tolerance: f64,
        stroke: &'a Stroke,
        limits: &'a Limits,
    ) -> Self {
        Self {
            out,
            radius,
            tolerance,
            stroke,
            limits,
            arc: Vec::new(),
        }
    }

    pub(crate) fn draw(&mut self, run: &Run) -> Result<()> {
        let closed = run.closed;
        let mut points = Vec::new();
        for &point in &run.points {
            if points.last() != Some(&point) {
                points.push(point);
            }
        }
        if closed && points.len() > 1 && points.first() == points.last() {
            points.pop();
        }
        let (first, last) = match points[..] {
            [] => return Ok(()),
            // A zero-length run is a dot that keeps its tangent (8.5.3.2).
            [point] => {
                self.cap(point, run.tangent)?;
                return self.cap(point, -run.tangent);
            }
            [first, .., last] => (first, last),
        };
        let mut directions = Vec::new();
        for (&a, &b) in points
            .iter()
            .zip(points.iter().skip(1).chain(closed.then_some(&first)))
        {
            let d = (b - a) / (b - a).length();
            let n = d.perp() * self.radius;
            polygon(self.out, &[a + n, b + n, b - n, a - n], self.limits)?;
            directions.push(d);
        }
        for (i, &point) in points.iter().enumerate() {
            let before = if i == 0 {
                if closed { directions.last() } else { None }
            } else {
                directions.get(i - 1)
            };
            match (before, directions.get(i)) {
                (Some(&a), Some(&b)) => self.join(point, [a, b])?,
                (None, Some(&d)) => self.cap(first, -d)?,
                (Some(&d), None) => self.cap(last, d)?,
                (None, None) => {}
            }
        }
        Ok(())
    }

    fn join(&mut self, point: Point, tangents: [Point; 2]) -> Result<()> {
        let [a, b] = tangents;
        let cross = a.cross(b);
        let join = self.stroke.join;
        if cross == 0.0 && (join != LineJoin::Round || a.dot(b) >= 0.0) {
            return Ok(());
        }
        let side = if cross > 0.0 {
            -self.radius
        } else {
            self.radius
        };
        let n1 = a.perp() * side;
        let n2 = b.perp() * side;
        if join == LineJoin::Round {
            return self.round_join(point, [n1, n2], cross);
        }
        let p1 = point + n1;
        let p2 = point + n2;
        let denominator = 1.0 + a.dot(b);
        if join == LineJoin::Miter && denominator > 0.0 {
            let m = (n1 + n2) / denominator;
            if m.length() <= self.radius * self.stroke.miter_limit {
                return polygon(self.out, &[point, p1, point + m, p2], self.limits);
            }
        }
        polygon(self.out, &[point, p1, p2], self.limits)
    }

    fn round_join(&mut self, point: Point, normals: [Point; 2], cross: f64) -> Result<()> {
        let [a, b] = normals;
        let start = a.y.atan2(a.x);
        let end = b.y.atan2(b.x);
        let angle = if cross > 0.0 {
            (end - start).rem_euclid(TAU)
        } else {
            -(start - end).rem_euclid(TAU)
        };
        let count = self.arc_segments(angle, 1.0)?;
        self.arc.clear();
        self.arc.extend([point, point + a]);
        for i in 1..count {
            let angle = start + angle * f64::from(i) / f64::from(count);
            self.arc.push(Point::new(
                point.x + self.radius * angle.cos(),
                point.y + self.radius * angle.sin(),
            ));
        }
        self.arc.push(point + b);
        polygon(self.out, &self.arc, self.limits)
    }

    fn cap(&mut self, point: Point, direction: Point) -> Result<()> {
        match self.stroke.cap {
            LineCap::Butt => Ok(()),
            LineCap::Round => self.circle(point),
            LineCap::ProjectingSquare => {
                let n = direction.perp() * self.radius;
                let end = point + direction * self.radius;
                polygon(
                    self.out,
                    &[point + n, end + n, end - n, point - n],
                    self.limits,
                )
            }
        }
    }

    fn circle(&mut self, center: Point) -> Result<()> {
        if self.radius == 0.0 {
            return Ok(());
        }
        let count = self.arc_segments(TAU, 8.0)?;
        self.arc.clear();
        self.arc.extend((0..count).map(|i| {
            let angle = TAU * f64::from(i) / f64::from(count);
            Point::new(
                center.x + self.radius * angle.cos(),
                center.y + self.radius * angle.sin(),
            )
        }));
        polygon(self.out, &self.arc, self.limits)
    }

    /// The number of chords, at least `minimum`, that approximate an arc of
    /// `sweep` radians within the tolerance.
    fn arc_segments(&self, sweep: f64, minimum: f64) -> Result<u32> {
        let step = 2.0 * (1.0 - (self.tolerance / self.radius).min(1.0)).acos();
        let count = (sweep.abs() / step).ceil().max(minimum);
        if !count.is_finite()
            || count > self.limits.path_segments as f64
            || count > f64::from(u32::MAX)
        {
            return Err(Error::limit("round stroke subdivision budget exceeded"));
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a positive integer count bounded by u32::MAX above"
        )]
        Ok(count as u32)
    }
}

/// Appends a positively oriented polygon (module comment).
fn polygon(out: &mut PathBuilder, points: &[Point], limits: &Limits) -> Result<()> {
    if out
        .len()
        .checked_add(points.len() + 1)
        .is_none_or(|n| n > limits.path_segments)
    {
        return Err(Error::limit("stroke outline budget exceeded"));
    }
    let Some(&first) = points.first() else {
        return Ok(());
    };
    for &point in points {
        bounded(point)?;
    }
    let area: f64 = points
        .iter()
        .zip(points.iter().skip(1).chain(std::iter::once(&first)))
        .map(|(&a, &b)| (a - first).cross(b - first))
        .sum();
    if area < 0.0 {
        out.polygon(points.iter().rev().copied());
    } else {
        out.polygon(points.iter().copied());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_55_run_sequences() {
        // Explicit on intervals derived from Table 55, independent of dash().
        type Example<'a> = (&'a [f64], f64, f64, &'a [(f64, f64)]);
        let examples: &[Example<'_>] = &[
            (&[], 0.0, 12.0, &[(0.0, 12.0)]),
            (&[3.0], 0.0, 12.0, &[(0.0, 3.0), (6.0, 9.0)]),
            (&[2.0], 1.0, 9.0, &[(0.0, 1.0), (3.0, 5.0), (7.0, 9.0)]),
            (&[2.0, 1.0], 0.0, 8.0, &[(0.0, 2.0), (3.0, 5.0), (6.0, 8.0)]),
            (&[3.0, 5.0], 6.0, 18.0, &[(2.0, 5.0), (10.0, 13.0)]),
            (
                &[2.0, 3.0],
                11.0,
                16.0,
                &[(0.0, 1.0), (4.0, 6.0), (9.0, 11.0), (14.0, 16.0)],
            ),
            (
                &[2.0, 1.0, 3.0],
                0.0,
                14.0,
                &[(0.0, 2.0), (3.0, 6.0), (8.0, 9.0), (12.0, 14.0)],
            ),
            (
                &[2.0, 1.0, 3.0],
                -2.0,
                14.0,
                &[(2.0, 4.0), (5.0, 8.0), (10.0, 11.0)],
            ),
        ];
        for &(array, phase, length, expected) in examples {
            let stroke = Stroke {
                dash_array: array.to_vec(),
                dash_phase: phase,
                ..Stroke::default()
            };
            let runs = dash(
                vec![Point::new(0.0, 0.0), Point::new(length, 0.0)],
                false,
                &stroke,
                &Limits::default(),
            )
            .unwrap();
            let actual: Vec<_> = runs
                .iter()
                .map(|run| (run.points.first().unwrap().x, run.points.last().unwrap().x))
                .collect();
            assert_eq!(actual, expected, "{array:?} {phase}");
        }
    }
}
