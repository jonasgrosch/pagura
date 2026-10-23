//! A placed view of canonical stored polygon sides, with no edge copy.

use super::super::edges::{Line, Polygon};
use crate::Point;

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
        let corners = Polygon::cell_corners(self.at(*self.points.get(point)?));
        let i = side * 2 + 1;
        Some(Line {
            a: corners[i],
            b: corners[(i + 1) % corners.len()],
        })
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
