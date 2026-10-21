//! Stroke outlines in user space (ISO 32000-2, 8.4.3 and 8.5.3).
//!
//! Positively oriented component polygons form a union under nonzero fill.
//! Flattening is measured after the CTM; constructing the pen before the CTM
//! preserves elliptical pens under nonuniform transforms.

use crate::coverage::RASTER_WORK;
use crate::flatten::{TOLERANCE, bounded, flatten};
use crate::{LineCap, LineJoin, Path, PathBuilder, Point, Stroke, Transform};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

mod convex;
mod tiling;

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
    outline_with(path, transform, stroke, limits, false).map(|(path, transform, _)| (path, transform))
}

/// [`outline`] whose boundary, when every run admits one, is made of pieces
/// that meet without overlapping ([`tiling`]): signed-area coverage of it
/// needs no union integration.
pub(crate) fn tiled_outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    outline_with(path, transform, stroke, limits, true).map(|(path, transform, _)| (path, transform))
}

/// [`tiled_outline`], and whether every run admitted pieces: then no two
/// pieces overlap, and the outline is its own boundary.
pub(crate) fn pieces(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform, bool)> {
    outline_with(path, transform, stroke, limits, true)
}

fn outline_with(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
    tiled: bool,
) -> Result<(Path, Transform, bool)> {
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
    let radius = width * 0.5;
    let stretch = pen_transform.a.hypot(pen_transform.b) + pen_transform.c.hypot(pen_transform.d);
    let tolerance = if stretch == 0.0 {
        TOLERANCE
    } else {
        TOLERANCE / stretch
    };
    // Boundaries replace components only for undashed miter strokes whose pen
    // maps circles to circles.
    let boundaries_allowed = stroke.dash_array.is_empty()
        && stroke.join == LineJoin::Miter
        && ((transform.a == transform.d && transform.b == -transform.c)
            || (transform.a == -transform.d && transform.b == transform.c));
    let mut runs = Vec::new();
    let mut cleanup_work = 0;
    for contour in contours {
        if !contour.strokeable(stroke.cap) {
            continue;
        }
        for run in dash(contour.points, contour.closed, stroke, limits)? {
            let mut run = if hairline {
                run.transformed(transform)
            } else {
                run
            };
            let boundary = if boundaries_allowed
                && run.closed
                && run.points.len() >= convex::MIN_VERTICES
            {
                // Closed curves may explicitly repeat their starting vertex.
                // Use the same point cleanup as the component stroker before
                // proving strict convexity; duplicate endpoints are not a
                // geometric turn.
                crate::coverage::charge(&mut cleanup_work, run.points.len(), limits.raster_work)?;
                run.points.dedup();
                if run.points.first() == run.points.last() {
                    run.points.pop();
                }
                convex::boundaries(&run.points, radius, tolerance, stroke.miter_limit, limits)?
            } else {
                None
            };
            runs.push((run, boundary));
        }
    }
    // Pieces replace the components when every run admits them; they cover
    // the same set, so they serve as the outline and as its boundary.
    if tiled {
        let mut tiles = PathBuilder::new();
        let mut complete = tiling::apart(runs.iter().map(|(run, _)| run), radius, stroke)?;
        for (run, boundary) in &runs {
            if !complete {
                break;
            }
            complete = match boundary {
                Some(contours) => {
                    convex::append(&mut tiles, contours.clone(), limits)?;
                    true
                }
                None => tiling::append(&mut tiles, run, radius, tolerance, stroke, limits)?,
            };
        }
        if complete {
            let mut outline = tiles.finish();
            outline.boundary = Some(Arc::new(outline.clone()));
            return Ok((outline, pen_transform, true));
        }
    }
    let mut components = PathBuilder::new();
    // Begins at the first proven boundary as a copy of the components so far;
    // each later run adds its boundary or its components.
    let mut boundaries: Option<PathBuilder> = None;
    for (run, boundary) in runs {
        if let Some(contours) = boundary {
            let out = boundaries.get_or_insert_with(|| components.clone());
            convex::append(out, contours, limits)?;
        } else if let Some(out) = &mut boundaries {
            draw(out, &run, radius, tolerance, stroke, limits)?;
        }
        draw(&mut components, &run, radius, tolerance, stroke, limits)?;
    }
    let mut outline = components.finish();
    outline.boundary = boundaries.map(|mut boundaries| Arc::new(boundaries.finish()));
    Ok((outline, pen_transform, false))
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

pub(crate) fn draw(
    out: &mut PathBuilder,
    run: &Run,
    radius: f64,
    tolerance: f64,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<()> {
    let closed = run.closed;
    let points = tiling::centreline(run);
    if points.len() < 2 {
        if let Some(&point) = points.first() {
            cap(
                out,
                point,
                run.tangent,
                radius,
                tolerance,
                stroke.cap,
                limits,
            )?;
            cap(
                out,
                point,
                -run.tangent,
                radius,
                tolerance,
                stroke.cap,
                limits,
            )?;
        }
        return Ok(());
    }
    let first = points
        .first()
        .copied()
        .ok_or(Error::invalid_argument("empty stroke"))?;
    let last = points
        .last()
        .copied()
        .ok_or(Error::invalid_argument("empty stroke"))?;
    let mut directions = Vec::new();
    for (&a, &b) in points
        .iter()
        .zip(points.iter().skip(1).chain(closed.then_some(&first)))
    {
        let d = (b - a) / (b - a).length();
        let n = d.perp() * radius;
        polygon(out, &[a + n, b + n, b - n, a - n], limits)?;
        directions.push(d);
    }
    for (i, &point) in points.iter().enumerate() {
        let before = if i == 0 {
            if closed { directions.last() } else { None }
        } else {
            directions.get(i - 1)
        };
        let after = directions.get(i);
        match (before, after) {
            (Some(&a), Some(&b)) => join(out, point, [a, b], radius, tolerance, stroke, limits)?,
            (None, Some(&d)) => cap(out, first, -d, radius, tolerance, stroke.cap, limits)?,
            (Some(&d), None) => cap(out, last, d, radius, tolerance, stroke.cap, limits)?,
            _ => {}
        }
    }
    Ok(())
}

fn join(
    out: &mut PathBuilder,
    point: Point,
    tangents: [Point; 2],
    radius: f64,
    tolerance: f64,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<()> {
    let [a, b] = tangents;
    let cross = a.cross(b);
    if cross == 0.0 && (stroke.join != LineJoin::Round || a.dot(b) >= 0.0) {
        return Ok(());
    }
    let side = if cross > 0.0 { -radius } else { radius };
    let n1 = a.perp() * side;
    let n2 = b.perp() * side;
    if stroke.join == LineJoin::Round {
        return round_join(out, point, [n1, n2], radius, tolerance, cross, limits);
    }
    let p1 = point + n1;
    let p2 = point + n2;
    let denominator = 1.0 + a.dot(b);
    if stroke.join == LineJoin::Miter && denominator > 0.0 {
        let m = (n1 + n2) / denominator;
        if m.length() <= radius * stroke.miter_limit {
            return polygon(out, &[point, p1, point + m, p2], limits);
        }
    }
    polygon(out, &[point, p1, p2], limits)
}

fn round_join(
    out: &mut PathBuilder,
    point: Point,
    normals: [Point; 2],
    radius: f64,
    tolerance: f64,
    cross: f64,
    limits: &Limits,
) -> Result<()> {
    let [a, b] = normals;
    let start = a.y.atan2(a.x);
    let end = b.y.atan2(b.x);
    let angle = if cross > 0.0 {
        (end - start).rem_euclid(std::f64::consts::TAU)
    } else {
        -(start - end).rem_euclid(std::f64::consts::TAU)
    };
    let step = 2.0 * (1.0 - (tolerance / radius).min(1.0)).acos();
    let count = (angle.abs() / step).ceil().max(1.0);
    if !count.is_finite() || count > limits.path_segments as f64 || count > f64::from(u32::MAX) {
        return Err(Error::limit("round join subdivision budget exceeded"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "integer count bounded by the u32-compatible path budget below"
    )]
    let count = count as u32;
    let mut points = vec![point, point + a];
    for i in 1..count {
        let angle = start + angle * f64::from(i) / f64::from(count);
        points.push(Point::new(
            point.x + radius * angle.cos(),
            point.y + radius * angle.sin(),
        ));
    }
    points.push(point + b);
    polygon(out, &points, limits)
}

fn cap(
    out: &mut PathBuilder,
    point: Point,
    direction: Point,
    radius: f64,
    tolerance: f64,
    cap: LineCap,
    limits: &Limits,
) -> Result<()> {
    match cap {
        LineCap::Butt => Ok(()),
        LineCap::Round => circle(out, point, radius, tolerance, limits),
        LineCap::ProjectingSquare => {
            let n = direction.perp() * radius;
            let end = point + direction * radius;
            polygon(out, &[point + n, end + n, end - n, point - n], limits)
        }
    }
}

fn circle(
    out: &mut PathBuilder,
    center: Point,
    radius: f64,
    tolerance: f64,
    limits: &Limits,
) -> Result<()> {
    if radius == 0.0 {
        return Ok(());
    }
    let angle = 2.0 * (1.0 - (tolerance / radius).min(1.0)).acos();
    let count = (std::f64::consts::TAU / angle).ceil().max(8.0);
    if !count.is_finite() || count > limits.path_segments as f64 || count > f64::from(u32::MAX) {
        return Err(Error::limit("round stroke subdivision budget exceeded"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "integer count is bounded by the u32-compatible path budget below"
    )]
    let count = count as u32;
    let points: Vec<_> = (0..count)
        .map(|i| {
            let angle = std::f64::consts::TAU * f64::from(i) / f64::from(count);
            Point::new(
                center.x + radius * angle.cos(),
                center.y + radius * angle.sin(),
            )
        })
        .collect();
    polygon(out, &points, limits)
}

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
    // Every component polygon is positively oriented (module comment).
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
