//! Device grid fitting for strokes (ISO 32000-2, 10.7.5).
//!
//! Device choice: pen diameters round to whole pixels, at least one. Centerlines
//! snap to the corresponding half-width grid; open butt endpoints snap along
//! the tangent to whole pixels. Dashes are split in original user space first.
//! The fitted elliptical pen keeps the original nonuniform scale ratio to the
//! nearest device-pixel widths. General geometry retains its centreline and fits
//! the principal diameters of the device pen. This device policy bounds the
//! diameter change in every direction by half a pixel (except the required
//! minimum one-pixel pen), without introducing corners into flattened curves.

use crate::flatten::{MAX_COORDINATE, TOLERANCE, flatten};
use crate::stroke::Pen;
use crate::{LineCap, Path, PathBuilder, Point, Stroke, Transform};
use pagura_core::{Error, Limits, Result};

pub(crate) fn outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    crate::stroke::validate(stroke, limits)?;
    let contours = flatten(path, transform, limits)?;
    if !contours.iter().any(|c| c.strokeable(stroke.cap)) {
        return Ok((PathBuilder::new().finish(), Transform::IDENTITY));
    }
    let axis =
        (transform.b == 0.0 && transform.c == 0.0) || (transform.a == 0.0 && transform.d == 0.0);
    let orthogonal = contours.iter().all(|c| {
        c.points
            .windows(2)
            .all(|v| v[0].x == v[1].x || v[0].y == v[1].y)
            && (!c.closed
                || c.points
                    .first()
                    .zip(c.points.last())
                    .is_none_or(|(a, b)| a.x == b.x || a.y == b.y))
    });
    if !axis || !orthogonal {
        return general(contours, transform, stroke, limits);
    }
    let width = |scale: f64| {
        if stroke.width == 0.0 {
            1.0
        } else {
            (stroke.width * scale).round().max(1.0)
        }
    };
    let wx = width(transform.a.hypot(transform.c));
    let wy = width(transform.b.hypot(transform.d));
    if !wx.is_finite() || !wy.is_finite() || wx.max(wy) > MAX_COORDINATE {
        return Err(Error::limit("adjusted pen exceeds numeric working range"));
    }
    let pen = Transform::scale(wx, wy);
    let mut out = PathBuilder::new();
    let mut drawing = Pen::new(&mut out, 0.5, TOLERANCE / (wx + wy), stroke, limits);
    for contour in contours {
        if !contour.strokeable(stroke.cap) {
            continue;
        }
        for run in crate::stroke::dash(contour.points, contour.closed, stroke, limits)? {
            let mut run = run.transformed(transform);
            let device = &run.points;
            let mut fitted: Vec<_> = device
                .iter()
                .map(|p| Point::new(snap(p.x, wx), snap(p.y, wy)))
                .collect();
            if !run.closed && stroke.cap == LineCap::Butt {
                butt_endpoints(device, &mut fitted);
            }
            run.points = fitted;
            drawing.draw(&run.transformed(Transform::scale(1.0 / wx, 1.0 / wy)))?;
        }
    }
    Ok((out.finish(), pen))
}
fn snap(value: f64, width: f64) -> f64 {
    (value - width * 0.5).round() + width * 0.5
}
fn butt_endpoints(original: &[Point], fitted: &mut [Point]) {
    if let (Some(a), Some(b), Some(target)) =
        (original.first(), original.get(1), fitted.first_mut())
    {
        if a.y == b.y {
            target.x = a.x.round();
        } else {
            target.y = a.y.round();
        }
    }
    if let (Some(a), Some(b), Some(target)) = (
        original.last(),
        original.iter().rev().nth(1),
        fitted.last_mut(),
    ) {
        if a.y == b.y {
            target.x = a.x.round();
        } else {
            target.y = a.y.round();
        }
    }
}
/// Fit an affine pen by its principal device diameters. For any unit normal,
/// the support-function difference is bounded by the largest singular-value
/// change. Only the pen changes; dashes still measure the original user path.
fn general(
    contours: Vec<crate::flatten::Polyline>,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    let pen = if stroke.width == 0.0 {
        Transform::IDENTITY
    } else {
        fitted_pen(transform, stroke.width)?
    };
    let inverse = pen
        .inverse()
        .ok_or(Error::limit("adjusted pen inverse overflow"))?;
    let normalize = transform.then(inverse);
    let scale = pen.a.hypot(pen.b).max(pen.c.hypot(pen.d));
    let mut out = PathBuilder::new();
    let mut drawing = Pen::new(&mut out, 0.5, TOLERANCE / (2.0 * scale), stroke, limits);
    for contour in contours {
        if !contour.strokeable(stroke.cap) {
            continue;
        }
        for run in crate::stroke::dash(contour.points, contour.closed, stroke, limits)? {
            drawing.draw(&run.transformed(normalize))?;
        }
    }
    Ok((out.finish(), pen))
}

fn fitted_pen(t: Transform, width: f64) -> Result<Transform> {
    let scale = t.a.abs().max(t.b.abs()).max(t.c.abs()).max(t.d.abs());
    if scale == 0.0 || !scale.is_finite() || !t.is_finite() {
        return Err(Error::limit("adjusted pen exceeds numeric working range"));
    }
    let [a, b, c, d] = [t.a, t.b, t.c, t.d].map(|v| v / scale);
    let xx = a * a + c * c;
    let yy = b * b + d * d;
    let xy = a * b + c * d;
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    let (sin, cos) = angle.sin_cos();
    let major = ((xx + yy + (xx - yy).hypot(2.0 * xy)) * 0.5).sqrt();
    // The determinant avoids cancellation for a narrow ellipse.
    let minor = (a * d - b * c).abs() / major;
    let [major, minor] = [major, minor].map(|v| (v * scale * width).round().max(1.0));
    if !major.is_finite() || major > MAX_COORDINATE || !minor.is_finite() {
        return Err(Error::limit("adjusted pen exceeds numeric working range"));
    }
    Ok(Transform::new(
        cos * major,
        sin * major,
        -sin * minor,
        cos * minor,
        0.0,
        0.0,
    ))
}
