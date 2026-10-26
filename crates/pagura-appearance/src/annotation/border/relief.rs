//! Simulated raised/recessed rectangular borders (12.5.4, Table 168).
//!
//! Reader policy splits the inside frame into two six-vertex facets. Raised
//! borders light the top/left, recessed borders light the bottom/right. Device
//! components interpolate halfway toward white/black; no lighting model is run.

use crate::annotation::marks::{Color, Marks, Subpath};
use pagura_core::{Error, Rect, Result};

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Raised,
    Sunken,
}

pub(super) fn paint(
    marks: &mut Marks<'_>,
    bounds: Rect,
    width: f64,
    color: &Color,
    kind: Kind,
) -> Result<()> {
    if !bounds.width().is_finite() || !bounds.height().is_finite() || !width.is_finite() {
        return Err(Error::limit("relief border extent exceeds numeric range"));
    }
    let width = width.min(bounds.width() / 2.0).min(bounds.height() / 2.0);
    if width == 0.0 {
        return Ok(());
    }
    let (x0, y0, x1, y1) = (bounds.x0(), bounds.y0(), bounds.x1(), bounds.y1());
    let (left, bottom, right, top) = (x0 + width, y0 + width, x1 - width, y1 - width);
    let top_left = [
        (x0, y0),
        (x0, y1),
        (x1, y1),
        (right, top),
        (left, top),
        (left, bottom),
    ];
    let bottom_right = [
        (x0, y0),
        (x1, y0),
        (x1, y1),
        (right, top),
        (right, bottom),
        (left, bottom),
    ];
    let [light, dark] = color.relief_colors();
    let (first, second) = match kind {
        Kind::Raised => (&light, &dark),
        Kind::Sunken => (&dark, &light),
    };
    marks.fill_border(&[Subpath::Polygon(&top_left)], first)?;
    marks.fill_border(&[Subpath::Polygon(&bottom_right)], second)
}
