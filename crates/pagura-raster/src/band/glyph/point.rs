//! Certified nonpoint geometry with the canonical point row resolved once.

use super::{GlyphEdges, Kind, PolygonLines, Relative, curve, spline_certified};
use crate::band::shape::{Frame, Shape};
use crate::band::{Prep, RASTER_WORK};
use crate::flatten::bounded;
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Limits, Meter, Result};
use std::sync::Arc;

pub(super) fn relative(
    outline: &Path,
    linear: Transform,
    limits: &Limits,
    vertices: usize,
    parent: &mut Meter,
) -> Result<Option<Box<Relative>>> {
    let Some(headroom) = vertices.checked_mul(size_of::<Point>()) else {
        return Ok(None);
    };
    let Some(segments) = vertices.checked_add(4) else {
        return Ok(None);
    };
    let Some(source_bytes) = outline
        .segments()
        .len()
        .checked_mul(size_of::<Segment>())
        .and_then(|n| n.checked_add(size_of::<Path>() + 2 * size_of::<usize>()))
    else {
        return Ok(None);
    };
    // Canonical construction held every borrowed contour point while it
    // copied these vertices. That released overlap is a source-fixed byte
    // allowance, independent of current limits or Store history.
    parent.charge(headroom)?;
    let mut bytes = Meter::new(headroom, "nonpoint glyph byte headroom exceeded");
    let limits = Limits {
        path_segments: segments.max(outline.segments().len()),
        ..*limits
    };
    let result = (|| {
        let Some(spline) = curve::nonpoint_spline(outline, linear, &limits, &mut bytes)? else {
            return Ok(None);
        };
        if !spline_certified(&spline, &mut bytes).map_err(curve::SplineError::Bytes)? {
            return Ok(None);
        }
        let relative = Relative::new(&spline, &mut bytes).map_err(curve::SplineError::Bytes)?;
        let released = spline.retained_bytes();
        drop(spline);
        bytes.refund(released);
        if relative.lines.is_empty() && relative.curves.is_empty() {
            return Ok(None);
        }
        bytes
            .charge(size_of::<Relative>())
            .map_err(curve::SplineError::Bytes)?;
        Ok(Some(Box::new(relative)))
    })();
    match result {
        // All candidate owners have dropped on these paths. The canonical
        // polygon and its parent meter have not changed.
        Ok(None) | Err(curve::SplineError::Bytes(_) | curve::SplineError::Segments(_)) => {
            parent.refund(headroom);
            Ok(None)
        }
        Err(curve::SplineError::Geometry(error)) => {
            parent.refund(headroom);
            Err(error)
        }
        Ok(Some(relative)) => {
            let retained = bytes.used();
            if retained
                .checked_add(source_bytes)
                .is_some_and(|n| n <= headroom)
            {
                parent.refund(headroom - retained);
                Ok(Some(relative))
            } else {
                drop(relative);
                parent.refund(headroom);
                Ok(None)
            }
        }
    }
}

pub(super) fn placed(
    (edges, relative, point): (&Arc<GlyphEdges>, &Relative, Point),
    origin: Point,
    hull: [Point; 2],
    frame: &Frame<'_>,
    options: &Prep<'_>,
    bytes: &mut Meter,
) -> Result<Option<Shape>> {
    let Kind::Polygon {
        polygon, points, ..
    } = &edges.kind
    else {
        unreachable!("a canonical polygon with nonpoint edges");
    };
    let mut work = Meter::new(options.limits.raster_work, RASTER_WORK);
    work.charge(polygon.vertices().saturating_add(points.len()))?;
    // Preserve the stored polygon's placed-coordinate validation even
    // though its full translated copy is no longer needed.
    for &point in &polygon.points {
        bounded(point + origin)?;
    }
    let point = point + origin;
    bounded(point)?;
    let Some(mut shape) = Shape::translated(edges, relative, origin, hull, frame, options)? else {
        return Ok(None);
    };
    bytes.charge(shape.retained_bytes())?;
    let bounds = shape.bounds();
    let (x, y) = (point.x.floor(), point.y.floor());
    if y >= f64::from(bounds.y0)
        && y < f64::from(bounds.y1)
        && x < f64::from(bounds.x1)
        && x + 1.0 > f64::from(bounds.x0)
    {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the row lies in the u32 device bounds above"
        )]
        let row = y as u32;
        shape.point_row(
            PolygonLines::new(polygon, points, origin),
            row,
            options,
            bytes,
            &mut work,
        )?;
    }
    // Keep only the ready body and its row; there is no placement geometry
    // outside those owners.
    debug_assert_eq!(bytes.used(), shape.retained_bytes());
    Ok(Some(shape))
}

#[cfg(test)]
mod tests;
