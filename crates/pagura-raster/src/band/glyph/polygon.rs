//! A placed view of canonical stored polygon sides, with no edge copy.

use super::super::edges::{Line, Polygon};
use crate::Point;
use crate::band::cells::{WIDE, WINDOW};
use crate::flatten::{MAX_COORDINATE, bounded};
use pagura_core::{Result, convert::u32_to_usize};
use std::num::NonZeroU32;

/// Source facts for uncut, short polygon placements. The positive cell
/// bound lets an optional summary use its existing zero niche.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Summary {
    hull: [Point; 2],
    cells: NonZeroU32,
    boundaries: u32,
}

impl Summary {
    /// A source-fixed bound; large or empty sources keep ordinary setup.
    pub(crate) fn new(polygon: &Polygon) -> Option<Self> {
        let hull = polygon.bounds()?;
        // An accepted rounded sum lies in a rounding cell of a coordinate
        // inside MAX_COORDINATE, even if its exact sum is slightly outside.
        // EPS*MAX bounds that cell's radius and subnormal rounding too.
        let rounding = (f64::EPSILON * MAX_COORDINATE).next_up();
        let (mut cells, mut boundaries) = (0_u32, 0_u32);
        for line in polygon.sides().filter(|line| !line.is_horizontal()) {
            let rows = extent(line.a.y, line.b.y, rounding)?.checked_add(1)?;
            let columns = extent(line.a.x, line.b.x, rounding)?.checked_add(4)?;
            cells = cells.checked_add(rows.checked_mul(columns)?)?;
            boundaries = boundaries.checked_add(1)?;
        }
        Some(Self {
            hull,
            cells: NonZeroU32::new(cells)?,
            boundaries,
        })
    }

    /// Monotone rounded addition makes these four extrema sufficient to
    /// validate every canonical vertex before any clip or empty result.
    pub(crate) fn placed_hull(self, origin: Point) -> Result<[Point; 2]> {
        let [lo, hi] = self.hull.map(|point| point + origin);
        bounded(lo)?;
        bounded(hi)?;
        Ok([lo, hi])
    }

    /// Keep real strip-local traversal for tall or wide complete glyphs,
    /// even when the requested region contains only a small part of one.
    #[expect(
        clippy::cast_precision_loss,
        reason = "the shared window width is 4096"
    )]
    pub(crate) fn short([lo, hi]: [Point; 2]) -> bool {
        let height = hi.y.ceil() - lo.y.floor();
        let width = hi.x.ceil() + 1.0 - lo.x.floor();
        height.is_finite()
            && width.is_finite()
            && (0.0..=f64::from(WINDOW)).contains(&height)
            && (0.0..=WIDE as f64).contains(&width)
    }

    pub(crate) fn boundaries(self) -> usize {
        u32_to_usize(self.boundaries)
    }

    /// Raw visits, primitive work, touched emission and the ordinary row
    /// overhead, independent of bands and width-dependent window splits.
    pub(crate) fn work(self, raw: usize, (rows, columns): (usize, usize)) -> Option<usize> {
        let cells = u32_to_usize(self.cells.get()).min(
            self.boundaries()
                .checked_mul(rows)?
                .checked_mul(columns.checked_add(3)?)?,
        );
        let touched = cells.min(rows.checked_mul(columns)?);
        let per_row = (columns / 64)
            .checked_add(2)?
            .checked_add(columns)?
            .checked_add(4)?;
        raw.checked_mul(rows)?
            .checked_add(cells)?
            .checked_add(touched.checked_mul(3)?)?
            .checked_add(rows.checked_mul(per_row)?)
    }
}

/// An outward integer bound on one translated endpoint displacement.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite nonnegative ceiling checked against u32 before conversion"
)]
fn extent(a: f64, b: f64, rounding: f64) -> Option<u32> {
    let span = ((a - b).abs().next_up() + 2.0 * rounding).next_up().ceil();
    (span.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&span)).then_some(span as u32)
}

/// The canonical point cell's vertical sides, already placed and in
/// contour order. No origin or default zero is added to these endpoints.
pub(crate) fn point_sides(point: Point) -> [Line; 2] {
    let [a, b, c, d] = Polygon::cell_corners(point);
    [Line { a: b, b: c }, Line { a: d, b: a }]
}

/// A borrowed view of the stored polygon, with canonical placed point cells.
#[derive(Clone, Copy)]
pub(crate) struct PolygonLines<'a> {
    polygon: &'a Polygon,
    points: &'a [Point],
    origin: Point,
}

impl<'a> PolygonLines<'a> {
    pub(crate) fn new(polygon: &'a Polygon, points: &'a [Point], origin: Point) -> Self {
        Self {
            polygon,
            points,
            origin,
        }
    }

    /// The source side key range; unrepresentable keys keep owned geometry.
    pub(crate) fn keys(self) -> Option<usize> {
        key_count(
            self.polygon.vertices(),
            self.polygon.starts.len(),
            self.points.len(),
        )
    }

    pub(crate) fn raw_count(self) -> Option<usize> {
        self.polygon
            .vertices()
            .checked_add(self.points.len().checked_mul(4)?)
    }

    fn point_base(self) -> usize {
        self.polygon
            .vertices()
            .saturating_add(self.polygon.starts.len())
    }

    fn at(self, point: Point) -> Point {
        Point::new(point.x + self.origin.x, point.y + self.origin.y)
    }

    fn point_side(self, point: usize, side: usize) -> Option<Line> {
        point_sides(self.at(*self.points.get(point)?))
            .get(side)
            .copied()
    }

    pub(crate) fn line(self, key: usize) -> Option<Line> {
        let base = self.point_base();
        if key < base {
            let line = self.polygon.side(key)?;
            Some(Line {
                a: self.at(line.a),
                b: self.at(line.b),
            })
        } else {
            let key = key - base;
            self.point_side(key / 2, key % 2)
        }
    }

    /// Every placed side in [`Self::indexed`] order, without keys: one pass
    /// over the stored contours, then the point cells' sides.
    pub(crate) fn for_each(self, mut visit: impl FnMut(Line)) {
        let points = &self.polygon.points;
        for (contour, &start) in self.polygon.starts.iter().enumerate() {
            let end = self
                .polygon
                .starts
                .get(contour + 1)
                .copied()
                .unwrap_or(points.len());
            let Some(contour) = points.get(start..end) else {
                continue;
            };
            for (i, &a) in contour.iter().enumerate() {
                let next = if i + 1 == contour.len() { 0 } else { i + 1 };
                if let Some(&b) = contour.get(next) {
                    visit(Line {
                        a: self.at(a),
                        b: self.at(b),
                    });
                }
            }
        }
        for &point in self.points {
            for side in point_sides(self.at(point)) {
                visit(side);
            }
        }
    }

    /// Keep source order, never numeric key order: a closing key precedes
    /// the next contour's ordinary vertex keys.
    pub(crate) fn indexed(self) -> impl Iterator<Item = (usize, Line)> + Clone {
        let sides = self.polygon.indexed_sides().map(move |(key, line)| {
            (
                key,
                Line {
                    a: self.at(line.a),
                    b: self.at(line.b),
                },
            )
        });
        sides.chain((0..self.points.len()).flat_map(move |point| {
            (0..2).filter_map(move |side| {
                let key = self
                    .point_base()
                    .saturating_add(point.saturating_mul(2))
                    .saturating_add(side);
                self.point_side(point, side).map(|line| (key, line))
            })
        }))
    }
}

fn key_count(vertices: usize, contours: usize, points: usize) -> Option<usize> {
    let count = vertices
        .checked_add(contours)?
        .checked_add(points.checked_mul(2)?)?;
    u32::try_from(count).ok()?;
    Some(count)
}

#[cfg(test)]
mod tests;
