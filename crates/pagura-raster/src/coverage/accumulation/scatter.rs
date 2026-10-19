//! Counting-sorted row admission and integer pieces with shared grid crossings.

use super::super::{capacity_bytes, charge, index, sort_work};
use super::dda::{Dda, ordinate, quotient};
use super::geometry::{GRID, Geometry, reserve};
use pagura_core::{Error, Limits, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Piece {
    pub(super) column: u32,
    pub(super) segment: u32,
    pub(super) a: [i32; 2],
    pub(super) b: [i32; 2],
}

// Original-segment and ghost-piece counts over a column interval. Ghosts
// carry no area or cover; keeping them here also preserves zero-length images.
#[derive(Debug, Clone, Copy)]
pub(super) struct Risk {
    pub(super) column: u32,
    pub(super) originals: i64,
    pub(super) ghosts: i64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SpanEvent {
    pub(super) column: u32,
    pub(super) segment: u32,
    pub(super) height: i32,
    pub(super) entering: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Active {
    pub(super) segment: u32,
    last: u32,
    x: i64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Admission {
    segment: u32,
    last: u32,
}

#[derive(Debug, Clone, Copy)]
struct GridRow {
    left: i64,
    right: i64,
    top: i64,
}

#[derive(Debug, Clone, Copy)]
struct RowPiece {
    a: [i64; 2],
    b: [i64; 2],
    segment: usize,
    reverse: bool,
}

#[derive(Debug, Default)]
pub(super) struct Scatter {
    pub(super) pieces: Vec<Piece>,
    pub(super) risks: Vec<Risk>,
    pub(super) spans: Vec<SpanEvent>,
    pub(super) carry: Vec<i64>,
    pub(super) active: Vec<Active>,
    order: Vec<Admission>,
    offsets: Vec<usize>,
    seen: Vec<u32>,
}

impl Scatter {
    pub(super) fn retained_bytes(&self) -> usize {
        [
            capacity_bytes(&self.pieces),
            capacity_bytes(&self.risks),
            capacity_bytes(&self.spans),
            capacity_bytes(&self.carry),
            capacity_bytes(&self.active),
            capacity_bytes(&self.order),
            capacity_bytes(&self.offsets),
            capacity_bytes(&self.seen),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }

    pub(super) fn prepare(
        &mut self,
        geometry: &Geometry,
        origin: (u32, u32),
        height: u32,
        limits: &Limits,
        work: &mut usize,
    ) -> Result<()> {
        self.active.clear();
        self.order.clear();
        self.offsets.clear();
        self.seen.clear();
        self.carry.clear();
        let rows = usize::try_from(height).map_err(|_| Error::limit("row count exceeds usize"))?;
        let count = rows
            .checked_add(1)
            .ok_or(Error::limit("row count overflow"))?;
        reserve(&mut self.offsets, count)?;
        self.offsets.resize(count, 0);
        reserve(&mut self.seen, geometry.contours.len())?;
        self.seen.resize(geometry.contours.len(), 0);
        reserve(&mut self.carry, geometry.rules.len())?;
        self.carry.resize(geometry.rules.len(), 0);
        charge(work, geometry.segments.len() + count, limits.raster_work)?;
        for segment in 0..geometry.segments.len() {
            if let Some((first, _)) = rows_for(geometry, segment, origin.1, height) {
                self.offsets[first as usize + 1] += 1;
            }
        }
        for row in 0..rows {
            self.offsets[row + 1] += self.offsets[row];
        }
        let total = self.offsets[rows];
        reserve(&mut self.order, total)?;
        self.order.resize(total, Admission::default());
        for segment in 0..geometry.segments.len() {
            if let Some((first, last)) = rows_for(geometry, segment, origin.1, height) {
                self.offsets[first as usize + 1] -= 1;
                self.order[self.offsets[first as usize + 1]] = Admission {
                    segment: u32::try_from(segment)
                        .map_err(|_| Error::limit("segment index exceeds u32"))?,
                    last,
                };
            }
        }
        for row in 0..rows {
            self.offsets[row] = self.offsets[row + 1];
        }
        self.offsets[rows] = total;
        Ok(())
    }

    // At most this many original segments can meet any cell in the row.
    // A bound at or below 512 proves that no spatial density scan is needed.
    pub(super) fn sparse_bound(&self) -> Option<i64> {
        if self.active.len() <= 512 {
            i64::try_from(self.active.len()).ok()
        } else {
            None
        }
    }

    pub(super) fn row(
        &mut self,
        geometry: &Geometry,
        origin: (u32, u32),
        width: u32,
        row: u32,
        limits: &Limits,
        work: &mut usize,
    ) -> Result<()> {
        self.pieces.clear();
        self.risks.clear();
        self.spans.clear();
        self.carry.fill(0);
        charge(work, self.active.len(), limits.raster_work)?;
        self.active.retain(|a| a.last >= row);
        let r = row as usize;
        let top = (i64::from(origin.1) + i64::from(row)) * GRID;
        reserve(&mut self.active, self.offsets[r + 1] - self.offsets[r])?;
        for admission in &self.order[self.offsets[r]..self.offsets[r + 1]] {
            let segment = admission.segment as usize;
            let a = geometry.points[segment];
            let b = geometry.points[geometry.next(segment)];
            let (a, b) = if a[1] <= b[1] { (a, b) } else { (b, a) };
            let x = interpolate(a, b, top.clamp(a[1], b[1]), 1);
            self.active.push(Active {
                segment: admission.segment,
                last: admission.last,
                x,
            });
        }
        let grid = GridRow {
            left: i64::from(origin.0) * GRID,
            right: (i64::from(origin.0) + i64::from(width)) * GRID,
            top,
        };
        for i in 0..self.active.len() {
            charge(work, 1, limits.raster_work)?;
            self.original(
                geometry,
                self.active[i].segment as usize,
                origin,
                width,
                row,
            )?;
            self.segment(geometry, i, grid, limits, work)?;
        }
        self.sort(limits, work)
    }

    fn segment(
        &mut self,
        geometry: &Geometry,
        active: usize,
        grid: GridRow,
        limits: &Limits,
        work: &mut usize,
    ) -> Result<()> {
        let GridRow {
            left,
            right,
            top: y,
        } = grid;
        let segment = self.active[active].segment as usize;
        let next = geometry.next(segment);
        let (a, b) = (geometry.points[segment], geometry.points[next]);
        if a == b {
            return Ok(());
        }
        if a[1] == b[1] {
            if a[1] > y && a[1] < y + GRID {
                self.horizontal(a, b, segment, left, right, y)?;
            }
            return Ok(());
        }
        let (mut a, mut b, reverse) = if a[1] < b[1] {
            (a, b, false)
        } else {
            (b, a, true)
        };
        let lo = a[1].max(y);
        let hi = b[1].min(y + GRID);
        if lo >= hi {
            return Ok(());
        }
        let ax = if lo == a[1] {
            a[0]
        } else {
            self.active[active].x
        };
        let bx = interpolate(a, b, hi, 1);
        self.active[active].x = bx;
        a = [ax, lo];
        b = [bx, hi];
        let group = geometry.group(segment);
        if a[0].max(b[0]) < left || (a[0].max(b[0]) == left && a[0] != b[0]) {
            add(
                &mut self.carry[group],
                if reverse { lo - hi } else { hi - lo },
            )?;
            return Ok(());
        }
        let first = a[0].min(b[0]).div_euclid(GRID).max(left / GRID);
        let last = if a[0] == b[0] {
            first
        } else {
            (a[0].max(b[0]) - 1).div_euclid(GRID)
        }
        .min(right / GRID - 1);
        if first > last {
            return Ok(());
        }
        let (l, r) = if a[0] < b[0] { (a, b) } else { (b, a) };
        let start = l[0].max(left);
        let first_cut = if start > l[0] {
            start
        } else {
            (l[0].div_euclid(GRID) + 1) * GRID
        };
        let mut columns = if last - first >= 3 {
            Dda::new(l, r, first_cut, 0)?
        } else {
            None
        };
        let previous = if columns.is_some() {
            ordinate(l, r, start, 0, &mut columns)?
        } else {
            interpolate(l, r, start, 0)
        };
        if a[0].min(b[0]) < left {
            let dy = if a[0] < b[0] {
                previous - a[1]
            } else {
                b[1] - previous
            };
            add(&mut self.carry[group], if reverse { -dy } else { dy })?;
        }
        let count = usize::try_from(last - first + 1)
            .map_err(|_| Error::limit("piece count exceeds usize"))?;
        charge(work, count, limits.raster_work)?;
        reserve(&mut self.pieces, count)?;
        self.cells(
            RowPiece {
                a,
                b,
                segment,
                reverse,
            },
            grid,
            first..=last,
            &mut columns,
            previous,
        )
    }

    fn cells(
        &mut self,
        line: RowPiece,
        grid: GridRow,
        range: std::ops::RangeInclusive<i64>,
        columns: &mut Option<Dda>,
        mut previous: i64,
    ) -> Result<()> {
        let RowPiece {
            a,
            b,
            segment,
            reverse,
        } = line;
        let GridRow { left, top: y, .. } = grid;
        let (l, r) = if a[0] < b[0] { (a, b) } else { (b, a) };
        for column in range {
            let (mut p, mut q) = if a[0] == b[0] {
                (a, b)
            } else {
                let x0 = l[0].max(column * GRID);
                let x1 = r[0].min((column + 1) * GRID);
                let next = if columns.is_some() {
                    ordinate(l, r, x1, 0, columns)?
                } else {
                    interpolate(l, r, x1, 0)
                };
                let p = [x0, previous];
                let q = [x1, next];
                previous = next;
                if a[0] < b[0] { (p, q) } else { (q, p) }
            };
            if reverse {
                std::mem::swap(&mut p, &mut q);
            }
            self.pieces.push(piece(column, segment, p, q, left, y)?);
        }
        Ok(())
    }

    fn sort(&mut self, limits: &Limits, work: &mut usize) -> Result<()> {
        for count in [self.pieces.len(), self.risks.len(), self.spans.len()] {
            charge(work, sort_work(count)?, limits.raster_work)?;
        }
        self.pieces.sort_unstable_by_key(|p| p.column);
        self.risks.sort_unstable_by_key(|p| p.column);
        self.spans.sort_unstable_by_key(|p| p.column);
        Ok(())
    }

    fn original(
        &mut self,
        geometry: &Geometry,
        segment: usize,
        origin: (u32, u32),
        width: u32,
        row: u32,
    ) -> Result<()> {
        let next = geometry.next(segment);
        let contour = &geometry.contours[geometry.segments[segment].contour as usize];
        if contour.collapsed && self.seen[geometry.segments[segment].contour as usize] != row + 1 {
            self.seen[geometry.segments[segment].contour as usize] = row + 1;
            let [lo, _, hi, _] = contour.bounds;
            let start = index((lo - f64::from(origin.0)).ceil() - 1.0, width as usize);
            let end = index((hi - f64::from(origin.0)).floor() + 1.0, width as usize);
            self.risk(start, end, 0, 1)?;
        }
        let count_density = self.active.len() > 512;
        if !count_density && !geometry.segments[segment].ghost {
            return Ok(());
        }
        let (a, b) = (geometry.exact[segment], geometry.exact[next]);
        let top = f64::from(origin.1) + f64::from(row);
        let extent = if a.y == b.y {
            (a.y >= top && a.y < top + 1.0).then_some((a.x, b.x))
        } else {
            let lo = a.y.min(b.y).max(top);
            let hi = a.y.max(b.y).min(top + 1.0);
            (lo < hi).then(|| {
                let at = |y| a.x + (b.x - a.x) * ((y - a.y) / (b.y - a.y));
                (at(lo), at(hi))
            })
        };
        if let Some((a, b)) = extent {
            // One-column enclosure absorbs floating intersection uncertainty
            // at the 10^12 coordinate limit. Overcounting only selects the
            // unsnapped fallback earlier; it never weakens the area bound.
            let start = index(
                (a.min(b) - f64::from(origin.0)).floor() - 1.0,
                width as usize,
            );
            let end = index(
                (a.max(b) - f64::from(origin.0)).floor() + 2.0,
                width as usize,
            );
            self.risk(
                start,
                end,
                i64::from(count_density),
                i64::from(geometry.segments[segment].ghost),
            )?;
        }
        Ok(())
    }

    fn risk(&mut self, first: usize, end: usize, originals: i64, ghosts: i64) -> Result<()> {
        if first < end {
            reserve(&mut self.risks, 2)?;
            self.risks.push(Risk {
                column: u32::try_from(first).map_err(|_| Error::limit("column exceeds u32"))?,
                originals,
                ghosts,
            });
            self.risks.push(Risk {
                column: u32::try_from(end).map_err(|_| Error::limit("column exceeds u32"))?,
                originals: -originals,
                ghosts: -ghosts,
            });
        }
        Ok(())
    }

    fn horizontal(
        &mut self,
        a: [i64; 2],
        b: [i64; 2],
        segment: usize,
        left: i64,
        right: i64,
        y: i64,
    ) -> Result<()> {
        let lo = a[0].min(b[0]).max(left);
        let hi = a[0].max(b[0]).min(right);
        if lo >= hi {
            return Ok(());
        }
        let first = lo.div_euclid(GRID);
        let last = (hi - 1).div_euclid(GRID);
        reserve(&mut self.pieces, 2)?;
        for column in std::iter::once(first).chain((last != first).then_some(last)) {
            let p = [lo.max(column * GRID), a[1]];
            let q = [hi.min((column + 1) * GRID), a[1]];
            let (p, q) = if a[0] < b[0] { (p, q) } else { (q, p) };
            self.pieces.push(piece(column, segment, p, q, left, y)?);
        }
        if last > first + 1 {
            reserve(&mut self.spans, 2)?;
            let segment =
                u32::try_from(segment).map_err(|_| Error::limit("segment index exceeds u32"))?;
            let height =
                i32::try_from(a[1] - y).map_err(|_| Error::limit("piece height exceeds i32"))?;
            self.spans.push(SpanEvent {
                column: u32::try_from(first + 1 - left / GRID)
                    .map_err(|_| Error::limit("column exceeds u32"))?,
                segment,
                height,
                entering: true,
            });
            self.spans.push(SpanEvent {
                column: u32::try_from(last - left / GRID)
                    .map_err(|_| Error::limit("column exceeds u32"))?,
                segment,
                height,
                entering: false,
            });
        }
        Ok(())
    }
}

pub(super) fn add(target: &mut i64, value: i64) -> Result<()> {
    *target = target
        .checked_add(value)
        .ok_or(Error::limit("integer cover overflow"))?;
    Ok(())
}

fn piece(
    column: i64,
    segment: usize,
    a: [i64; 2],
    b: [i64; 2],
    left: i64,
    y: i64,
) -> Result<Piece> {
    let local = |p: [i64; 2]| {
        Ok([
            i32::try_from(p[0] - column * GRID).map_err(|_| Error::limit("piece x exceeds i32"))?,
            i32::try_from(p[1] - y).map_err(|_| Error::limit("piece y exceeds i32"))?,
        ])
    };
    Ok(Piece {
        column: u32::try_from(column - left / GRID)
            .map_err(|_| Error::limit("column exceeds u32"))?,
        segment: u32::try_from(segment).map_err(|_| Error::limit("segment index exceeds u32"))?,
        a: local(a)?,
        b: local(b)?,
    })
}

// The same closed form is used at both sides of every tile boundary. The
// Euclidean quotient implements half-up rounding, including negative slopes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "closed-form interpolation stays between bounded endpoints, within 2^60"
)]
pub(super) fn interpolate(mut a: [i64; 2], mut b: [i64; 2], at: i64, axis: usize) -> i64 {
    if a[axis] > b[axis] {
        std::mem::swap(&mut a, &mut b);
    }
    let other = 1 - axis;
    if at == a[axis] {
        return a[other];
    }
    if at == b[axis] {
        return b[other];
    }
    let extent = i128::from(b[axis]) - i128::from(a[axis]);
    let numerator =
        2 * (i128::from(at) - i128::from(a[axis])) * (i128::from(b[other]) - i128::from(a[other]))
            + extent;
    // The interpolated coordinate lies between bounded endpoints (< 2^60).
    (i128::from(a[other]) + quotient(numerator, (2 * extent) as i64)) as i64
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "bounded device ordinates have magnitude at most 10^12"
)]
fn floor(value: f64) -> i64 {
    value.floor() as i64
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "visible row indices are clamped to 0..height, where height is u32"
)]
fn rows_for(g: &Geometry, segment: usize, origin: u32, height: u32) -> Option<(u32, u32)> {
    let next = g.next(segment);
    let lo = floor(g.exact[segment].y.min(g.exact[next].y))
        .min(g.points[segment][1].min(g.points[next][1]).div_euclid(GRID));
    let hi = floor(g.exact[segment].y.max(g.exact[next].y))
        .max(g.points[segment][1].max(g.points[next][1]).div_euclid(GRID));
    let contour = &g.contours[g.segments[segment].contour as usize];
    let original_min = g.exact[segment].y.min(g.exact[next].y);
    let lo = if contour.collapsed && original_min == original_min.floor() {
        lo.min(floor(original_min) - 1)
    } else {
        lo
    };
    let first = (lo - i64::from(origin)).max(0);
    let last = (hi - i64::from(origin)).min(i64::from(height) - 1);
    if first > last {
        None
    } else {
        Some((first as u32, last as u32))
    }
}

#[cfg(test)]
mod tests;
