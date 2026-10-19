//! Closed-form rectangle coverage and fractional clipping bounds (8.5.3–8.5.4).

use super::{Geometry, Row, Shape, index};
use crate::flatten::bounded;
use crate::{IntRect, Segment, quantize_unit};
use pagura_core::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub(super) struct Rectangle {
    pub(super) left: f64,
    pub(super) top: f64,
    pub(super) right: f64,
    pub(super) bottom: f64,
}

impl Shape {
    pub(super) fn rectangle(&self) -> Option<Rectangle> {
        use Segment::{Close, LineTo, MoveTo};
        let points = match &self.geometry {
            Geometry::Path(path) => {
                let [MoveTo(a), LineTo(b), LineTo(c), LineTo(d), tail @ ..] = path.segments()
                else {
                    return None;
                };
                // Filled open paths close implicitly (8.5.3.3.1).
                if !matches!(tail, [] | [Close])
                    && !matches!(tail, [LineTo(end)] | [LineTo(end), Close] if end == a)
                {
                    return None;
                }
                [*a, *b, *c, *d]
            }
            Geometry::Glyph(glyph) => {
                let [contour] = glyph.contours() else {
                    return None;
                };
                if contour.is_degenerate() {
                    return None;
                }
                let [a, b, c, d, tail @ ..] = contour.points() else {
                    return None;
                };
                if !matches!(tail, [] | [_]) || tail.first().is_some_and(|end| end != a) {
                    return None;
                }
                [*a, *b, *c, *d]
            }
        };
        if !self.transform.is_finite() {
            return None;
        }
        let [a, b, c, d] = points.map(|p| p * self.transform);
        // Ineligible geometry goes through the regular validator, including
        // degenerate paths, whose device-pixel policy differs from empty area.
        if points
            .into_iter()
            .chain([a, b, c, d])
            .any(|p| bounded(p).is_err())
        {
            return None;
        }
        let axis = (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
            || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x);
        (axis && a != b && b != c && c != d && d != a).then_some(Rectangle {
            left: a.x.min(c.x),
            top: a.y.min(c.y),
            right: a.x.max(c.x),
            bottom: a.y.max(c.y),
        })
    }
}

impl Rectangle {
    // Rectangular intersection is geometry (8.5.4), never a multiplication of
    // separately quantized masks. Keep only other shapes in the winding state.
    pub(super) fn intersection(
        shapes: &[Shape],
        area: IntRect,
        row: &mut Row,
    ) -> Result<(Self, usize)> {
        let mut bounds = Self::from(area);
        let mut groups = 0;
        // Paths that are not rectangles are validated when they are flattened.
        for shape in shapes {
            row.charge(1)?;
            if let Some(rectangle) = shape.rectangle() {
                row.charge(shape.segments())?;
                bounds.intersect(rectangle);
            } else {
                groups += 1;
            }
        }
        Ok((bounds, groups))
    }

    pub(super) fn intersect(&mut self, other: Self) {
        self.left = self.left.max(other.left);
        self.top = self.top.max(other.top);
        self.right = self.right.min(other.right);
        self.bottom = self.bottom.min(other.bottom);
    }

    pub(super) fn is_empty(self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }

    pub(super) fn write(self, out: &mut [u8], stride: usize, area: IntRect) -> Result<()> {
        out.fill(0);
        if self.is_empty() {
            return Ok(());
        }
        let left = self.left - f64::from(area.x());
        let right = self.right - f64::from(area.x());
        let start = index(left.floor(), stride);
        let end = index(right.ceil(), stride);
        let column =
            f64::from(u32::try_from(start).map_err(|_| Error::limit("column exceeds u32"))?);
        let first = (right.min(column + 1.0) - left).min(1.0);
        let last = right - (right.ceil() - 1.0).max(left);
        for (y, row) in out.chunks_exact_mut(stride).enumerate() {
            let y = f64::from(u32::try_from(y).map_err(|_| Error::limit("row exceeds u32"))?)
                + f64::from(area.y());
            let height = (self.bottom.min(y + 1.0) - self.top.max(y)).clamp(0.0, 1.0);
            if height > 0.0 {
                let run = &mut row[start..end];
                run.fill(quantize_unit(height));
                if let Some(pixel) = run.first_mut() {
                    *pixel = quantize_unit(first * height);
                }
                if let Some(pixel) = run.last_mut() {
                    *pixel = quantize_unit(last * height);
                }
            }
        }
        Ok(())
    }
}

impl From<IntRect> for Rectangle {
    fn from(area: IntRect) -> Self {
        Self {
            left: f64::from(area.x()),
            top: f64::from(area.y()),
            right: f64::from(area.right()),
            bottom: f64::from(area.bottom()),
        }
    }
}

#[cfg(test)]
mod tests;
