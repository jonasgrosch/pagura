//! Prepared device geometry: closed polygons, their straight edges and
//! monotone chains, clipping to a rectangle, and the index of edges by band.
//!
//! Edges keep the coordinates they were built with. Bands and the requested
//! region only select which edges and rows are visited, never which
//! arithmetic produces a cell, so coverage is independent of both.

use super::DeviceRect;
use crate::flatten::{CurveStack, Polyline, bounded, flatten_into};
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};

/// A straight edge in device space, in contour order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Line {
    pub(crate) a: Point,
    pub(crate) b: Point,
}

impl Line {
    /// The x of the edge at height `y` within its vertical extent: the end
    /// points exactly, and between them by the weighted endpoint numerator.
    /// Its products and sums keep their rounding tails: distant endpoints
    /// may cancel almost completely at an ordinary device pixel.
    pub(crate) fn x(self, y: f64) -> f64 {
        let (top, bottom) = if self.a.y < self.b.y {
            (self.a, self.b)
        } else {
            (self.b, self.a)
        };
        if y <= top.y {
            top.x
        } else if y >= bottom.y {
            bottom.x
        } else if top.x == bottom.x {
            top.x
        } else {
            let (height, low) = sum(bottom.y, -top.y);
            // Normalize the vertical weights by an exact power of two.
            // Subnormal heights must not underflow their x products.
            let exponent = (height.to_bits() >> 52).max(1);
            let scale = f64::from_bits((2046 - exponent) << 52);
            let (wa, ea) = sum(bottom.y, -y);
            let (wb, eb) = sum(y, -top.y);
            let (wa, ea, wb, eb) = (wa * scale, ea * scale, wb * scale, eb * scale);
            let (height, low) = (height * scale, low * scale);
            let (pa, ra) = product(top.x, wa);
            let (pb, rb) = product(bottom.x, wb);
            let (numerator, tail) = sum(pa, pb);
            let tail = tail + ra + rb + top.x * ea + bottom.x * eb;
            let quotient = numerator / height;
            let (back, error) = product(quotient, height);
            let x = quotient + (((numerator - back) + tail - error) - quotient * low) / height;
            x.max(top.x.min(bottom.x)).min(top.x.max(bottom.x))
        }
    }

    /// The height of a nonvertical line at column `x`, through the same
    /// interpolation with the coordinate axes exchanged.
    pub(crate) fn y(self, x: f64) -> f64 {
        Self {
            a: Point::new(self.a.y, self.a.x),
            b: Point::new(self.b.y, self.b.x),
        }
        .x(x)
    }

    pub(crate) fn is_horizontal(self) -> bool {
        self.a.y == self.b.y
    }
}

/// The exact sum of two finite values, apart from subnormal underflow.
fn sum(a: f64, b: f64) -> (f64, f64) {
    let high = a + b;
    let back = high - a;
    (high, (a - (high - back)) + (b - back))
}

/// Product and rounding tail without fused operations. Device coordinates
/// are bounded by 10^12, so splitting and the products cannot overflow.
fn product(a: f64, b: f64) -> (f64, f64) {
    let split = |value| {
        let expanded = value * 134_217_729.0;
        let high = expanded - (expanded - value);
        (high, value - high)
    };
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    let high = a * b;
    let low = ((ah * bh - high) + ah * bl + al * bh) + al * bl;
    (high, low)
}

#[cfg(test)]
#[test]
fn distant_endpoints_preserve_the_stored_line_inside_a_pixel() {
    let n = 549_755_813_888.0;
    let u = 1.0 / 8192.0;
    let line = Line {
        a: Point::new(-n, -n),
        b: Point::new(n, n + u),
    };
    // Exact rational x(y) = n * (2*y-u) / (2*n+u), rounded to f64.
    let expected = [
        (0.0, -6.103_515_624_999_999e-5),
        (0.5, 0.499_938_964_843_749_94),
        (1.0, 0.999_938_964_843_749_9),
    ];
    for (y, x) in expected {
        assert_eq!(line.x(y), x);
        assert_eq!(
            Line {
                a: line.b,
                b: line.a
            }
            .x(y),
            x
        );
    }
    assert_eq!(line.x(line.a.y), line.a.x);
    assert_eq!(line.x(line.b.y), line.b.x);
    // Its inverse at zero is exactly u/2; an uncorrected height
    // interpolation from either distant end loses that displacement.
    assert_eq!(line.y(0.0), u * 0.5);
}

#[cfg(test)]
#[test]
fn interpolation_keeps_subnormal_heights_and_endpoint_ranges() {
    let tiny = f64::from_bits(1);
    let line = Line {
        a: Point::new(0.25, 0.0),
        b: Point::new(0.5, 2.0 * tiny),
    };
    assert_eq!(line.x(tiny), 0.375);
    let line = Line {
        a: Point::new(tiny, 0.0),
        b: Point::new(2.0 * tiny, 2.0 * tiny),
    };
    assert!((tiny..=2.0 * tiny).contains(&line.x(tiny)));
}

/// Closed device polygons: contour `i` is `points[starts[i]..starts[i + 1]]`
/// and closes from its last point to its first.
#[derive(Debug, Default)]
pub(crate) struct Polygon {
    pub(crate) points: Vec<Point>,
    pub(crate) starts: Vec<usize>,
    /// Whether some contour is the floor cell of a geometric point.
    pub(crate) points_cells: bool,
}

impl Polygon {
    pub(crate) fn contours(&self) -> impl Iterator<Item = &[Point]> {
        self.starts.iter().enumerate().map(|(i, &start)| {
            let end = self.starts.get(i + 1).copied().unwrap_or(self.points.len());
            &self.points[start..end]
        })
    }

    pub(crate) fn vertices(&self) -> usize {
        self.points.len()
    }

    /// The fill polygon of `path` under `transform`, flattened within the
    /// exact tolerance (1/256 pixel).
    pub(crate) fn from_path(path: &Path, transform: Transform, limits: &Limits) -> Result<Self> {
        let mut contours: Vec<Polyline> = Vec::new();
        let mut curves = CurveStack::new();
        let count = flatten_into(path, transform, limits, &mut contours, &mut curves)?;
        let mut polygon = Self::default();
        polygon.extend(&contours[..count], transform, limits)?;
        Ok(polygon)
    }

    /// [`Self::from_path`] for a path of lines only, without flattening:
    /// the same contours, points and errors. `None` for a path with curves.
    pub(crate) fn from_lines(
        path: &Path,
        transform: Transform,
        limits: &Limits,
    ) -> Result<Option<Self>> {
        if path
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. }))
        {
            return Ok(None);
        }
        crate::flatten::validate(path, transform, limits)?;
        let mut polygon = Self::default();
        let mut count = 0_usize;
        // The current contour's user points start at `from` in `points`,
        // untransformed until the contour ends.
        let mut user: Vec<Point> = Vec::new();
        let mut closed = false;
        let finish =
            |polygon: &mut Self, user: &mut Vec<Point>, last: bool, closed: bool| -> Result<()> {
                let Some(&first) = user.first() else {
                    return Ok(());
                };
                if last && !closed && user.len() == 1 {
                    user.clear();
                    return Ok(());
                }
                if user.iter().all(|&p| p == first) {
                    let p = first * transform;
                    bounded(p)?;
                    polygon.cell(p, limits)?;
                } else {
                    polygon.begin(limits, user.len())?;
                    for &point in user.iter() {
                        let p = point * transform;
                        bounded(p)?;
                        polygon.points.push(p);
                    }
                    polygon.end();
                }
                user.clear();
                Ok(())
            };
        let mut append = |user: &mut Vec<Point>, point: Point| -> Result<()> {
            if count >= limits.path_segments {
                return Err(Error::limit("flattened segment budget exceeded"));
            }
            count += 1;
            user.try_reserve(1)
                .map_err(|_| Error::limit("raster geometry allocation failed"))?;
            user.push(point);
            Ok(())
        };
        for segment in path.segments() {
            match *segment {
                Segment::MoveTo(point) => {
                    finish(&mut polygon, &mut user, false, closed)?;
                    bounded(point)?;
                    bounded(point * transform)?;
                    append(&mut user, point)?;
                    closed = false;
                }
                Segment::LineTo(point) => {
                    if closed {
                        // A line after a close reopens at the subpath start.
                        let first = user.first().copied();
                        finish(&mut polygon, &mut user, false, closed)?;
                        user.extend(first);
                        closed = false;
                    }
                    bounded(point)?;
                    bounded(point * transform)?;
                    append(&mut user, point)?;
                }
                Segment::Close => closed = true,
                Segment::CubicTo { .. } => return Ok(None),
            }
        }
        finish(&mut polygon, &mut user, true, closed)?;
        Ok(Some(polygon))
    }

    /// Adds flattened contours: a final open one-point contour is omitted,
    /// a geometric point encloses its floor cell (8.5.3.3.1), and every other
    /// contour closes implicitly.
    pub(crate) fn extend(
        &mut self,
        contours: &[Polyline],
        transform: Transform,
        limits: &Limits,
    ) -> Result<()> {
        let count = contours.len();
        for (i, contour) in contours.iter().enumerate() {
            if i + 1 == count && !contour.closed && contour.points.len() == 1 {
                continue;
            }
            let Some(&first) = contour.points.first() else {
                continue;
            };
            if contour.degenerate {
                let p = first * transform;
                bounded(p)?;
                self.cell(p, limits)?;
                continue;
            }
            self.begin(limits, contour.points.len())?;
            for &point in &contour.points {
                let p = point * transform;
                bounded(p)?;
                self.points.push(p);
            }
            self.end();
        }
        Ok(())
    }

    /// The floor cell of a geometric point at `p`.
    pub(crate) fn cell(&mut self, p: Point, limits: &Limits) -> Result<()> {
        let (x, y) = (p.x.floor(), p.y.floor());
        self.begin(limits, 4)?;
        self.points.extend([
            Point::new(x, y),
            Point::new(x + 1.0, y),
            Point::new(x + 1.0, y + 1.0),
            Point::new(x, y + 1.0),
        ]);
        self.points_cells = true;
        self.end();
        Ok(())
    }

    pub(crate) fn begin(&mut self, limits: &Limits, count: usize) -> Result<()> {
        if self
            .points
            .len()
            .checked_add(count)
            .is_none_or(|n| n > limits.path_segments)
        {
            return Err(Error::limit("flattened segment budget exceeded"));
        }
        self.points
            .try_reserve(count)
            .and_then(|()| self.starts.try_reserve(1))
            .map_err(|_| Error::limit("raster geometry allocation failed"))?;
        self.starts.push(self.points.len());
        Ok(())
    }

    /// Ends the contour begun last, dropping repeated points; a contour of
    /// fewer than two distinct points encloses nothing and is removed.
    pub(crate) fn end(&mut self) {
        let Some(&start) = self.starts.last() else {
            return;
        };
        let mut write = start;
        for read in start..self.points.len() {
            let p = self.points[read];
            if write == start || self.points[write - 1] != p {
                self.points[write] = p;
                write += 1;
            }
        }
        if write > start + 1 && self.points[write - 1] == self.points[start] {
            write -= 1;
        }
        self.points.truncate(write);
        if write < start + 2 {
            self.points.truncate(start);
            self.starts.pop();
        }
    }

    /// Removes the edges that coincide with an edge of opposite direction,
    /// in pairs, and relinks the rest into contours: the winding number off
    /// the removed edges stays the same, and tiles that meet along shared
    /// edges become the boundary of their union. Returns whether any edge
    /// was removed; the polygon is unchanged when none was or the rest does
    /// not relink.
    pub(crate) fn cancel_shared(&mut self) -> Result<bool> {
        let allocation = |_| Error::limit("raster geometry allocation failed");
        // Points ordered by their coordinates' total order, as integers.
        let key = |p: Point| (ordered(p.x), ordered(p.y));
        let mut edges: Vec<Keyed> = Vec::new();
        edges
            .try_reserve_exact(self.points.len())
            .map_err(allocation)?;
        for contour in self.contours() {
            let n = contour.len();
            edges.extend((0..n).map(|i| {
                let (a, b) = (contour[i], contour[(i + 1) % n]);
                let (ka, kb) = (key(a), key(b));
                Keyed {
                    lo: ka.min(kb),
                    hi: ka.max(kb),
                    forward: ka <= kb,
                    from: ka,
                    to: kb,
                    a,
                }
            }));
        }
        // Each edge by its undirected key, the reversed ones first;
        // opposite directions cancel.
        edges.sort_unstable_by_key(|e| (e.lo, e.hi, e.forward));
        let mut kept: Vec<Keyed> = Vec::new();
        kept.try_reserve_exact(edges.len()).map_err(allocation)?;
        let mut start = 0;
        while let Some(first) = edges.get(start) {
            let end = start
                + edges[start..]
                    .iter()
                    .take_while(|e| e.lo == first.lo && e.hi == first.hi)
                    .count();
            let group = &edges[start..end];
            let reversed = group.iter().filter(|e| !e.forward).count();
            let cancelled = reversed.min(group.len() - reversed);
            kept.extend_from_slice(&group[cancelled..reversed]);
            kept.extend_from_slice(&group[reversed + cancelled..]);
            start = end;
        }
        if kept.len() == edges.len() {
            return Ok(false);
        }
        // Relink by start point: any pairing at a shared vertex keeps the
        // winding.
        kept.sort_unstable_by_key(|e| e.from);
        let mut used: Vec<bool> = Vec::new();
        used.try_reserve_exact(kept.len()).map_err(allocation)?;
        used.resize(kept.len(), false);
        let mut points = Vec::new();
        let mut starts = Vec::new();
        points.try_reserve_exact(kept.len()).map_err(allocation)?;
        for first in 0..kept.len() {
            if used[first] {
                continue;
            }
            starts.try_reserve(1).map_err(allocation)?;
            starts.push(points.len());
            let origin = kept[first].from;
            let mut current = first;
            loop {
                used[current] = true;
                points.push(kept[current].a);
                let next = kept[current].to;
                if next == origin {
                    break;
                }
                let from = kept.partition_point(|e| e.from < next);
                let Some(found) = (from..kept.len())
                    .take_while(|&i| kept[i].from == next)
                    .find(|&i| !used[i])
                else {
                    return Ok(false);
                };
                current = found;
            }
        }
        self.points = points;
        self.starts = starts;
        Ok(true)
    }

    /// The device bounds of the vertices, as `[min, max]`.
    pub(crate) fn bounds(&self) -> Option<[Point; 2]> {
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in &self.points {
            lo.x = lo.x.min(p.x);
            lo.y = lo.y.min(p.y);
            hi.x = hi.x.max(p.x);
            hi.y = hi.y.max(p.y);
        }
        (lo.x <= hi.x && lo.y <= hi.y).then_some([lo, hi])
    }
}

/// An edge keyed for cancelling: its ends' integer keys, undirected and
/// directed, and its first end.
#[derive(Debug, Clone, Copy)]
struct Keyed {
    lo: (i64, i64),
    hi: (i64, i64),
    forward: bool,
    from: (i64, i64),
    to: (i64, i64),
    a: Point,
}

/// The total order of f64 as an integer: equal keys exactly for equal bits.
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "bit reinterpretation of the total order"
)]
fn ordered(value: f64) -> i64 {
    let bits = value.to_bits() as i64;
    bits ^ (((bits >> 63) as u64) >> 1) as i64
}

/// A straight edge with the monotone chain it belongs to and its group (a
/// clip path of an intersection), for the analysis of uncertified cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Chained {
    pub(crate) line: Line,
    pub(crate) chain: u32,
    pub(crate) group: u8,
    /// Whether the edge's contour fits in one pixel, where it can enclose
    /// a region no side of the pixel meets.
    pub(crate) enclosed: bool,
    /// The edge's place: its contour, its index there, and the contour's
    /// edge count; pieces of one edge share it.
    pub(crate) place: Place,
}

/// Where an edge lies in its polygon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Place {
    pub(crate) contour: u32,
    pub(crate) index: u32,
    pub(crate) count: u32,
}

impl Place {
    /// Whether two edges are one or consecutive edges of one contour.
    pub(crate) fn adjacent(self, other: Self) -> bool {
        self.contour == other.contour
            && (self.index == other.index
                || (self.index + 1) % self.count.max(1) == other.index
                || (other.index + 1) % other.count.max(1) == self.index)
    }
}

/// The edges of `polygon` in contour order with their chains: maximal runs
/// whose vertical direction does not reverse. Chain numbers continue from
/// `next`, which is advanced.
pub(crate) fn chained(
    polygon: &Polygon,
    group: u8,
    next: &mut u32,
    out: &mut Vec<Chained>,
) -> Result<()> {
    out.try_reserve(polygon.vertices())
        .map_err(|_| Error::limit("raster geometry allocation failed"))?;
    for (contour_number, contour) in polygon.contours().enumerate() {
        let n = contour.len();
        let direction = |i: usize| {
            let (a, b) = (contour[i].y, contour[(i + 1) % n].y);
            if a < b {
                1
            } else if a > b {
                -1
            } else {
                0
            }
        };
        let base = out.len();
        let (mut lo, mut hi) = (contour[0], contour[0]);
        for p in contour {
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let enclosed = hi.x.ceil() - lo.x.floor() <= 1.0 && hi.y.ceil() - lo.y.floor() <= 1.0;
        let count = u32::try_from(n).map_err(|_| Error::limit("contour exceeds u32"))?;
        let contour_index =
            u32::try_from(contour_number).map_err(|_| Error::limit("contours exceed u32"))?;
        for (i, index) in (0..n).zip(0_u32..) {
            out.push(Chained {
                line: Line {
                    a: contour[i],
                    b: contour[(i + 1) % n],
                },
                chain: 0,
                group,
                enclosed,
                place: Place {
                    contour: contour_index,
                    index,
                    count,
                },
            });
        }
        let Some(last) = (0..n).rev().find(|&i| direction(i) != 0) else {
            *next = next
                .checked_add(1)
                .ok_or(Error::limit("chain index overflow"))?;
            for edge in &mut out[base..] {
                edge.chain = *next;
            }
            continue;
        };
        // Start where the direction changes, so a walk around the contour
        // never splits a chain at the seam.
        let mut previous = direction(last);
        let start = (0..n)
            .find(|&i| {
                let d = direction(i);
                let changed = d != 0 && d != previous;
                if d != 0 {
                    previous = d;
                }
                changed
            })
            .unwrap_or(0);
        let mut previous = 0;
        for offset in 0..n {
            let i = (start + offset) % n;
            let d = direction(i);
            let prior = (i + n - 1) % n;
            // A horizontal reversal is a spike of zero area: a new chain.
            let spike = d == 0
                && direction(prior) == 0
                && (contour[i].x - contour[prior].x).signum()
                    != (contour[(i + 1) % n].x - contour[i].x).signum();
            if offset == 0 || (d != 0 && d != previous) || spike {
                *next = next
                    .checked_add(1)
                    .ok_or(Error::limit("chain index overflow"))?;
            }
            out[base + i].chain = *next;
            if d != 0 {
                previous = d;
            }
        }
    }
    Ok(())
}

/// A device rectangle `[left, right] × [top, bottom]` of real coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bounds {
    pub(crate) left: f64,
    pub(crate) top: f64,
    pub(crate) right: f64,
    pub(crate) bottom: f64,
}

impl Bounds {
    pub(crate) fn intersect(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }

    pub(crate) fn is_empty(self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }

    pub(crate) fn contains(self, other: Self) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && other.right <= self.right
            && other.bottom <= self.bottom
    }

    /// The pixels the rectangle meets, within `region`.
    pub(crate) fn pixels(self, region: DeviceRect) -> DeviceRect {
        if self.is_empty() {
            return DeviceRect::default();
        }
        pixels(
            [
                Point::new(self.left, self.top),
                Point::new(self.right, self.bottom),
            ],
            region,
            false,
        )
    }
}

/// The pixels whose interior or floor cell `[min, max]` can touch, within
/// `region`. With `floor_cells`, a maximum on a pixel boundary keeps the
/// pixel after it, as a geometric point's floor cell needs.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the u32 region before conversion"
)]
pub(crate) fn pixels([min, max]: [Point; 2], region: DeviceRect, floor_cells: bool) -> DeviceRect {
    let clamp = |v: f64, lo: u32, hi: u32| v.clamp(f64::from(lo), f64::from(hi)) as u32;
    let end = |v: f64| {
        if floor_cells {
            v.floor() + 1.0
        } else {
            v.ceil()
        }
    };
    let rect = DeviceRect {
        x0: clamp(min.x.floor(), region.x0, region.x1),
        y0: clamp(min.y.floor(), region.y0, region.y1),
        x1: clamp(end(max.x), region.x0, region.x1),
        y1: clamp(end(max.y), region.y0, region.y1),
    };
    if rect.is_empty() {
        DeviceRect::default()
    } else {
        rect
    }
}

/// Clips a straight edge to `bounds`, as [`clip`] does.
pub(crate) fn clip_line(line: Line, bounds: Bounds, out: &mut Vec<Line>) -> Result<()> {
    out.try_reserve(3)
        .map_err(|_| Error::limit("raster geometry allocation failed"))?;
    clip_pieces(line, bounds, |piece| out.push(piece));
    Ok(())
}

/// The straight edges of `polygon` that bound area (not horizontal).
pub(crate) fn lines(polygon: &Polygon) -> Result<Vec<Line>> {
    let mut out = Vec::new();
    out.try_reserve(polygon.vertices())
        .map_err(|_| Error::limit("raster geometry allocation failed"))?;
    for contour in polygon.contours() {
        let n = contour.len();
        for i in 0..n {
            let (a, b) = (contour[i], contour[(i + 1) % n]);
            if a.y != b.y {
                out.push(Line { a, b });
            }
        }
    }
    Ok(out)
}

/// Clips a chained edge to `bounds`: parts above or below disappear and
/// parts left or right of it become vertical edges on its sides, which keeps
/// the winding inside and leaves none outside (8.5.4). Horizontal edges are
/// kept within the bounds' height with their x clamped.
pub(crate) fn clip(edge: Chained, bounds: Bounds, out: &mut Vec<Chained>) -> Result<()> {
    out.try_reserve(3)
        .map_err(|_| Error::limit("raster geometry allocation failed"))?;
    clip_pieces(edge.line, bounds, |line| out.push(Chained { line, ..edge }));
    Ok(())
}

/// The at most three pieces of `line` within `bounds`, in its direction.
fn clip_pieces(line: Line, bounds: Bounds, mut emit: impl FnMut(Line)) {
    if bounds.is_empty() {
        return;
    }
    let Line { a, b } = line;
    let x = |p: f64| p.clamp(bounds.left, bounds.right);
    if a.y == b.y {
        if a.y > bounds.top && a.y < bounds.bottom && x(a.x) != x(b.x) {
            emit(Line {
                a: Point::new(x(a.x), a.y),
                b: Point::new(x(b.x), b.y),
            });
        }
        return;
    }
    let reversed = a.y > b.y;
    let (top, bottom) = if reversed { (b, a) } else { (a, b) };
    let lo = top.y.max(bounds.top);
    let hi = bottom.y.min(bounds.bottom);
    if lo >= hi {
        return;
    }
    let mut events = [lo, hi, 0.0, 0.0];
    let mut count = 2;
    if top.x != bottom.x {
        for side in [bounds.left, bounds.right] {
            let y = line.y(side);
            if y > lo && y < hi {
                events[count] = y;
                count += 1;
            }
        }
    }
    let events = &mut events[..count];
    events.sort_unstable_by(f64::total_cmp);
    let mut pieces = [(Point::default(), Point::default()); 3];
    for (piece, pair) in pieces.iter_mut().zip(events.windows(2)) {
        *piece = (
            Point::new(x(line.x(pair[0])), pair[0]),
            Point::new(x(line.x(pair[1])), pair[1]),
        );
    }
    let pieces = &pieces[..count - 1];
    if reversed {
        for &(p, q) in pieces.iter().rev() {
            emit(Line { a: q, b: p });
        }
    } else {
        for &(p, q) in pieces {
            emit(Line { a: p, b: q });
        }
    }
}

/// The rows an edge with vertical extent `[lo, hi]` has area in, within the
/// region, as `first..=last`; horizontal edges keep the row they lie in.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the u32 region before conversion"
)]
pub(crate) fn rows(lo: f64, hi: f64, region: DeviceRect) -> Option<(u32, u32)> {
    if region.is_empty() {
        return None;
    }
    let first = lo.floor();
    let last = if hi > lo { hi.ceil() - 1.0 } else { first };
    let top = f64::from(region.y0);
    let bottom = f64::from(region.y1 - 1);
    if last < top || first > bottom {
        return None;
    }
    Some((first.max(top) as u32, last.min(bottom) as u32))
}

/// The edges and curves of one shape listed per row they have area in,
/// each row in the shapes' own order: a row visits only the edges that
/// cross it, whatever the band height.
#[derive(Debug, Default)]
pub(crate) struct RowList {
    first: u32,
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl RowList {
    /// Lists every item `i` in each row of `ranges(i)` (inclusive) within
    /// rows `first..end`.
    pub(crate) fn new(
        (first, end): (u32, u32),
        count: usize,
        ranges: impl Fn(usize) -> Option<(u32, u32)>,
    ) -> Result<Self> {
        let span = end.saturating_sub(first) as usize;
        let mut list = Self {
            first,
            ..Self::default()
        };
        if u32::try_from(count).is_err() {
            return Err(Error::limit("row list exceeds u32"));
        }
        let allocation = |_| Error::limit("row list allocation failed");
        list.offsets
            .try_reserve_exact(span + 1)
            .map_err(allocation)?;
        list.offsets.resize(span + 1, 0);
        let clamp = |i: usize| {
            let (a, b) = ranges(i)?;
            let (a, b) = (a.max(first), b.min(end.checked_sub(1)?));
            (a <= b).then(|| ((a - first) as usize, (b - first) as usize))
        };
        let mut total = 0_u32;
        for i in 0..count {
            if let Some((a, b)) = clamp(i) {
                for row in a..=b {
                    list.offsets[row + 1] += 1;
                }
                total = u32::try_from(b - a + 1)
                    .ok()
                    .and_then(|n| total.checked_add(n))
                    .ok_or(Error::limit("row list exceeds u32"))?;
            }
        }
        for row in 0..span {
            list.offsets[row + 1] += list.offsets[row];
        }
        list.items
            .try_reserve_exact(total as usize)
            .map_err(allocation)?;
        list.items.resize(total as usize, 0);
        // Fill each row from its start, which leaves every start at the
        // next row's; shifting restores them.
        for i in 0..count {
            if let Some((a, b)) = clamp(i) {
                for row in a..=b {
                    let slot = &mut list.offsets[row];
                    // Checked above: the item count fits u32.
                    list.items[*slot as usize] = u32::try_from(i).unwrap_or(u32::MAX);
                    *slot += 1;
                }
            }
        }
        if span > 0 {
            list.offsets.copy_within(0..span, 1);
            list.offsets[0] = 0;
        }
        Ok(list)
    }

    /// The items listed for row `y`.
    pub(crate) fn row(&self, y: u32) -> &[u32] {
        let Some(local) = y.checked_sub(self.first) else {
            return &[];
        };
        let local = local as usize;
        match (self.offsets.get(local), self.offsets.get(local + 1)) {
            (Some(&start), Some(&end)) => &self.items[start as usize..end as usize],
            _ => &[],
        }
    }

    /// Row visits over all rows.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        (self.offsets.capacity() + self.items.capacity()).saturating_mul(size_of::<u32>())
    }
}

/// An upper bound of the cells a straight edge visits in rows
/// `first..=last` of a row `width` cells wide: one per row for its cover,
/// plus the columns it crosses within the row.
pub(crate) fn line_work(line: Line, (first, last): (u32, u32), width: usize) -> usize {
    let rows = (last - first) as usize + 1;
    let dx = (line.b.x - line.a.x).abs().min(1e15);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded nonnegative width"
    )]
    let columns = (dx.ceil() as usize).min(rows.saturating_mul(width));
    rows.saturating_mul(4)
        .saturating_add(columns)
        .saturating_add(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    #[test]
    fn line_paths_skip_flattening_with_its_contours() {
        let limits = Limits::default();
        let t = Transform::new(1.5, 0.25, -0.5, 2.0, 3.0, -1.0);
        let p = |x: f64, y: f64| Point::new(x, y);
        let mut paths = Vec::new();
        let mut b = PathBuilder::new();
        b.move_to(p(0.0, 0.0));
        b.line_to(p(4.0, 0.0)).unwrap();
        b.line_to(p(4.0, 3.0)).unwrap();
        b.close();
        // A line after a close reopens at the start; a lone move is a point.
        b.line_to(p(-2.0, 5.0)).unwrap();
        b.line_to(p(1.0, 6.0)).unwrap();
        b.move_to(p(9.0, 9.0));
        b.move_to(p(7.0, 1.0));
        b.line_to(p(7.0, 1.0)).unwrap();
        b.close();
        b.move_to(p(2.0, 2.0));
        b.line_to(p(5.0, 2.0)).unwrap();
        b.line_to(p(5.0, 2.0)).unwrap();
        b.line_to(p(2.0, 7.0)).unwrap();
        b.move_to(p(8.0, 8.0));
        paths.push(b.finish());
        let mut b = PathBuilder::new();
        b.rect(1.0, 2.0, 3.0, 4.0);
        paths.push(b.finish());
        for path in &paths {
            let fast = Polygon::from_lines(path, t, &limits).unwrap().unwrap();
            let slow = Polygon::from_path(path, t, &limits).unwrap();
            assert_eq!(fast.points, slow.points);
            assert_eq!(fast.starts, slow.starts);
            assert_eq!(fast.points_cells, slow.points_cells);
        }
    }

    #[test]
    fn shared_edges_cancel_into_the_union_boundary() {
        let square = |x: f64| {
            [
                Point::new(x, 0.0),
                Point::new(x + 1.0, 0.0),
                Point::new(x + 1.0, 1.0),
                Point::new(x, 1.0),
            ]
        };
        let mut polygon = Polygon::default();
        for x in [0.0, 1.0] {
            polygon.starts.push(polygon.points.len());
            polygon.points.extend(square(x));
        }
        assert!(polygon.cancel_shared().unwrap());
        assert_eq!(polygon.starts, [0]);
        assert_eq!(polygon.points.len(), 6);
        assert!(crate::band::certify::certified(&polygon, 0.0));
        // Nothing shared: unchanged.
        let before = polygon.points.clone();
        assert!(!polygon.cancel_shared().unwrap());
        assert_eq!(polygon.points, before);
    }

    #[test]
    fn chains_follow_vertical_direction_and_wrap_at_the_seam() {
        let mut polygon = Polygon::default();
        let limits = Limits::default();
        polygon.begin(&limits, 4).unwrap();
        polygon.points.extend([
            Point::new(0.0, 1.0),
            Point::new(1.0, 0.0),
            Point::new(2.0, 1.0),
            Point::new(1.0, 2.0),
        ]);
        polygon.end();
        let mut next = 0;
        let mut out = Vec::new();
        chained(&polygon, 0, &mut next, &mut out).unwrap();
        let chains: Vec<_> = out.iter().map(|e| e.chain).collect();
        // Up, down, down, up: the two upward edges form one chain across
        // the seam.
        assert_eq!(chains[0], chains[3]);
        assert_eq!(chains[1], chains[2]);
        assert_ne!(chains[0], chains[1]);
    }

    #[test]
    fn rectangle_clipping_projects_outside_parts_onto_the_sides() {
        let bounds = Bounds {
            left: 1.0,
            top: 0.0,
            right: 3.0,
            bottom: 4.0,
        };
        let edge = Chained {
            line: Line {
                a: Point::new(0.0, -1.0),
                b: Point::new(4.0, 7.0),
            },
            chain: 1,
            group: 0,
            enclosed: false,
            place: Place::default(),
        };
        let mut out = Vec::new();
        clip(edge, bounds, &mut out).unwrap();
        let lines: Vec<_> = out.iter().map(|e| e.line).collect();
        assert_eq!(
            lines,
            [
                Line {
                    a: Point::new(1.0, 0.0),
                    b: Point::new(1.0, 1.0)
                },
                Line {
                    a: Point::new(1.0, 1.0),
                    b: Point::new(2.5, 4.0)
                },
            ]
        );
    }

    #[test]
    fn polygons_drop_repeats_and_keep_point_cells() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1.5, 2.5));
        path.close();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(0.0, 0.0)).unwrap();
        path.line_to(Point::new(2.0, 0.0)).unwrap();
        path.line_to(Point::new(2.0, 2.0)).unwrap();
        path.line_to(Point::new(0.0, 0.0)).unwrap();
        let polygon =
            Polygon::from_path(&path.finish(), Transform::IDENTITY, &Limits::default()).unwrap();
        let contours: Vec<_> = polygon.contours().collect();
        assert_eq!(contours.len(), 2);
        assert_eq!(contours[0][0], Point::new(1.0, 2.0));
        assert_eq!(contours[1].len(), 3);
        assert!(polygon.points_cells);
    }
}
