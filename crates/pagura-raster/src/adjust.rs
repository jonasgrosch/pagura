//! Device grid fitting for orthogonal strokes (ISO 32000-2, 10.7.5).
//!
//! Device choice: pen diameters round to whole pixels, at least one. Centerlines
//! snap to the corresponding half-width grid; open butt endpoints snap along
//! the tangent to whole pixels. Dashes are split in original user space first.
//! The fitted elliptical pen keeps the original nonuniform scale ratio to the
//! nearest device-pixel widths. Curved/oblique paths need a separate fitter and
//! remain explicitly unsupported rather than being quantized indiscriminately.

use crate::flatten::{TOLERANCE, flatten};
use crate::{LineCap, Path, PathBuilder, Point, Stroke, Transform};
use pagura_core::{Error, Limits, Result};

pub(crate) fn outline(
    path: &Path,
    transform: Transform,
    stroke: &Stroke,
    limits: &Limits,
) -> Result<(Path, Transform)> {
    crate::stroke::validate(stroke, limits)?;
    let axis =
        (transform.b == 0.0 && transform.c == 0.0) || (transform.a == 0.0 && transform.d == 0.0);
    if !axis || !transform.is_finite() {
        return Err(unsupported());
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
    if !wx.is_finite() || !wy.is_finite() || wx.max(wy) > 1e12 {
        return Err(Error::limit("adjusted pen exceeds numeric working range"));
    }
    let pen = Transform::scale(wx, wy);
    let mut out = PathBuilder::new();
    for contour in flatten(path, transform, limits)? {
        if contour
            .points
            .windows(2)
            .any(|v| v[0].x != v[1].x && v[0].y != v[1].y)
        {
            return Err(unsupported());
        }
        if contour.closed
            && let (Some(a), Some(b)) = (contour.points.first(), contour.points.last())
            && a.x != b.x
            && a.y != b.y
        {
            return Err(unsupported());
        }
        for (run, closed) in crate::stroke::dash(contour.points, contour.closed, stroke, limits)? {
            let device: Vec<_> = run.into_iter().map(|p| p * transform).collect();
            let mut fitted: Vec<_> = device
                .iter()
                .map(|p| Point::new(snap(p.x, wx), snap(p.y, wy)))
                .collect();
            if !closed && stroke.cap == LineCap::Butt {
                butt_endpoints(&device, &mut fitted);
            }
            for point in &mut fitted {
                point.x /= wx;
                point.y /= wy;
            }
            crate::stroke::draw(
                &mut out,
                &fitted,
                closed,
                0.5,
                TOLERANCE / (wx + wy),
                stroke,
                limits,
            )?;
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
fn unsupported() -> Error {
    Error::unsupported(
        crate::features::STROKE_ADJUSTMENT,
        "stroke adjustment for oblique or curved geometry is not implemented",
    )
}
