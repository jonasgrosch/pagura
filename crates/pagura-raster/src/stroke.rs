//! Stroke outlines in user space (ISO 32000-2, 8.4.3 and 8.5.3).
//!
//! Positively oriented component polygons form a union under nonzero fill.
//! Flattening is measured after the CTM; constructing the pen before the CTM
//! preserves elliptical pens under nonuniform transforms.

use crate::flatten::{MAX_SEGMENTS, TOLERANCE, bounded, flatten};
use crate::{LineCap, LineJoin, Path, PathBuilder, Point, Stroke, Transform};
use pagura_core::{Error, Result};

pub(crate) fn outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
) -> Result<(Path, Transform)> {
    validate(stroke)?;
    if path.segments().len() > MAX_SEGMENTS {
        return Err(Error::limit("path segment budget exceeded"));
    }
    if !transform.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    let (path, transform, width) = if stroke.width == 0.0 {
        (path.transformed(transform), Transform::IDENTITY, 1.0)
    } else {
        (path.clone(), transform, stroke.width)
    };
    let contours = flatten(&path, transform)?;
    let mut builder = PathBuilder::new();
    let radius = width * 0.5;
    let stretch = transform.a.hypot(transform.b) + transform.c.hypot(transform.d);
    let tolerance = if stretch == 0.0 {
        TOLERANCE
    } else {
        TOLERANCE / stretch
    };
    for contour in contours {
        if contour.points.len() < 2 {
            continue;
        }
        let runs = dash(contour.points, contour.closed, stroke)?;
        for (points, closed) in runs {
            draw(&mut builder, &points, closed, radius, tolerance, stroke)?;
        }
    }
    Ok((builder.finish(), transform))
}

fn validate(stroke: &Stroke) -> Result<()> {
    if !stroke.width.is_finite()
        || stroke.width < 0.0
        || !stroke.miter_limit.is_finite()
        || stroke.miter_limit < 1.0
        || !stroke.dash_phase.is_finite()
        || stroke.dash_phase < 0.0
        || stroke.dash_array.iter().any(|&n| !n.is_finite() || n < 0.0)
    {
        return Err(Error::invalid_argument("invalid stroke parameters"));
    }
    if stroke.dash_array.len() > MAX_SEGMENTS {
        return Err(Error::limit("dash array budget exceeded"));
    }
    if !stroke.dash_array.is_empty() && stroke.dash_array.iter().all(|&n| n == 0.0) {
        return Err(Error::invalid_argument("dash array is all zero"));
    }
    Ok(())
}

fn dash(mut points: Vec<Point>, closed: bool, stroke: &Stroke) -> Result<Vec<(Vec<Point>, bool)>> {
    if stroke.dash_array.is_empty() {
        return Ok(vec![(points, closed)]);
    }
    if closed && let Some(&first) = points.first() {
        points.push(first);
    }
    let cycle_count = stroke
        .dash_array
        .len()
        .checked_mul(if stroke.dash_array.len().is_multiple_of(2) {
            1
        } else {
            2
        })
        .ok_or(Error::limit("dash cycle size overflow"))?;
    let length = stroke.dash_array.iter().sum::<f64>()
        * if cycle_count == stroke.dash_array.len() {
            1.0
        } else {
            2.0
        };
    if !length.is_finite() {
        return Err(Error::limit("dash cycle exceeds numeric range"));
    }
    let mut phase = stroke.dash_phase % length;
    let mut index = 0;
    while phase > 0.0 && phase >= dash_length(stroke, index) {
        phase -= dash_length(stroke, index);
        index = (index + 1) % cycle_count;
    }
    let initial_on = index % 2 == 0;
    let mut remaining = dash_length(stroke, index) - phase;
    let mut result = Vec::new();
    let mut run = Vec::new();
    let mut work = 0;
    for pair in points.windows(2) {
        let [a, b] = [pair[0], pair[1]];
        let distance = (b.x - a.x).hypot(b.y - a.y);
        if distance == 0.0 {
            if index % 2 == 0 {
                run.extend([a, b]);
            }
            continue;
        }
        let mut offset = 0.0;
        while offset < distance || remaining == 0.0 {
            crate::coverage::charge(&mut work, 1)?;
            if result.len() + run.len() > MAX_SEGMENTS {
                return Err(Error::limit("dashed segment budget exceeded"));
            }
            let step = remaining.min(distance - offset);
            let end = offset + step;
            if step > 0.0 && end == offset {
                return Err(Error::limit("dash lengths below numeric resolution"));
            }
            let interpolate = |d: f64| {
                Point::new(
                    a.x + (b.x - a.x) * (d / distance),
                    a.y + (b.y - a.y) * (d / distance),
                )
            };
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
                    result.push((std::mem::take(&mut run), false));
                }
                index = (index + 1) % cycle_count;
                remaining = dash_length(stroke, index);
            }
        }
    }
    if !run.is_empty() {
        result.push((run, false));
    }
    // Join the runs that meet at the closing vertex. A dash crossing the
    // seam has a join there, not two caps. An uninterrupted contour is closed.
    let seam = points.first();
    let starts_at_seam = result.first().and_then(|(run, _)| run.first()) == seam;
    let ends_at_seam = result.last().and_then(|(run, _)| run.last()) == seam;
    if closed && initial_on && starts_at_seam && ends_at_seam && !result.is_empty() {
        if result.len() == 1 {
            if let Some((run, closed)) = result.first_mut() {
                *closed = true;
                run.pop();
            }
        } else if let Some((mut tail, _)) = result.pop()
            && let Some((head, _)) = result.first_mut()
        {
            tail.extend(head.iter().skip(1).copied());
            *head = tail;
        }
    }
    Ok(result)
}

fn dash_length(stroke: &Stroke, index: usize) -> f64 {
    stroke
        .dash_array
        .get(index % stroke.dash_array.len())
        .copied()
        .unwrap_or(0.0)
}

fn draw(
    out: &mut PathBuilder,
    input: &[Point],
    closed: bool,
    radius: f64,
    tolerance: f64,
    stroke: &Stroke,
) -> Result<()> {
    let mut points = Vec::new();
    for &point in input {
        if points.last() != Some(&point) {
            points.push(point);
        }
    }
    if closed && points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() < 2 {
        if !closed && let Some(&point) = points.first() {
            cap(
                out,
                point,
                Point::new(1.0, 0.0),
                radius,
                tolerance,
                stroke.cap,
            )?;
            cap(
                out,
                point,
                Point::new(-1.0, 0.0),
                radius,
                tolerance,
                stroke.cap,
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
        let length = (b.x - a.x).hypot(b.y - a.y);
        let d = Point::new((b.x - a.x) / length, (b.y - a.y) / length);
        let n = Point::new(-d.y * radius, d.x * radius);
        polygon(out, &[add(a, n), add(b, n), sub(b, n), sub(a, n)])?;
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
            (Some(&a), Some(&b)) => join(out, point, a, b, radius, tolerance, stroke)?,
            (None, Some(&d)) => cap(
                out,
                first,
                Point::new(-d.x, -d.y),
                radius,
                tolerance,
                stroke.cap,
            )?,
            (Some(&d), None) => cap(out, last, d, radius, tolerance, stroke.cap)?,
            _ => {}
        }
    }
    Ok(())
}

fn join(
    out: &mut PathBuilder,
    point: Point,
    a: Point,
    b: Point,
    radius: f64,
    tolerance: f64,
    stroke: &Stroke,
) -> Result<()> {
    let cross = a.x * b.y - a.y * b.x;
    if cross == 0.0 && (stroke.join != LineJoin::Round || a.x * b.x + a.y * b.y >= 0.0) {
        return Ok(());
    }
    let side = if cross > 0.0 { -radius } else { radius };
    let n1 = Point::new(-a.y * side, a.x * side);
    let n2 = Point::new(-b.y * side, b.x * side);
    if stroke.join == LineJoin::Round {
        return round_join(out, point, n1, n2, radius, tolerance, cross);
    }
    let p1 = add(point, n1);
    let p2 = add(point, n2);
    let dot = a.x * b.x + a.y * b.y;
    let denominator = 1.0 + dot;
    if stroke.join == LineJoin::Miter && denominator > 0.0 {
        let m = Point::new((n1.x + n2.x) / denominator, (n1.y + n2.y) / denominator);
        if m.x.hypot(m.y) <= radius * stroke.miter_limit {
            return polygon(out, &[point, p1, add(point, m), p2]);
        }
    }
    polygon(out, &[point, p1, p2])
}

fn round_join(
    out: &mut PathBuilder,
    point: Point,
    a: Point,
    b: Point,
    radius: f64,
    tolerance: f64,
    cross: f64,
) -> Result<()> {
    let start = a.y.atan2(a.x);
    let end = b.y.atan2(b.x);
    let angle = if cross > 0.0 {
        (end - start).rem_euclid(std::f64::consts::TAU)
    } else {
        -(start - end).rem_euclid(std::f64::consts::TAU)
    };
    let step = 2.0 * (1.0 - (tolerance / radius).min(1.0)).acos();
    let count = (angle.abs() / step).ceil().max(1.0);
    if !count.is_finite() || count > 4096.0 {
        return Err(Error::limit("round join subdivision budget exceeded"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "integer count bounded to 1..=4096 above"
    )]
    let count = count as u32;
    let mut points = vec![point, add(point, a)];
    for i in 1..count {
        let angle = start + angle * f64::from(i) / f64::from(count);
        points.push(Point::new(
            point.x + radius * angle.cos(),
            point.y + radius * angle.sin(),
        ));
    }
    points.push(add(point, b));
    polygon(out, &points)
}

fn cap(
    out: &mut PathBuilder,
    point: Point,
    direction: Point,
    radius: f64,
    tolerance: f64,
    cap: LineCap,
) -> Result<()> {
    match cap {
        LineCap::Butt => Ok(()),
        LineCap::Round => circle(out, point, radius, tolerance),
        LineCap::ProjectingSquare => {
            let n = Point::new(-direction.y * radius, direction.x * radius);
            let end = add(
                point,
                Point::new(direction.x * radius, direction.y * radius),
            );
            polygon(
                out,
                &[add(point, n), add(end, n), sub(end, n), sub(point, n)],
            )
        }
    }
}

fn circle(out: &mut PathBuilder, center: Point, radius: f64, tolerance: f64) -> Result<()> {
    if radius == 0.0 {
        return Ok(());
    }
    let angle = 2.0 * (1.0 - (tolerance / radius).min(1.0)).acos();
    let count = (std::f64::consts::TAU / angle).ceil().max(8.0);
    if !count.is_finite() || count > 4096.0 {
        return Err(Error::limit("round stroke subdivision budget exceeded"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "integer count is bounded to 8..=4096 above"
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
    polygon(out, &points)
}

fn polygon(out: &mut PathBuilder, points: &[Point]) -> Result<()> {
    if out
        .len()
        .checked_add(points.len() + 1)
        .is_none_or(|n| n > MAX_SEGMENTS)
    {
        return Err(Error::limit("stroke outline budget exceeded"));
    }
    let Some(&first) = points.first() else {
        return Ok(());
    };
    let area: f64 = points
        .iter()
        .zip(points.iter().skip(1).chain(std::iter::once(&first)))
        .map(|(a, b)| (a.x - first.x) * (b.y - first.y) - (a.y - first.y) * (b.x - first.x))
        .sum();
    let mut ordered: Vec<_> = points.to_vec();
    if area < 0.0 {
        ordered.reverse();
    }
    for (i, point) in ordered.into_iter().enumerate() {
        bounded(point)?;
        if i == 0 {
            out.move_to(point);
        } else {
            out.line_to(point);
        }
    }
    out.close();
    Ok(())
}

fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}
fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}
