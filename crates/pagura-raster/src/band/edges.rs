//! Prepared device geometry: closed polygons, their straight edges and
//! monotone chains, clipping to a rectangle, and the index of edges by band.
//!
//! Edges keep the coordinates they were built with. Bands and the requested
//! region only select which edges and rows are visited, never which
//! arithmetic produces a cell, so coverage is independent of both.

use super::DeviceRect;
use super::cells::STRIP;
use crate::flatten::{CurveStack, Polyline, bounded, flatten_into};
use crate::geometry::{greater, lesser};
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Meter, Result};

/// The largest coordinate magnitude, in device pixels, at which the cell
/// kernel interpolates edges in plain f64 (2^16).
pub(crate) const PLAIN_LIMIT: f64 = 65_536.0;

/// The smallest edge height the cell kernel interpolates in plain f64:
/// shorter heights could overflow the slope.
pub(crate) const PLAIN_HEIGHT: f64 = 1.0 / 1_073_741_824.0;

/// A straight edge in device space, in contour order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Line {
    pub(crate) a: Point,
    pub(crate) b: Point,
}

/// `x` clamped to the closed interval between `a` and `b`.
#[inline]
fn clamp_between(x: f64, a: f64, b: f64) -> f64 {
    lesser(greater(x, lesser(a, b)), greater(a, b))
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
            clamp_between(x, top.x, bottom.x)
        }
    }

    /// Whether the cell kernel interpolates the edge plainly: every
    /// coordinate is within [`PLAIN_LIMIT`] and the height is at least
    /// [`PLAIN_HEIGHT`]. Plain f64 interpolation from the upper endpoint,
    /// [`Self::plain_x`], then stays within [`Self::interpolation_error`] of
    /// the exact value (about 1e-12 of a pixel at ordinary device
    /// coordinates); elsewhere the kernel takes [`Self::x`], whose rounding
    /// tails keep distant endpoints exact.
    #[inline]
    pub(crate) fn plain(self) -> bool {
        let (top, bottom) = if self.a.y < self.b.y {
            (self.a, self.b)
        } else {
            (self.b, self.a)
        };
        top.x.abs() <= PLAIN_LIMIT
            && bottom.x.abs() <= PLAIN_LIMIT
            && top.y.abs() <= PLAIN_LIMIT
            && bottom.y.abs() <= PLAIN_LIMIT
            && bottom.y - top.y >= PLAIN_HEIGHT
    }

    /// The x at height `y` of a [`Self::plain`] edge, as the cell kernel
    /// interpolates it.
    #[inline]
    pub(crate) fn plain_x(self, y: f64) -> f64 {
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
            clamp_between(
                top.x + (y - top.y) * ((bottom.x - top.x) / (bottom.y - top.y)),
                top.x,
                bottom.x,
            )
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

    /// Outward coordinate uncertainty of interpolation and crossing
    /// evaluation on this stored line. For an interior parameter, each
    /// subtraction has relative error at most EPS, division adds one,
    /// and multiplication/addition contribute at most six EPS times the
    /// endpoint magnitudes; sixteen covers their products and rounding.
    /// Axis-aligned boundaries need no such interpolation.
    pub(crate) fn interpolation_error(self) -> Point {
        if self.a.x == self.b.x || self.a.y == self.b.y {
            return Point::default();
        }
        let bound = |a: f64, b: f64| 16.0 * f64::EPSILON * (1.0 + a.abs() + b.abs());
        Point::new(bound(self.a.x, self.b.x), bound(self.a.y, self.b.y))
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

/// Outline polygons mapped to device space as a stroker makes them: the
/// polygon [`Polygon::from_lines`] makes of the path they would form under
/// `transform`, built without the path.
pub(crate) struct Mapped {
    polygon: Polygon,
    transform: Transform,
    /// The segments of that path.
    segments: usize,
}

impl Mapped {
    pub(crate) fn new(transform: Transform) -> Self {
        Self {
            polygon: Polygon::default(),
            transform,
            segments: 0,
        }
    }

    pub(crate) fn finish(self) -> Polygon {
        self.polygon
    }
}

impl crate::stroke::Outline for Mapped {
    fn len(&self) -> usize {
        self.segments
    }

    fn heap_bytes(&self) -> usize {
        self.polygon.heap_bytes()
    }

    /// One closed contour, as [`Polygon::from_lines`] takes it: points that
    /// all coincide enclose their floor cell, other points map one by one.
    fn add_polygon(
        &mut self,
        points: &[Point],
        reversed: bool,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<()> {
        let Some(&first) = points.first() else {
            return Ok(());
        };
        self.segments = self.segments.saturating_add(points.len() + 1);
        if points.iter().all(|&p| p == first) {
            let p = first * self.transform;
            bounded(p)?;
            return self.polygon.cell(p, limits, bytes);
        }
        self.polygon.begin(limits, points.len(), bytes)?;
        let mut map = |point: Point| -> Result<()> {
            let p = point * self.transform;
            bounded(p)?;
            self.polygon.points.push(p);
            Ok(())
        };
        if reversed {
            points.iter().rev().try_for_each(|&p| map(p))?;
        } else {
            points.iter().try_for_each(|&p| map(p))?;
        }
        self.polygon.end();
        Ok(())
    }
}

/// The points of a path of lines' longest contour, its points in all and its
/// contours, as [`Polygon::from_lines`] collects them: a line after a close
/// reopens a contour at the subpath start.
fn contour_sizes(path: &Path) -> (usize, usize, usize) {
    let (mut longest, mut points, mut contours) = (0, 0, 0);
    let (mut run, mut closed) = (0_usize, false);
    for segment in path.segments() {
        match segment {
            Segment::MoveTo(_) => {
                longest = longest.max(run);
                (run, closed) = (1, false);
                contours += 1;
                points += 1;
            }
            Segment::LineTo(_) => {
                if closed {
                    longest = longest.max(run);
                    (run, closed) = (1, false);
                    contours += 1;
                    points += 1;
                }
                run += 1;
                points += 1;
            }
            Segment::Close => closed = true,
            Segment::CubicTo { .. } | Segment::QuadTo { .. } => {}
        }
    }
    (longest.max(run), points, contours)
}

impl Polygon {
    pub(crate) fn contours(&self) -> impl Iterator<Item = &[Point]> {
        self.starts.iter().enumerate().map(|(i, &start)| {
            let end = self.starts.get(i + 1).copied().unwrap_or(self.points.len());
            &self.points[start..end]
        })
    }

    /// Every contour side in source order, including horizontal sides.
    pub(crate) fn sides(&self) -> impl Iterator<Item = Line> {
        self.contours().flat_map(|contour| {
            let closing = contour.last().zip(contour.first());
            contour
                .windows(2)
                .map(|side| (&side[0], &side[1]))
                .chain(closing)
                .map(|(&a, &b)| Line { a, b })
        })
    }

    /// Source-ordered sides with keys that recover both endpoints in O(1).
    /// Closing sides use the contour ordinal after the vertex key range.
    pub(crate) fn indexed_sides(&self) -> impl Iterator<Item = (usize, Line)> + Clone {
        self.starts
            .iter()
            .enumerate()
            .flat_map(move |(contour, &start)| {
                let end = self
                    .starts
                    .get(contour + 1)
                    .copied()
                    .unwrap_or(self.points.len());
                (start..end).map(move |i| {
                    let (key, next) = if i + 1 == end {
                        (self.points.len().saturating_add(contour), start)
                    } else {
                        (i, i + 1)
                    };
                    (
                        key,
                        Line {
                            a: self.points[i],
                            b: self.points[next],
                        },
                    )
                })
            })
    }

    pub(crate) fn side(&self, key: usize) -> Option<Line> {
        let (a, b) = if key < self.points.len() {
            (key, key.checked_add(1)?)
        } else {
            let contour = key - self.points.len();
            let start = *self.starts.get(contour)?;
            let end = self
                .starts
                .get(contour.checked_add(1)?)
                .copied()
                .unwrap_or(self.points.len());
            (end.checked_sub(1)?, start)
        };
        Some(Line {
            a: *self.points.get(a)?,
            b: *self.points.get(b)?,
        })
    }

    pub(crate) fn vertices(&self) -> usize {
        self.points.len()
    }

    /// The fill polygon of `path` under `transform`, flattened within the
    /// exact tolerance (1/256 pixel).
    pub(crate) fn from_path(
        path: &Path,
        transform: Transform,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let before = bytes.used();
        let mut contours: Vec<Polyline> = Vec::new();
        let mut curves = CurveStack::new();
        let count = flatten_into(path, transform, limits, &mut contours, &mut curves, bytes)?;
        let mut polygon = Self::default();
        polygon.extend(&contours[..count], transform, limits, bytes)?;
        drop(contours);
        drop(curves);
        bytes.refund((bytes.used() - before).saturating_sub(polygon.heap_bytes()));
        Ok(polygon)
    }

    /// [`Self::from_path`] for a path of lines only, without flattening:
    /// the same contours, points and errors. `None` for a path with curves.
    pub(crate) fn from_lines(
        path: &Path,
        transform: Transform,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<Option<Self>> {
        if path
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. } | Segment::QuadTo { .. }))
        {
            return Ok(None);
        }
        crate::flatten::validate(path, transform, limits)?;
        let mut polygon = Self::default();
        let mut count = 0_usize;
        // The current contour's user points, untransformed until the
        // contour ends, room for the longest contour reserved at once.
        let mut user: Vec<Point> = Vec::new();
        let (longest, points, contours) = contour_sizes(path);
        bytes.reserve(&mut user, longest)?;
        bytes.reserve(&mut polygon.points, points)?;
        bytes.reserve(&mut polygon.starts, contours)?;
        let mut closed = false;
        let finish = |polygon: &mut Self,
                      user: &mut Vec<Point>,
                      last: bool,
                      closed: bool,
                      bytes: &mut Meter|
         -> Result<()> {
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
                polygon.cell(p, limits, bytes)?;
            } else {
                polygon.begin(limits, user.len(), bytes)?;
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
        let mut append = |user: &mut Vec<Point>, point: Point, bytes: &mut Meter| -> Result<()> {
            if count >= limits.path_segments {
                return Err(Error::limit("flattened segment budget exceeded"));
            }
            count += 1;
            bytes.reserve_capacity(user, 1)?;
            user.push(point);
            Ok(())
        };
        for segment in path.segments() {
            match *segment {
                Segment::MoveTo(point) => {
                    finish(&mut polygon, &mut user, false, closed, bytes)?;
                    bounded(point)?;
                    bounded(point * transform)?;
                    append(&mut user, point, bytes)?;
                    closed = false;
                }
                Segment::LineTo(point) => {
                    if closed {
                        // A line after a close reopens at the subpath start.
                        let first = user.first().copied();
                        finish(&mut polygon, &mut user, false, closed, bytes)?;
                        if let Some(first) = first {
                            bytes.reserve_capacity(&mut user, 1)?;
                            user.push(first);
                        }
                        closed = false;
                    }
                    bounded(point)?;
                    bounded(point * transform)?;
                    append(&mut user, point, bytes)?;
                }
                Segment::Close => closed = true,
                Segment::CubicTo { .. } | Segment::QuadTo { .. } => return Ok(None),
            }
        }
        finish(&mut polygon, &mut user, true, closed, bytes)?;
        let scratch = user.capacity().saturating_mul(size_of::<Point>());
        drop(user);
        bytes.refund(scratch);
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
        bytes: &mut Meter,
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
                self.cell(p, limits, bytes)?;
                continue;
            }
            self.begin(limits, contour.points.len(), bytes)?;
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
    pub(crate) fn cell(&mut self, p: Point, limits: &Limits, bytes: &mut Meter) -> Result<()> {
        self.begin(limits, 4, bytes)?;
        self.points.extend(Self::cell_corners(p));
        self.points_cells = true;
        self.end();
        Ok(())
    }

    /// Corners of the floor cell, in the contour's accumulation order.
    pub(crate) fn cell_corners(p: Point) -> [Point; 4] {
        let (x, y) = (p.x.floor(), p.y.floor());
        [
            Point::new(x, y),
            Point::new(x + 1.0, y),
            Point::new(x + 1.0, y + 1.0),
            Point::new(x, y + 1.0),
        ]
    }

    pub(crate) fn begin(&mut self, limits: &Limits, count: usize, bytes: &mut Meter) -> Result<()> {
        if self
            .points
            .len()
            .checked_add(count)
            .is_none_or(|n| n > limits.path_segments)
        {
            return Err(Error::limit("flattened segment budget exceeded"));
        }
        bytes.reserve_capacity(&mut self.points, count)?;
        bytes.reserve_capacity(&mut self.starts, 1)?;
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
    pub(crate) fn cancel_shared(&mut self, bytes: &mut Meter) -> Result<bool> {
        let before = bytes.used();
        let result = self.cancelled(bytes);
        let retained = result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map_or(0, Self::heap_bytes);
        bytes.refund((bytes.used() - before).saturating_sub(retained));
        if let Some(polygon) = result? {
            let old = self.heap_bytes();
            *self = polygon;
            bytes.refund(old);
            return Ok(true);
        }
        Ok(false)
    }

    fn cancelled(&self, bytes: &mut Meter) -> Result<Option<Self>> {
        let n = self.points.len();
        // Points ordered by their coordinates' total order, as integers,
        // and each side as its first point's index and the next one's.
        let mut keys: Vec<(i64, i64)> = Vec::new();
        bytes.reserve(&mut keys, n)?;
        keys.extend(self.points.iter().map(|p| (ordered(p.x), ordered(p.y))));
        let mut next: Vec<usize> = Vec::new();
        bytes.reserve(&mut next, n)?;
        for (contour, &start) in self.starts.iter().enumerate() {
            let end = self.starts.get(contour + 1).copied().unwrap_or(n);
            next.extend(start + 1..end);
            next.push(start);
        }
        let mut kept = uncancelled(&keys, &next, bytes)?;
        if kept.len() == n {
            return Ok(None);
        }
        // Relink by start point: any pairing at a shared vertex keeps the
        // winding, but which one is taken shapes the contours.
        kept.sort_unstable();
        if kept.windows(2).any(|w| w[0].0 == w[1].0) {
            shared_order(&mut kept, (&keys, &next, &self.points), bytes)?;
        }
        let firsts = Firsts::new(&kept, bytes)?;
        let mut used: Vec<bool> = Vec::new();
        bytes.reserve(&mut used, kept.len())?;
        used.resize(kept.len(), false);
        let mut points = Vec::new();
        let mut starts = Vec::new();
        bytes.reserve(&mut points, kept.len())?;
        for first in 0..kept.len() {
            if used[first] {
                continue;
            }
            bytes.reserve_capacity(&mut starts, 1)?;
            starts.push(points.len());
            let origin = kept[first].0;
            let mut current = first;
            loop {
                used[current] = true;
                let a = kept[current].1;
                points.push(self.points[a]);
                let end = keys[next[a]];
                if end == origin {
                    break;
                }
                // The first unused side that starts at the end continues
                // the contour.
                let Some(found) = (firsts.get(&kept, end)..kept.len())
                    .take_while(|&i| kept[i].0 == end)
                    .find(|&i| !used[i])
                else {
                    return Ok(None);
                };
                current = found;
            }
        }
        Ok(Some(Self {
            points,
            starts,
            points_cells: self.points_cells,
        }))
    }

    /// Heap storage of this owned polygon.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.points
            .capacity()
            .saturating_mul(size_of::<Point>())
            .saturating_add(self.starts.capacity().saturating_mul(size_of::<usize>()))
    }

    /// The device bounds of the vertices, as `[min, max]`.
    pub(crate) fn bounds(&self) -> Option<[Point; 2]> {
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in &self.points {
            lo.x = lesser(lo.x, p.x);
            lo.y = lesser(lo.y, p.y);
            hi.x = greater(hi.x, p.x);
            hi.y = greater(hi.y, p.y);
        }
        (lo.x <= hi.x && lo.y <= hi.y).then_some([lo, hi])
    }
}

/// The sides, as their start keys and first points' indices, that no side of
/// opposite direction cancels: of the sides with one undirected key, as many
/// reversed as forward ones cancel in pairs.
fn uncancelled(
    keys: &[(i64, i64)],
    next: &[usize],
    bytes: &mut Meter,
) -> Result<Vec<((i64, i64), usize)>> {
    let undirected = |a: usize| {
        let (from, to) = (keys[a], keys[next[a]]);
        (from.min(to), from.max(to), from <= to)
    };
    // Sides with equal undirected keys have equal fingerprints, so ordering
    // by fingerprint brings each key's sides together. The index takes as
    // few low bits as the sides need.
    let n = keys.len();
    let low = n
        .checked_next_power_of_two()
        .and_then(|power| u64::try_from(power - 1).ok())
        .unwrap_or(u64::MAX);
    let index = |o: u64| usize::try_from(o & low).unwrap_or(usize::MAX);
    let mut order: Vec<u64> = Vec::new();
    bytes.reserve(&mut order, n)?;
    for a in 0..n {
        let (lo, hi, _) = undirected(a);
        order.push(fingerprint(lo, hi) & !low | u64::try_from(a).unwrap_or(low));
    }
    order.sort_unstable();
    let mut kept = Vec::new();
    bytes.reserve(&mut kept, n)?;
    let mut run: Vec<(Undirected, usize)> = Vec::new();
    for same in order.chunk_by(|o, p| o & !low == p & !low) {
        if let [o] = *same {
            kept.push((keys[index(o)], index(o)));
            continue;
        }
        // Within a fingerprint, each side by its undirected key, the
        // reversed ones first.
        run.clear();
        bytes.reserve_capacity(&mut run, same.len())?;
        run.extend(same.iter().map(|&o| (undirected(index(o)), index(o))));
        run.sort_unstable();
        for group in run.chunk_by(|(e, _), (g, _)| (e.0, e.1) == (g.0, g.1)) {
            let reversed = group.iter().filter(|(e, _)| !e.2).count();
            let cancelled = reversed.min(group.len() - reversed);
            let rest = group[cancelled..reversed]
                .iter()
                .chain(&group[reversed + cancelled..]);
            kept.extend(rest.map(|&(_, a)| (keys[a], a)));
        }
    }
    Ok(kept)
}

/// The first place of each start key among sides ordered by start key, in
/// an open-addressed table of places. An empty table uses the sorted sides
/// directly when collisions exceed the probe bound.
struct Firsts {
    places: Vec<usize>,
    mask: usize,
}

/// Colliding keys fall back to the sorted sides instead of quadratic probing.
const RELINK_PROBES: usize = 16;

impl Firsts {
    fn new(sides: &[((i64, i64), usize)], bytes: &mut Meter) -> Result<Self> {
        let size = sides
            .len()
            .saturating_mul(2)
            .checked_next_power_of_two()
            .ok_or(Error::limit("relink table size overflows"))?;
        let mut places = Vec::new();
        bytes.reserve(&mut places, size)?;
        places.resize(size, usize::MAX);
        let mask = size - 1;
        for (place, side) in sides.iter().enumerate() {
            if place > 0 && sides[place - 1].0 == side.0 {
                continue;
            }
            let mut slot = slot(side.0) & mask;
            let mut probes = 0;
            while places[slot] != usize::MAX {
                probes += 1;
                if probes == RELINK_PROBES {
                    let released = places.capacity() * size_of::<usize>();
                    drop(places);
                    bytes.refund(released);
                    return Ok(Self {
                        places: Vec::new(),
                        mask: 0,
                    });
                }
                slot = (slot + 1) & mask;
            }
            places[slot] = place;
        }
        Ok(Self { places, mask })
    }

    /// The first place of the sides starting at `key`, or past them all.
    fn get(&self, sides: &[((i64, i64), usize)], key: (i64, i64)) -> usize {
        let mut slot = slot(key) & self.mask;
        for _ in 0..RELINK_PROBES {
            let Some(&place) = self.places.get(slot) else {
                break;
            };
            if place == usize::MAX {
                return sides.len();
            }
            if sides[place].0 == key {
                return place;
            }
            slot = (slot + 1) & self.mask;
        }
        let first = sides.partition_point(|&(from, _)| from < key);
        if sides.get(first).is_some_and(|&(from, _)| from == key) {
            first
        } else {
            sides.len()
        }
    }
}

/// A table slot for a point key.
#[expect(
    clippy::cast_sign_loss,
    reason = "bit reinterpretation of the integer key"
)]
fn slot(key: (i64, i64)) -> usize {
    let hash = (key.0 as u64 ^ (key.1 as u64).rotate_left(32)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    usize::try_from(hash >> 32).unwrap_or_default()
}

/// Orders the `kept` sides, by start key and index, that share a start as
/// sorting their keyed values by undirected key and then by start leaves
/// them; the sides elsewhere keep their places.
fn shared_order(
    kept: &mut [((i64, i64), usize)],
    (keys, next, points): (&[(i64, i64)], &[usize], &[Point]),
    bytes: &mut Meter,
) -> Result<()> {
    let mut links: Vec<Keyed> = Vec::new();
    bytes.reserve(&mut links, kept.len())?;
    links.extend(kept.iter().map(|&(from, a)| {
        let to = keys[next[a]];
        Keyed {
            lo: from.min(to),
            hi: from.max(to),
            forward: from <= to,
            from,
            to,
            a: points[a],
        }
    }));
    // Sides with equal undirected keys and directions are equal.
    links.sort_unstable_by_key(|e| (e.lo, e.hi, e.forward));
    links.sort_unstable_by_key(|e| e.from);
    let mut sides: Vec<usize> = Vec::new();
    bytes.reserve(&mut sides, kept.len())?;
    let mut first = 0;
    for same in kept.chunk_by_mut(|p, q| p.0 == q.0) {
        // The sorts moved sides only among those of one start.
        sides.clear();
        sides.extend(same.iter().map(|&(_, a)| a));
        if sides.len() > RELINK_PROBES {
            sides.sort_unstable_by_key(|&a| (keys[next[a]], a));
        }
        for (place, e) in same.iter_mut().zip(&links[first..]) {
            let found = if sides.len() > RELINK_PROBES {
                sides.get(sides.partition_point(|&a| keys[next[a]] < e.to))
            } else {
                sides.iter().find(|&&a| keys[next[a]] == e.to)
            };
            if let Some(&a) = found.filter(|&&a| keys[next[a]] == e.to) {
                place.1 = a;
            }
        }
        first += same.len();
    }
    Ok(())
}

/// A kept edge keyed for relinking: its ends' integer keys, undirected and
/// directed, and its first end. Edges that share a start take the order an
/// unstable sort of these values gives them, which depends on their size.
#[derive(Debug, Clone, Copy)]
struct Keyed {
    lo: (i64, i64),
    hi: (i64, i64),
    forward: bool,
    from: (i64, i64),
    to: (i64, i64),
    #[expect(dead_code, reason = "the sorts' tie order depends on the size")]
    a: Point,
}

/// A hash of an undirected edge key: equal keys have equal fingerprints.
#[expect(
    clippy::cast_sign_loss,
    reason = "bit reinterpretation of the integer keys"
)]
fn fingerprint(lo: (i64, i64), hi: (i64, i64)) -> u64 {
    [lo.0, lo.1, hi.0, hi.1].into_iter().fold(0, |hash, value| {
        (hash ^ value as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(29)
    })
}

/// An edge's undirected key and whether it runs from its lower end.
type Undirected = ((i64, i64), (i64, i64), bool);

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
pub(crate) fn lines(polygon: &Polygon, bytes: &mut Meter) -> Result<Vec<Line>> {
    let mut out = Vec::new();
    bytes.reserve(&mut out, polygon.vertices())?;
    out.extend(polygon.sides().filter(|line| !line.is_horizontal()));
    Ok(out)
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

/// The items of one shape listed per strip ([`STRIP`] rows of the global
/// grid) they have area in, each strip in the items' own order: a band
/// visits only the items that cross its strips, whatever its height.
#[derive(Debug, Default)]
pub(crate) struct StripList {
    first: u32,
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl StripList {
    /// Lists every item `i` in each strip that rows `ranges(i)` (inclusive)
    /// meet within rows `first..end`.
    pub(crate) fn new(
        (first, end): (u32, u32),
        count: usize,
        ranges: impl Iterator<Item = (usize, (u32, u32))> + Clone,
        bytes: &mut Meter,
    ) -> Result<Self> {
        if u32::try_from(count).is_err() {
            return Err(Error::limit("strip list exceeds u32"));
        }
        let last = end.checked_sub(1).filter(|&last| last >= first);
        let Some(last) = last else {
            return Ok(Self::default());
        };
        let strip0 = first / STRIP;
        let span = (last / STRIP - strip0) as usize + 1;
        let mut list = Self {
            first: strip0,
            ..Self::default()
        };
        bytes.reserve(&mut list.offsets, span + 1)?;
        list.offsets.resize(span + 1, 0);
        let clamp = |(a, b): (u32, u32)| {
            let (a, b) = (a.max(first), b.min(last));
            (a <= b).then(|| ((a / STRIP - strip0) as usize, (b / STRIP - strip0) as usize))
        };
        let mut total = 0_u32;
        for (i, range) in ranges.clone() {
            if i >= count {
                return Err(Error::limit("strip item exceeds its key range"));
            }
            if let Some((a, b)) = clamp(range) {
                for strip in a..=b {
                    list.offsets[strip + 1] += 1;
                }
                total = u32::try_from(b - a + 1)
                    .ok()
                    .and_then(|n| total.checked_add(n))
                    .ok_or(Error::limit("strip list exceeds u32"))?;
            }
        }
        for strip in 0..span {
            list.offsets[strip + 1] += list.offsets[strip];
        }
        bytes.reserve(&mut list.items, total as usize)?;
        list.items.resize(total as usize, 0);
        // Fill each strip from its start, which leaves every start at the
        // next strip's; shifting restores them.
        for (i, range) in ranges {
            if let Some((a, b)) = clamp(range) {
                for strip in a..=b {
                    let slot = &mut list.offsets[strip];
                    // Checked above: the item count fits u32.
                    list.items[*slot as usize] = u32::try_from(i).unwrap_or(u32::MAX);
                    *slot += 1;
                }
            }
        }
        list.offsets.copy_within(0..span, 1);
        list.offsets[0] = 0;
        Ok(list)
    }

    /// The items listed for strip `strip`.
    pub(crate) fn strip(&self, strip: u32) -> &[u32] {
        let Some(local) = strip.checked_sub(self.first) else {
            return &[];
        };
        let local = local as usize;
        match (self.offsets.get(local), self.offsets.get(local + 1)) {
            (Some(&start), Some(&end)) => {
                self.items.get(start as usize..end as usize).unwrap_or(&[])
            }
            _ => &[],
        }
    }

    /// Strip visits over all strips.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        (self.offsets.capacity() + self.items.capacity()).saturating_mul(size_of::<u32>())
    }
}

/// The strips rows `first..=last` meet.
pub(crate) fn strips((first, last): (u32, u32)) -> usize {
    (last / STRIP - first / STRIP) as usize + 1
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
mod tests;
