//! Closed-form rectangle coverage and fractional clipping bounds (8.5.3–8.5.4).

use super::{Geometry, Row, Shape, index};
use crate::Segment;
use crate::flatten::bounded;
use crate::quantize_unit;
use crate::raster::spans::Runs;
use pagura_core::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Rectangle {
    pub(crate) left: f64,
    pub(crate) top: f64,
    pub(crate) right: f64,
    pub(crate) bottom: f64,
}

impl Shape {
    /// The four corners of a single closed four-sided contour.
    pub(crate) fn quadrilateral(&self) -> Option<[crate::Point; 4]> {
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
        Some([a, b, c, d])
    }

    pub(crate) fn rectangle(&self) -> Option<Rectangle> {
        let [a, b, c, d] = self.quadrilateral()?;
        let axis = (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
            || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x);
        (axis && a != b && b != c && c != d && d != a).then_some(Rectangle {
            left: a.x.min(c.x),
            top: a.y.min(c.y),
            right: a.x.max(c.x),
            bottom: a.y.max(c.y),
        })
    }

    /// Strict convex half-planes contain the entire geometry hull. Outward-rounded
    /// orientation intervals decline near-boundary cases instead of guessing.
    pub(crate) fn contains_hull(&self, [lo, hi]: [crate::Point; 2]) -> bool {
        let Some(points) = self.quadrilateral() else {
            return false;
        };
        let Some(sign) = orientation(points[0], points[1], points[2]) else {
            return false;
        };
        let corners = [
            lo,
            crate::Point::new(hi.x, lo.y),
            hi,
            crate::Point::new(lo.x, hi.y),
        ];
        (0..4).all(|i| {
            orientation(points[i], points[(i + 1) % 4], points[(i + 2) % 4]) == Some(sign)
                && corners
                    .iter()
                    .all(|&p| orientation(points[i], points[(i + 1) % 4], p) == Some(sign))
        })
    }
}

fn orientation(a: crate::Point, b: crate::Point, c: crate::Point) -> Option<i8> {
    let difference = |x: f64, y: f64| {
        let value = x - y;
        [value.next_down(), value.next_up()]
    };
    let product = |a: [f64; 2], b: [f64; 2]| {
        let values = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
        [
            values.into_iter().fold(f64::INFINITY, f64::min).next_down(),
            values
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)
                .next_up(),
        ]
    };
    let left = product(difference(b.x, a.x), difference(c.y, a.y));
    let right = product(difference(b.y, a.y), difference(c.x, a.x));
    if (left[0] - right[1]).next_down() > 0.0 {
        Some(1)
    } else if (left[1] - right[0]).next_up() < 0.0 {
        Some(-1)
    } else {
        None
    }
}

impl Rectangle {
    // Rectangular intersection is geometry (8.5.4), never a multiplication of
    // separately quantized masks. Keep only other shapes in the winding state.
    pub(super) fn intersection(
        shapes: &[Shape],
        origin: (u32, u32),
        size: (u32, u32),
        row: &mut Row,
    ) -> Result<(Self, usize)> {
        let mut bounds = Self::viewport(origin, size);
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

    pub(super) fn viewport(origin: (u32, u32), size: (u32, u32)) -> Self {
        Self {
            left: f64::from(origin.0),
            top: f64::from(origin.1),
            right: f64::from(origin.0) + f64::from(size.0),
            bottom: f64::from(origin.1) + f64::from(size.1),
        }
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

    pub(super) fn write(self, out: &mut [u8], stride: usize, origin: (u32, u32)) -> Result<()> {
        out.fill(0);
        if self.is_empty() {
            return Ok(());
        }
        let columns = self.columns(stride, origin)?;
        for (y, row) in out.chunks_exact_mut(stride).enumerate() {
            let height = self.height(y, origin.1)?;
            if height > 0.0 {
                columns.write(&mut row[columns.start..columns.end], height);
            }
        }
        Ok(())
    }

    // The covered columns of every row, relative to the mask origin.
    fn columns(self, stride: usize, origin: (u32, u32)) -> Result<Columns> {
        let left = self.left - f64::from(origin.0);
        let right = self.right - f64::from(origin.0);
        let start = index(left.floor(), stride);
        let end = index(right.ceil(), stride);
        let column =
            f64::from(u32::try_from(start).map_err(|_| Error::limit("column exceeds u32"))?);
        Ok(Columns {
            start,
            end,
            first: (right.min(column + 1.0) - left).min(1.0),
            last: right - (right.ceil() - 1.0).max(left),
        })
    }

    // The covered height of mask row `y`, in global device coordinates.
    fn height(self, y: usize, origin: u32) -> Result<f64> {
        let y = f64::from(u32::try_from(y).map_err(|_| Error::limit("row exceeds u32"))?)
            + f64::from(origin);
        Ok((self.bottom.min(y + 1.0) - self.top.max(y)).clamp(0.0, 1.0))
    }
}

#[derive(Debug, Clone, Copy)]
struct Columns {
    start: usize,
    end: usize,
    first: f64,
    last: f64,
}

impl Columns {
    // One row's samples from `start` to `end`; a single column takes `last`.
    fn write(self, run: &mut [u8], height: f64) {
        run.fill(quantize_unit(height));
        if let Some(pixel) = run.first_mut() {
            *pixel = quantize_unit(self.first * height);
        }
        if let Some(pixel) = run.last_mut() {
            *pixel = quantize_unit(self.last * height);
        }
    }
}

/// Nonzero row runs of closed-form rectangle coverage, without a mask.
///
/// A row's coverage depends only on its covered height. Rows strictly
/// between the first and the last covered row have unit height, because
/// their bounds are integers inside the rectangle, so at most three distinct
/// rows exist. Each is written once by [`Rectangle::write`]'s row code and
/// split into runs once by [`crate::Mask::spans`]'s splitter, so the runs
/// equal those it finds in the dense mask.
#[derive(Debug, Default)]
pub(crate) struct Rows {
    samples: Vec<u8>,
    start: usize,
    bands: [Band; 3],
}

// Mask rows `rows.0..rows.1` share the samples at `offset` and their runs,
// as (column after `start`, length).
#[derive(Debug, Default, Clone, Copy)]
struct Band {
    rows: (usize, usize),
    offset: usize,
    runs: [(usize, usize); 3],
    count: usize,
}

impl Rows {
    pub(super) fn new(
        rectangle: Rectangle,
        (width, height): (u32, u32),
        origin: (u32, u32),
        mut samples: Vec<u8>,
    ) -> Result<Self> {
        samples.clear();
        let mut rows = Self {
            samples,
            ..Self::default()
        };
        if rectangle.is_empty() {
            return Ok(rows);
        }
        let stride =
            usize::try_from(width).map_err(|_| Error::limit("mask width exceeds usize"))?;
        let height =
            usize::try_from(height).map_err(|_| Error::limit("mask height exceeds usize"))?;
        let columns = rectangle.columns(stride, origin)?;
        let length = columns.end - columns.start;
        // Rows outside the covered interval have no positive height.
        let first = index(rectangle.top.floor() - f64::from(origin.1), height);
        let end = index(rectangle.bottom.ceil() - f64::from(origin.1), height);
        let Some(last) = end.checked_sub(1).filter(|&last| last >= first) else {
            return Ok(rows);
        };
        rows.start = columns.start;
        let bands = [
            (first, first + 1),
            (first + 1, last.max(first + 1)),
            (last.max(first + 1), last + 1),
        ];
        // One distinct row per covered row, up to three, so never more
        // samples than the mask would have held.
        rows.samples
            .try_reserve_exact(length.saturating_mul((last - first + 1).min(bands.len())))
            .map_err(|_| Error::limit("mask allocation failed"))?;
        for (band, range) in rows.bands.iter_mut().zip(bands) {
            if range.0 >= range.1 {
                continue;
            }
            let covered = rectangle.height(range.0, origin.1)?;
            band.rows = range;
            if covered > 0.0 {
                band.offset = rows.samples.len();
                rows.samples.resize(band.offset + length, 0);
                let samples = &mut rows.samples[band.offset..];
                columns.write(samples, covered);
                // A left edge sample, a constant interior and a right edge
                // sample split into at most three runs.
                let mut runs = Runs::new(samples);
                for (slot, (x, run)) in band.runs.iter_mut().zip(&mut runs) {
                    *slot = (x, run.len());
                    band.count += 1;
                }
                debug_assert!(runs.next().is_none());
            }
        }
        Ok(rows)
    }

    /// Returns the sample storage for reuse.
    pub(crate) fn into_samples(self) -> Vec<u8> {
        self.samples
    }

    /// Nonzero runs as `(row, column, coverage)`, as [`crate::Mask::spans`].
    pub(crate) fn spans(&self) -> impl Iterator<Item = (usize, usize, &[u8])> {
        self.bands.iter().flat_map(move |band| {
            let samples = &self.samples[band.offset..];
            let runs = &band.runs[..band.count];
            (band.rows.0..band.rows.1).flat_map(move |y| {
                runs.iter()
                    .map(move |&(x, length)| (y, self.start + x, &samples[x..x + length]))
            })
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod convex_tests {
    use super::*;
    use crate::{FillRule, PathBuilder, Point, Transform};
    use std::sync::Arc;

    fn shape(points: [[f64; 2]; 4]) -> Shape {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(points[0][0], points[0][1]));
        for [x, y] in &points[1..] {
            path.line_to(Point::new(*x, *y)).unwrap();
        }
        path.close();
        Shape {
            geometry: Geometry::Path(Arc::new(path.finish())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        }
    }
    #[test]
    fn convex_half_planes_certify_only_strict_interior_hulls() {
        for offset in [0.0, 1e9] {
            for reverse in [false, true] {
                let mut points = [[0.0, 2.0], [2.0, 0.0], [4.0, 2.0], [2.0, 4.0]];
                for p in &mut points {
                    p[0] += offset;
                    p[1] += offset;
                }
                if reverse {
                    points.reverse();
                }
                let clip = shape(points);
                let hull = |a, b| {
                    [
                        Point::new(offset + a, offset + a),
                        Point::new(offset + b, offset + b),
                    ]
                };
                assert!(clip.contains_hull(hull(1.5, 2.5)));
                assert!(!clip.contains_hull(hull(1.0, 3.0)));
                assert!(!clip.contains_hull(hull(0.0, 4.0)));
            }
        }
        let crossed = shape([[0.0, 0.0], [4.0, 4.0], [0.0, 4.0], [4.0, 0.0]]);
        assert!(!crossed.contains_hull([Point::new(1.9, 1.9), Point::new(2.1, 2.1)]));
    }
}
