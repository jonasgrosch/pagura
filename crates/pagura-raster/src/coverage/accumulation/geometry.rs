//! Page-anchored geometry, contour adjacency and monotone chain identities.

use crate::flatten::{CurveStack, Polyline, bounded, flatten_into};
use crate::{FillRule, Point};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Meter, Result};

use super::super::{Shape, capacity_bytes};

/// Grid units per device pixel: snapped coordinates are multiples of 2^-20 px.
pub(super) const GRID: i64 = 1 << 20;
/// [`GRID`] in a cell-local coordinate.
pub(super) const CELL: i32 = 1 << 20;
/// [`GRID`] as a scale factor.
pub(super) const SCALE: f64 = 1_048_576.0;

#[derive(Debug, Clone, Copy)]
pub(super) struct Segment {
    pub(super) chain: u32,
    pub(super) contour: u32,
    pub(super) ghost: bool,
}

#[derive(Debug)]
pub(super) struct Contour {
    pub(super) first: usize,
    pub(super) len: usize,
    pub(super) group: usize,
    pub(super) bounds: [f64; 4],
    pub(super) collapsed: bool,
    pub(super) enclosed: Option<[i64; 2]>,
}

#[derive(Debug, Default)]
pub(super) struct Geometry {
    pub(super) exact: Vec<Point>,
    pub(super) points: Vec<[i64; 2]>,
    pub(super) segments: Vec<Segment>,
    pub(super) contours: Vec<Contour>,
    pub(super) rules: Vec<FillRule>,
    flattened: Vec<Polyline>,
    curves: CurveStack,
}

impl Geometry {
    pub(super) fn retained_bytes(&self) -> usize {
        [
            capacity_bytes(&self.exact),
            capacity_bytes(&self.points),
            capacity_bytes(&self.segments),
            capacity_bytes(&self.contours),
            capacity_bytes(&self.rules),
            capacity_bytes(&self.flattened),
            capacity_bytes(&self.curves),
        ]
        .into_iter()
        .chain(self.flattened.iter().map(|c| capacity_bytes(&c.points)))
        .fold(0, usize::saturating_add)
    }

    pub(super) fn build(
        &mut self,
        shapes: &[Shape],
        limits: &Limits,
        work: &mut Meter,
    ) -> Result<()> {
        self.exact.clear();
        self.points.clear();
        self.segments.clear();
        self.contours.clear();
        self.rules.clear();
        if shapes.len() > limits.mask_sources {
            return Err(Error::limit("mask source budget exceeded"));
        }
        reserve(&mut self.rules, shapes.len())?;
        let mut chain = 0;
        for shape in shapes {
            self.rules.push(shape.rule);
            let contours = contours(shape, &mut self.flattened, &mut self.curves, limits, work)?;
            let count = contours.len();
            for (i, contour) in contours.iter().enumerate() {
                if i + 1 == count && !contour.closed && contour.points.len() == 1 {
                    continue;
                }
                let first = self.exact.len();
                let corners = contour
                    .points
                    .first()
                    .filter(|_| contour.degenerate)
                    .map(|&p| {
                        let p = p * shape.transform;
                        let (x, y) = (p.x.floor(), p.y.floor());
                        // 8.5.3.3.1: only source-degenerate subpaths enclose a floor cell.
                        [
                            Point::new(x, y),
                            Point::new(x + 1.0, y),
                            Point::new(x + 1.0, y + 1.0),
                            Point::new(x, y + 1.0),
                        ]
                    });
                let count = corners.as_ref().map_or(contour.points.len(), |_| 4);
                work.charge(count)?;
                if self
                    .exact
                    .len()
                    .checked_add(count)
                    .is_none_or(|n| n > limits.path_segments)
                {
                    return Err(Error::limit("geometry segment budget exceeded"));
                }
                reserve(&mut self.exact, count)?;
                for j in 0..count {
                    let p = corners
                        .as_ref()
                        .map_or_else(|| contour.points[j] * shape.transform, |c| c[j]);
                    bounded(p)?;
                    if self.exact.len() == first || self.exact.last() != Some(&p) {
                        self.exact.push(p);
                    }
                }
                if self.exact.len() > first + 1 && self.exact.last() == self.exact.get(first) {
                    self.exact.pop();
                }
                let len = self.exact.len() - first;
                if len < 2 {
                    self.exact.truncate(first);
                    continue;
                }
                let id = u32::try_from(self.contours.len())
                    .map_err(|_| Error::limit("contour index exceeds u32"))?;
                reserve(&mut self.points, len)?;
                reserve(&mut self.segments, len)?;
                for &p in &self.exact[first..] {
                    self.points.push([snap(p.x), snap(p.y)]);
                    self.segments.push(Segment {
                        chain: 0,
                        contour: id,
                        ghost: false,
                    });
                }
                let points = &self.points[first..];
                let (bounds, enclosed) = bounds(&self.exact[first..], points);
                let collapsed = collinear(points);
                reserve(&mut self.contours, 1)?;
                self.contours.push(Contour {
                    first,
                    len,
                    group: self.rules.len() - 1,
                    bounds,
                    collapsed,
                    enclosed,
                });
                for j in first..first + len {
                    let next = if j + 1 == first + len { first } else { j + 1 };
                    let (a, b) = (self.points[j], self.points[next]);
                    self.segments[j].ghost =
                        a == b || (self.exact[j].y != self.exact[next].y && a[1] == b[1]);
                }
                assign_chains(points, &mut self.segments[first..], &mut chain)?;
            }
        }
        Ok(())
    }

    pub(super) fn next(&self, segment: usize) -> usize {
        let c = &self.contours[u32_to_usize(self.segments[segment].contour)];
        if segment + 1 == c.first + c.len {
            c.first
        } else {
            segment + 1
        }
    }

    pub(super) fn group(&self, segment: usize) -> usize {
        self.contours[u32_to_usize(self.segments[segment].contour)].group
    }

    pub(super) fn neighbours(&self, a: usize, b: usize) -> bool {
        self.segments[a].contour == self.segments[b].contour
            && (self.next(a) == b || self.next(b) == a)
    }
}

fn contours<'a>(
    shape: &'a Shape,
    flattened: &'a mut Vec<Polyline>,
    curves: &mut CurveStack,
    limits: &Limits,
    work: &mut Meter,
) -> Result<&'a [Polyline]> {
    Ok(match &shape.geometry {
        super::super::Geometry::Path(path) => {
            let count = flatten_into(path, shape.transform, limits, flattened, curves)?;
            flattened.truncate(count);
            flattened.as_slice()
        }
        super::super::Geometry::Glyph(glyph) => {
            if glyph.vertices() > limits.path_segments {
                return Err(Error::limit("flattened segment budget exceeded"));
            }
            // Every placement pays for canonical geometry, including cache hits.
            work.charge(glyph.vertices())?;
            glyph.contours()
        }
    })
}

pub(super) fn reserve<T>(values: &mut Vec<T>, additional: usize) -> Result<()> {
    values
        .try_reserve(additional)
        .map_err(|_| Error::limit("raster geometry allocation failed"))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "bounded device coordinates times 2^20 are within 2^60"
)]
fn snap(value: f64) -> i64 {
    (value * SCALE).round_ties_even() as i64
}

fn direction(a: [i64; 2], b: [i64; 2]) -> i8 {
    match a[1].cmp(&b[1]) {
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => -1,
    }
}

fn assign_chains(points: &[[i64; 2]], segments: &mut [Segment], counter: &mut u32) -> Result<()> {
    let n = points.len();
    let last = (0..n)
        .rev()
        .find(|&i| points[i][1] != points[(i + 1) % n][1]);
    let Some(last) = last else {
        *counter = counter
            .checked_add(1)
            .ok_or(Error::limit("chain index overflow"))?;
        for segment in segments {
            segment.chain = *counter;
        }
        return Ok(());
    };
    let mut previous = direction(points[last], points[(last + 1) % n]);
    let start = (0..n)
        .find(|&i| {
            let d = direction(points[i], points[(i + 1) % n]);
            let changed = d != 0 && d != previous;
            if d != 0 {
                previous = d;
            }
            changed
        })
        .ok_or(Error::invalid_argument(
            "closed contour has inconsistent directions",
        ))?;
    let mut previous = 0;
    for offset in 0..n {
        let i = (start + offset) % n;
        let d = direction(points[i], points[(i + 1) % n]);
        let prior = (i + n - 1) % n;
        let spike = d == 0
            && direction(points[prior], points[i]) == 0
            && (points[i][0] - points[prior][0]).signum()
                != (points[(i + 1) % n][0] - points[i][0]).signum();
        if offset == 0 || (d != 0 && d != previous) || spike {
            *counter = counter
                .checked_add(1)
                .ok_or(Error::limit("chain index overflow"))?;
        }
        segments[i].chain = *counter;
        if d != 0 {
            previous = d;
        }
    }
    Ok(())
}

fn collinear(points: &[[i64; 2]]) -> bool {
    let a = points[0];
    let Some(&b) = points.iter().find(|&&p| p != a) else {
        return true;
    };
    let dx = i128::from(b[0]) - i128::from(a[0]);
    let dy = i128::from(b[1]) - i128::from(a[1]);
    points.iter().all(|p| {
        dx * (i128::from(p[1]) - i128::from(a[1])) == dy * (i128::from(p[0]) - i128::from(a[0]))
    })
}

fn bounds(exact: &[Point], points: &[[i64; 2]]) -> ([f64; 4], Option<[i64; 2]>) {
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut lo = [i64::MAX; 2];
    let mut hi = [i64::MIN; 2];
    for (p, q) in exact.iter().zip(points) {
        bounds[0] = bounds[0].min(p.x);
        bounds[1] = bounds[1].min(p.y);
        bounds[2] = bounds[2].max(p.x);
        bounds[3] = bounds[3].max(p.y);
        for axis in 0..2 {
            lo[axis] = lo[axis].min(q[axis]);
            hi[axis] = hi[axis].max(q[axis]);
        }
    }
    // A maximum on a grid line can belong to the cell immediately before it.
    let cell = [(hi[0] - 1).div_euclid(GRID), (hi[1] - 1).div_euclid(GRID)];
    let enclosed = (lo[0] >= cell[0] * GRID && lo[1] >= cell[1] * GRID).then_some(cell);
    (bounds, enclosed)
}

#[cfg(test)]
mod tests;
