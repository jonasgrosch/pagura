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

pub(super) type Facet = [(f64, f64); 6];

impl Kind {
    pub(super) fn shades(self, color: &Color) -> [Color; 2] {
        let [light, dark] = color.relief_colors();
        match self {
            Self::Raised => [light, dark],
            Self::Sunken => [dark, light],
        }
    }
}

pub(super) fn facets(bounds: Rect, width: f64) -> Result<Option<[Facet; 2]>> {
    if !bounds.width().is_finite() || !bounds.height().is_finite() || !width.is_finite() {
        return Err(Error::limit("relief border extent exceeds numeric range"));
    }
    let width = width.min(bounds.width() / 2.0).min(bounds.height() / 2.0);
    if width == 0.0 {
        return Ok(None);
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
    Ok(Some([top_left, bottom_right]))
}

pub(super) fn paint(
    marks: &mut Marks<'_>,
    bounds: Rect,
    width: f64,
    color: &Color,
    kind: Kind,
) -> Result<()> {
    if let Some(facets) = facets(bounds, width)? {
        for (points, shade) in facets.iter().zip(kind.shades(color)) {
            marks.fill_border(&[Subpath::Polygon(points)], &shade)?;
        }
    }
    Ok(())
}
