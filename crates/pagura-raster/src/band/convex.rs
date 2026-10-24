//! Convex clipping paths as two chains of strictly increasing height.
//!
//! A convex clipping path meets every horizontal line in one interval, from
//! its left chain to its right chain. A mark strictly inside every convex
//! path of its clip state meets none of their boundaries, so it keeps its
//! own edges and needs neither the joint row union nor the clip's mask.

use super::edges::Polygon;
use crate::Point;
use crate::geometry::convex_orientation;
use pagura_core::{Meter, Result};
use std::cmp::Ordering;

/// A convex clipping path's left and right chains, each from the top height
/// to the bottom height with strictly increasing heights and at least two
/// points.
#[derive(Debug)]
pub(crate) struct Convex {
    left: Vec<Point>,
    right: Vec<Point>,
    top: f64,
    bottom: f64,
}

impl Convex {
    /// The chains of `polygon` when it is one contour certified simple and
    /// convex, apart from repeated points; `None` otherwise. Scratch and
    /// chains are charged to `bytes`.
    pub(crate) fn new(polygon: &Polygon, bytes: &mut Meter) -> Result<Option<Self>> {
        if polygon.starts.len() != 1 || polygon.points_cells {
            return Ok(None);
        }
        let mut points = Vec::new();
        bytes.reserve(&mut points, polygon.points.len())?;
        for &p in &polygon.points {
            if points.last() != Some(&p) {
                points.push(p);
            }
        }
        while points.len() > 1 && points.first() == points.last() {
            points.pop();
        }
        let scratch = points.capacity() * size_of::<Point>();
        let convex = Self::chains(&points, bytes);
        drop(points);
        bytes.refund(scratch);
        convex
    }

    fn chains(points: &[Point], bytes: &mut Meter) -> Result<Option<Self>> {
        let Some(orientation) = convex_orientation(points) else {
            return Ok(None);
        };
        let count = points.len();
        // The topmost point, the leftmost of two at the top height.
        let Some((top, first)) = points
            .iter()
            .enumerate()
            .min_by(|(_, p), (_, q)| p.y.total_cmp(&q.y).then(p.x.total_cmp(&q.x)))
        else {
            return Ok(None);
        };
        let bottom = points.iter().fold(first.y, |y, p| y.max(p.y));
        let forward = chain(points, top, bottom, |i| (i + 1) % count, bytes)?;
        let backward = chain(points, top, bottom, |i| (i + count - 1) % count, bytes)?;
        // Turning with a positive cross product in device space (y down),
        // the contour runs clockwise: down its right side from the top.
        let (left, right) = if orientation > 0.0 {
            (backward, forward)
        } else {
            (forward, backward)
        };
        Ok(Some(Self {
            left,
            right,
            top: first.y,
            bottom,
        }))
    }

    /// Bytes retained.
    pub(crate) fn retained_bytes(&self) -> usize {
        (self.left.capacity() + self.right.capacity()) * size_of::<Point>()
    }

    /// Whether `p` lies strictly inside: between the top and bottom heights,
    /// and right of the left chain and left of the right chain by
    /// orientation signs certified against their rounding.
    pub(crate) fn contains(&self, p: Point) -> bool {
        p.y > self.top
            && p.y < self.bottom
            && beside(&self.left, p) == Some(Ordering::Greater)
            && beside(&self.right, p) == Some(Ordering::Less)
    }
}

/// The chain from the point `top` in the direction of `step`: past a
/// horizontal top side, then down to the first point at height `bottom`.
fn chain(
    points: &[Point],
    top: usize,
    bottom: f64,
    step: impl Fn(usize) -> usize,
    bytes: &mut Meter,
) -> Result<Vec<Point>> {
    let height = points[top].y;
    let mut first = top;
    for _ in 0..points.len() {
        if points[step(first)].y != height {
            break;
        }
        first = step(first);
    }
    let mut length = 1;
    let mut i = first;
    while points[i].y != bottom && length <= points.len() {
        i = step(i);
        length += 1;
    }
    let mut chain = Vec::new();
    bytes.reserve(&mut chain, length)?;
    let mut i = first;
    chain.push(points[i]);
    for _ in 1..length {
        i = step(i);
        chain.push(points[i]);
    }
    Ok(chain)
}

/// The index `k` of the chain side from `chain[k - 1]` to `chain[k]` that
/// holds height `y`, the lower one at a chain point.
fn segment(chain: &[Point], y: f64) -> usize {
    chain
        .partition_point(|p| p.y <= y)
        .clamp(1, chain.len().saturating_sub(1).max(1))
}

/// Whether `p` lies right (`Greater`) or left (`Less`) of the chain at its
/// height, when the orientation sign is certified: the differences, products
/// and their difference round by less than four units of the products'
/// magnitudes, and the guard takes eight, plus an underflow allowance.
fn beside(chain: &[Point], p: Point) -> Option<Ordering> {
    let k = segment(chain, p.y);
    let (a, b) = (chain[k - 1], chain[k]);
    let along = (b.x - a.x) * (p.y - a.y);
    let across = (b.y - a.y) * (p.x - a.x);
    let d = across - along;
    (d.abs() > 8.0 * f64::EPSILON * (along.abs() + across.abs()) + f64::MIN_POSITIVE)
        .then(|| d.total_cmp(&0.0))
}

#[cfg(test)]
mod tests;
