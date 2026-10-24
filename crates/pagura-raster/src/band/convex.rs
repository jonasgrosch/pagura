//! Convex clipping paths as two chains of strictly increasing height.
//!
//! A convex clipping path meets every horizontal line in one interval, from
//! its left chain to its right chain. A mark is clipped to it as to a
//! rectangle (8.5.4): each edge keeps its part between the chains, a part
//! beside a chain moves horizontally onto that chain, and a part above or
//! below the path becomes horizontal and bounds no area. That map fixes the
//! region and moves every other point along a horizontal segment outside it,
//! or to the top or bottom height through heights outside it, so the moved
//! boundary winds around each point of the region as before and around no
//! point outside: the mark's winding times the region's indicator. A winding
//! of zero or one sign of magnitude one stays so, and a fill rule selects
//! the same points inside the region.
//!
//! Each clipped piece lies on one stored line, the edge or a side of a
//! chain, between two heights, and its ends are evaluated on that line, as
//! the row union's sides are. Where the edge enters or leaves the region,
//! the crossing height is interpolated as the row union interpolates its
//! crossings; the pieces before and after it end there on their own lines,
//! joined by a horizontal step that bounds no area. A crossing height off by
//! a rounding unit moves area only within that unit's slab, however flat the
//! lines meet.

use super::edges::{Line, Polygon};
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

/// A mark edge at one height: its own x and the chains' x there.
#[derive(Debug, Clone, Copy)]
struct At {
    edge: f64,
    left: f64,
    right: f64,
}

impl At {
    /// The line the clipped edge lies on here, chain sides `l` and `r`
    /// holding the height: beside a chain that chain's side, else the edge.
    fn on(self, l: usize, r: usize) -> On {
        if self.edge < self.left {
            On::Left(l)
        } else if self.edge > self.right {
            On::Right(r)
        } else {
            On::Edge
        }
    }
}

/// The stored line a clipped piece lies on: the edge, or a side of a chain
/// by its index (see [`segment`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum On {
    Edge,
    Left(usize),
    Right(usize),
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

    /// Every side of both chains.
    pub(crate) fn sides(&self) -> impl Iterator<Item = Line> + '_ {
        [&self.left, &self.right]
            .into_iter()
            .flat_map(|chain| chain.windows(2).map(|w| Line { a: w[0], b: w[1] }))
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

    /// At most how many pieces [`Self::clip`] appends for `line`: the chain
    /// points strictly within its heights divide them into intervals, and
    /// two crossings divide each interval into at most three pieces.
    pub(crate) fn bound(&self, line: Line) -> usize {
        let (lo, hi) = (line.a.y.min(line.b.y), line.a.y.max(line.b.y));
        let inner = |chain: &[Point]| {
            chain
                .partition_point(|p| p.y < hi)
                .saturating_sub(chain.partition_point(|p| p.y <= lo))
        };
        inner(&self.left)
            .saturating_add(inner(&self.right))
            .saturating_mul(3)
            .saturating_add(3)
    }

    /// Appends the pieces of `line` clipped to the region, in its direction;
    /// `out` has room for [`Self::bound`] more. A line inside keeps its two
    /// stored end points.
    pub(crate) fn clip(&self, line: Line, out: &mut Vec<Line>) {
        let reversed = line.a.y > line.b.y;
        let (upper, lower) = if reversed {
            (line.b, line.a)
        } else {
            (line.a, line.b)
        };
        let lo = upper.y.max(self.top);
        let hi = lower.y.min(self.bottom);
        if lo >= hi {
            return;
        }
        let at = |y: f64, l: usize, r: usize| At {
            edge: line.x(y),
            left: chain_x(&self.left, l, y),
            right: chain_x(&self.right, r, y),
        };
        let inside = |a: At| a.left <= a.edge && a.edge <= a.right;
        let (mut l, mut r) = (segment(&self.left, lo), segment(&self.right, lo));
        let mut here = at(lo, l, r);
        // Both ends inside: by convexity, so is the line. A line without a
        // chain point within its heights takes one step below anyway.
        if (self.left[l].y < hi || self.right[r].y < hi)
            && lo == upper.y
            && hi == lower.y
            && inside(here)
            && inside(at(hi, segment(&self.left, hi), segment(&self.right, hi)))
        {
            out.push(line);
            return;
        }
        let first = out.len();
        let mut piece = |on: On, (a, b): (f64, f64)| {
            let line = match on {
                On::Edge => line,
                On::Left(k) => side(&self.left, k),
                On::Right(k) => side(&self.right, k),
            };
            out.push(Line {
                a: Point::new(line.x(a), a),
                b: Point::new(line.x(b), b),
            });
        };
        // The line of the current piece and the height it starts at.
        let mut current: Option<(On, f64)> = None;
        let mut y = lo;
        loop {
            let next = self.left[l].y.min(self.right[r].y).min(hi);
            let there = at(next, l, r);
            // Between consecutive heights the part lies on one line.
            let (heights, count) = heights((here, there), (y, next));
            for pair in heights[..count].windows(2) {
                if pair[0] == pair[1] {
                    continue;
                }
                let on = at(pair[0].midpoint(pair[1]), l, r).on(l, r);
                match current {
                    Some((line, start)) if line != on => {
                        piece(line, (start, pair[0]));
                        current = Some((on, pair[0]));
                    }
                    Some(_) => {}
                    None => current = Some((on, pair[0])),
                }
            }
            if next >= hi {
                break;
            }
            // Below `hi`, so not either chain's last point.
            if next == self.left[l].y {
                l += 1;
            }
            if next == self.right[r].y {
                r += 1;
            }
            y = next;
            here = there;
        }
        if let Some((line, start)) = current {
            piece(line, (start, hi));
        }
        if reversed {
            let pieces = &mut out[first..];
            pieces.reverse();
            for piece in pieces {
                *piece = Line {
                    a: piece.b,
                    b: piece.a,
                };
            }
        }
    }
}

/// The heights from `y` to `next` and, strictly between them, where the edge
/// crosses a chain, from the differences at both ends as the row union
/// computes its crossings; sorted, with their count.
fn heights((here, there): (At, At), (y, next): (f64, f64)) -> ([f64; 4], usize) {
    let mut heights = [y, next, next, next];
    let mut count = 1;
    for (d0, d1) in [
        (here.edge - here.left, there.edge - there.left),
        (here.right - here.edge, there.right - there.edge),
    ] {
        if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
            let cut = y + (next - y) * (d0 / (d0 - d1));
            if cut > y && cut < next {
                heights[count] = cut;
                count += 1;
            }
        }
    }
    heights[count] = next;
    heights[..=count].sort_unstable_by(f64::total_cmp);
    (heights, count + 1)
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

/// Chain side `k`, from `chain[k - 1]` to `chain[k]`.
fn side(chain: &[Point], k: usize) -> Line {
    Line {
        a: chain[k - 1],
        b: chain[k],
    }
}

/// The x of chain side `k` at height `y`.
fn chain_x(chain: &[Point], k: usize, y: f64) -> f64 {
    side(chain, k).x(y)
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
