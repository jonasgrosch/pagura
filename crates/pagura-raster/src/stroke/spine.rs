//! What a stroke's own geometry proves covered, so that the row resolver
//! need not sweep there.
//!
//! Let C be a run's centreline as the stroker strokes it, r its pen's radius
//! and U the union of its components: each segment's quad, each joint's join
//! polygon and each end's cap. A vertex covers a radius around it: a joint
//! the sector between its two outer normals within its join polygon, an end
//! the half disc beyond it within its cap. Round joins and caps cover r - tol
//! at least, their chords lying that far from their centre; drawn miters and
//! square caps cover r. Bevels, miters past their limit, undrawn reversals
//! and butt ends cover less: they are the run's low vertices.
//!
//! Every point p within ρ of C, ρ what the round elements cover less a
//! margin, lies in U unless it lies in the veto of a low vertex: the part of
//! the disc of radius ρ around it beyond its bevel's chord, or for a butt
//! end or an undrawn reversal, beyond the line through it across its
//! tangent. For let q be the point of C nearest p. Inside a segment, p - q
//! is normal to it and no longer than ρ ≤ r, so p lies in its quad. At a
//! joint, p - q lies in the cone between its outer normals: within the join
//! where the joint covers ρ, else in the veto, since a bevel holds all of
//! the cone short of its chord. At an end, p - q points beyond it: within a
//! round or square cap, else in the end's veto. The argument is global, q
//! anywhere on C, as it must be for a fan of segments shorter than ρ.
//!
//! So a cell lies in U where it lies within ρ of one of C's segments and
//! meets no veto, or within one segment's quad: both are convex, so it does
//! where its top and bottom edges lie within their chords there. Where the
//! pen is wide, runs of segments whose vertices lie within δ of one chord
//! count as that chord within ρ - δ: the centreline crosses each normal of
//! the chord within δ of it.
//!
//! The pieces the tiling emits have the components' union; held round caps
//! differ from it by slivers of rounding width, and the pieces' vertices
//! round by about 1e-12 px, far within the margin.

use super::tiling::{self, Joints};
use super::{LineCap, LineJoin, Pen, Run, Stroke, charge, conformal, mitred, ring_count};
use crate::band::Bounds;
use crate::{Point, Transform};
use pagura_core::{Limits, Meter, Result};

/// Device pixels the proof keeps clear of the pieces' rounding.
const MARGIN: f64 = 1.0 / 1024.0;

/// The device radius from which runs of short segments count as chords:
/// narrower pens have few segments reaching each row, and would lose cells.
const CHORDS: f64 = 4.0;

/// How far a chord may lie from the vertices it stands for, in radii.
const CHORD_SLACK: f64 = 1.0 / 16.0;

/// The most segments one chord stands for.
const CHORD_SEGMENTS: usize = 64;

/// The part of a mitred or bevelled pen its capsules leave out: bevels
/// whose chords lie within the rest cover what the capsules claim.
const BEVEL_SLACK: f64 = 1.0 / 64.0;

/// How far the veto regions reach past their exact extents, in pixels.
const SLACK: f64 = 1e-9;

/// A convex set within a stroke, in device space: the points within
/// `radius` of segment `a..b` where `round`, else its pen quad of that half
/// width; and the heights it spans.
#[derive(Debug, Clone, Copy)]
struct Capsule {
    a: Point,
    b: Point,
    radius: f64,
    round: bool,
    top: f64,
    bottom: f64,
    /// The segment's unit direction and length.
    unit: Point,
    length: f64,
}

/// Where a low vertex covers less than the capsules claim: the points
/// within `radius` of `centre` beyond the line where (p - centre)·normal is
/// `offset`; and the heights it spans.
#[derive(Debug, Clone, Copy)]
struct Veto {
    centre: Point,
    normal: Point,
    offset: f64,
    radius: f64,
    top: f64,
    bottom: f64,
}

/// The capsules and vetoes of a stroke's runs, each by the top of what it
/// spans.
#[derive(Debug, Default)]
pub(crate) struct Spine {
    capsules: Vec<Capsule>,
    vetoes: Vec<Veto>,
}

impl Spine {
    /// The spine of `runs` stroked with `pen` under the conformal
    /// `transform` to device space, leaving out the runs `convex` names
    /// (their pieces are their convex boundaries). `None` where the pen is
    /// too narrow to hold a cell or the transform is not conformal, and where
    /// the stroke's budgets cannot admit it: the rows are then resolved
    /// whole, as without one. Charged to `bytes` while it lives.
    pub(super) fn of(
        runs: &[Run],
        convex: impl Fn(usize) -> bool,
        (pen, transform): (Pen, Transform),
        stroke: &Stroke,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Option<Self> {
        let scale = transform.a.hypot(transform.b);
        if !conformal(transform) || !scale.is_finite() || (pen.radius * scale - MARGIN) <= 0.5 {
            return None;
        }
        let before = bytes.used();
        let mut spine = Self::default();
        let built = (0..runs.len())
            .filter(|&i| !convex(i))
            .try_fold(0, |work, i| {
                // The run's centreline and segments are dropped after it.
                let (mark, kept) = (bytes.used(), spine.heap_bytes());
                let added = spine.add(
                    &runs[i],
                    (pen, transform, scale),
                    stroke,
                    (limits, work),
                    bytes,
                );
                let grown = spine.heap_bytes().saturating_sub(kept);
                bytes.refund(bytes.used().saturating_sub(mark.saturating_add(grown)));
                added
            })
            .and_then(|work| spine.sort(limits, work));
        let kept = spine.heap_bytes();
        bytes.refund(bytes.used().saturating_sub(before.saturating_add(kept)));
        match built {
            Ok(()) if !spine.capsules.is_empty() => Some(spine),
            _ => {
                drop(spine);
                bytes.refund(kept);
                None
            }
        }
    }

    /// Adds the capsules and vetoes of `run`, with `work` units of the
    /// stroke's work budget spent so far; returns the units spent then.
    fn add(
        &mut self,
        run: &Run,
        (pen, transform, scale): (Pen, Transform, f64),
        stroke: &Stroke,
        (limits, mut work): (&Limits, usize),
        bytes: &mut Meter,
    ) -> Result<usize> {
        let Pen { radius, tolerance } = pen;
        let margin = MARGIN / scale;
        let device = |p: Point| p * transform;
        let points = tiling::centreline(run, bytes)?;
        charge(&mut work, points.len(), limits.raster_work)?;
        // A round cap's chords lie r cos(π / n) from its centre.
        let cap = match stroke.cap {
            LineCap::Round => {
                radius * (std::f64::consts::PI / f64::from(ring_count(pen, limits)?)).cos()
            }
            LineCap::Butt | LineCap::ProjectingSquare => radius,
        };
        let [_, _, ..] = points.as_slice() else {
            // A point's round caps are one circle.
            if let (Some(&point), LineCap::Round) = (points.first(), stroke.cap) {
                self.capsule(
                    device(point),
                    device(point),
                    ((cap - margin) * scale, true),
                    bytes,
                )?;
            }
            return Ok(work);
        };
        let open = !run.closed;
        let joints = Joints {
            closed: run.closed,
            segments: if open { points.len() - 1 } else { points.len() },
        };
        let segments = tiling::segments(&points, joints, bytes)?;
        // What every round element covers: the chords of round joins lie
        // r cos(step / 2) from their joint. Mitred and bevelled runs claim a
        // little less than their pen, so that the bevels of slight turns,
        // most of a curve's, are no low vertices.
        let mut base = if stroke.join == LineJoin::Round {
            radius * (1.0 - (tolerance / radius).min(1.0))
        } else {
            radius * (1.0 - BEVEL_SLACK)
        };
        if open && stroke.cap == LineCap::Round {
            base = base.min(cap);
        }
        let rho = base - margin;
        for i in 0..joints.segments {
            let (a, b) = (points[i], points[joints.after(i)]);
            if segments[i].1 * scale >= 1.0 {
                self.capsule(
                    device(a),
                    device(b),
                    ((radius - margin) * scale, false),
                    bytes,
                )?;
            }
        }
        if rho * scale <= 0.5 {
            return Ok(work);
        }
        let device = (transform, scale);
        self.low(
            (&points, &segments, joints),
            (pen, stroke),
            (rho, margin),
            device,
            bytes,
        )?;
        self.round((&points, joints), rho, device, (limits, work), bytes)
    }

    /// Adds the vetoes of the run with centreline `points`: its butt ends
    /// and the joints whose joins cover less than `rho`.
    fn low(
        &mut self,
        (points, segments, joints): (&[Point], &[(Point, f64)], Joints),
        (pen, stroke): (Pen, &Stroke),
        (rho, margin): (f64, f64),
        device: (Transform, f64),
        bytes: &mut Meter,
    ) -> Result<()> {
        let radius = pen.radius;
        if let (false, LineCap::Butt, [start, .., end]) = (joints.closed, stroke.cap, points) {
            let (first, last) = (segments[0].0, segments[segments.len() - 1].0);
            for (point, out) in [(*start, Point::new(-first.x, -first.y)), (*end, last)] {
                self.veto((point, out, 0.0), device, (rho, margin), bytes)?;
            }
        }
        if stroke.join == LineJoin::Round {
            return Ok(());
        }
        for (j, &point) in points.iter().enumerate() {
            let Some((before, after)) = joints.around(j) else {
                continue;
            };
            let (a, b) = (segments[before].0, segments[after].0);
            let cross = a.cross(b);
            if cross == 0.0 {
                // A reversal draws no join: nothing beyond the joint.
                if a.dot(b) < 0.0 {
                    self.veto((point, a, 0.0), device, (rho, margin), bytes)?;
                }
                continue;
            }
            let side = if cross > 0.0 { -radius } else { radius };
            let (n1, n2) = (a.perp() * side, b.perp() * side);
            if mitred((n1, n2), [a, b], radius, stroke).is_some() {
                continue;
            }
            // The bevel's chord, halfway out along its normals' sum.
            let sum = n1 + n2;
            let length = sum.length();
            let (normal, offset) = if length > 0.0 {
                (sum / length, length / 2.0)
            } else {
                (a, 0.0)
            };
            self.veto((point, normal, offset), device, (rho, margin), bytes)?;
        }
        Ok(())
    }

    /// Adds the capsules within `rho` of the run with centreline `points`:
    /// one per segment, or for a wide pen, chords standing for runs of them.
    /// Returns the work units spent after `work`.
    fn round(
        &mut self,
        (points, joints): (&[Point], Joints),
        rho: f64,
        (transform, scale): (Transform, f64),
        (limits, mut work): (&Limits, usize),
        bytes: &mut Meter,
    ) -> Result<usize> {
        let device = |p: Point| p * transform;
        // The centreline's vertices in order, a closed run's first again
        // at its end.
        let vertex = |k: usize| {
            points
                .get(k)
                .or(points.first())
                .copied()
                .unwrap_or_default()
        };
        let vertices = joints.segments + 1;
        if rho * scale < CHORDS {
            for i in 0..joints.segments {
                let (a, b) = (vertex(i), vertex(i + 1));
                self.capsule(device(a), device(b), (rho * scale, true), bytes)?;
            }
            return Ok(work);
        }
        // Chords of up to `CHORD_SEGMENTS` segments, each tested vertex
        // charged once per chord it may join.
        let slack = rho * CHORD_SLACK;
        let mut from = 0;
        while from + 1 < vertices {
            let a = vertex(from);
            let mut to = from + 1;
            while to + 1 < vertices && to - from < CHORD_SEGMENTS {
                charge(&mut work, to - from, limits.raster_work)?;
                let d = vertex(to + 1) - a;
                let length = d.length();
                let near = length > 0.0
                    && (from + 1..=to).all(|k| ((vertex(k) - a).cross(d) / length).abs() <= slack);
                if !near {
                    break;
                }
                to += 1;
            }
            self.capsule(
                device(a),
                device(vertex(to)),
                ((rho - slack) * scale, true),
                bytes,
            )?;
            from = to;
        }
        Ok(work)
    }

    /// Adds the capsule of segment `a..b` with `radius`, `round` or a quad.
    fn capsule(
        &mut self,
        a: Point,
        b: Point,
        (radius, round): (f64, bool),
        bytes: &mut Meter,
    ) -> Result<()> {
        if radius.is_nan() || radius <= 0.0 {
            return Ok(());
        }
        let length = (b - a).length();
        let unit = if length > 0.0 {
            (b - a) / length
        } else {
            Point::default()
        };
        bytes.reserve_capacity(&mut self.capsules, 1)?;
        self.capsules.push(Capsule {
            a,
            b,
            radius,
            round,
            top: a.y.min(b.y) - radius,
            bottom: a.y.max(b.y) + radius,
            unit,
            length,
        });
        Ok(())
    }

    /// Adds the veto at construction-space `point` beyond the line at
    /// `offset` along unit `normal`, within the capsules' radius `rho`, less
    /// `margin` toward the vertex: unless the line lies beyond that radius.
    fn veto(
        &mut self,
        (point, normal, offset): (Point, Point, f64),
        (transform, scale): (Transform, f64),
        (rho, margin): (f64, f64),
        bytes: &mut Meter,
    ) -> Result<()> {
        let offset = offset - margin;
        if offset >= rho {
            return Ok(());
        }
        let centre = point * transform;
        let towards = (point + normal) * transform - centre;
        let length = towards.length();
        if length.is_nan() || length <= 0.0 {
            return Ok(());
        }
        let mut veto = Veto {
            centre,
            normal: towards / length,
            offset: offset * scale,
            radius: rho * scale,
            top: f64::NEG_INFINITY,
            bottom: f64::INFINITY,
        };
        // The heights it spans: the circle's own, where beyond the line,
        // and where the line meets the circle.
        let mut ys = [veto.centre.y - veto.radius, veto.centre.y + veto.radius]
            .into_iter()
            .map(|y| Point::new(veto.centre.x, y))
            .chain(veto.crossings())
            .filter(|&p| veto.beyond(p))
            .map(|p| p.y);
        if let Some(first) = ys.next() {
            let (top, bottom) = ys.fold((first, first), |(t, b), y| (t.min(y), b.max(y)));
            (veto.top, veto.bottom) = (top - SLACK, bottom + SLACK);
        }
        bytes.reserve_capacity(&mut self.vetoes, 1)?;
        self.vetoes.push(veto);
        Ok(())
    }

    /// Orders the capsules and vetoes by their tops, within the stroke's
    /// work budget after `work` units.
    fn sort(&mut self, limits: &Limits, mut work: usize) -> Result<()> {
        let count = self.capsules.len().saturating_add(self.vetoes.len());
        charge(
            &mut work,
            count.saturating_mul(count.max(1).ilog2() as usize + 2),
            limits.raster_work,
        )?;
        self.capsules
            .sort_unstable_by(|p, q| p.top.total_cmp(&q.top));
        self.vetoes.sort_unstable_by(|p, q| p.top.total_cmp(&q.top));
        Ok(())
    }

    /// Bytes on the heap.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.capsules
            .capacity()
            .saturating_mul(size_of::<Capsule>())
            .saturating_add(self.vetoes.capacity().saturating_mul(size_of::<Veto>()))
    }

    /// The cells it proves covered, row by row from the top, within
    /// `within` where the shape's lines were clipped to it.
    pub(crate) fn cover(&self, within: Option<Bounds>) -> Cover<'_> {
        Cover {
            spine: self,
            within,
            capsules: 0,
            vetoes: 0,
            reaching: Vec::new(),
            vetoing: Vec::new(),
            round: Vec::new(),
            vetoed: Vec::new(),
            cells: Vec::new(),
        }
    }
}

impl Capsule {
    /// Its chord on the line at height `y`.
    fn chord(&self, y: f64) -> Option<(f64, f64)> {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        let r = self.radius;
        if self.round {
            for c in [self.a, self.b] {
                let dy = y - c.y;
                let h = r * r - dy * dy;
                if h >= 0.0 {
                    let w = h.sqrt();
                    (lo, hi) = (lo.min(c.x - w), hi.max(c.x + w));
                }
            }
        }
        let (u, length) = (self.unit, self.length);
        if length > 0.0 {
            // Within `r` of the segment's line and between its ends.
            let n = u.perp();
            let (mut l, mut h) = (f64::NEG_INFINITY, f64::INFINITY);
            for (k, e, (from, to)) in [
                (n.x, (y - self.a.y) * n.y, (-r, r)),
                (u.x, (y - self.a.y) * u.y, (0.0, length)),
            ] {
                if k == 0.0 {
                    if e < from || e > to {
                        (l, h) = (f64::INFINITY, f64::NEG_INFINITY);
                    }
                } else {
                    let (x0, x1) = (self.a.x + (from - e) / k, self.a.x + (to - e) / k);
                    (l, h) = (l.max(x0.min(x1)), h.min(x0.max(x1)));
                }
            }
            if l <= h {
                (lo, hi) = (lo.min(l), hi.max(h));
            }
        }
        (lo <= hi).then_some((lo, hi))
    }

    /// The cells of row `top..bottom` it holds, first and last column.
    fn cells(&self, (top, bottom): (f64, f64)) -> Option<(f64, f64)> {
        let (l0, r0) = self.chord(top)?;
        let (l1, r1) = self.chord(bottom)?;
        let (first, last) = (l0.max(l1).ceil(), r0.min(r1).floor() - 1.0);
        (first <= last).then_some((first, last))
    }
}

impl Veto {
    /// Whether `p` lies in the closed region, give or take [`SLACK`].
    fn beyond(&self, p: Point) -> bool {
        let d = p - self.centre;
        d.length() <= self.radius + SLACK && d.dot(self.normal) >= self.offset - SLACK
    }

    /// Where its line meets its circle.
    fn crossings(&self) -> impl Iterator<Item = Point> {
        let along = self.normal.perp();
        let foot = self.centre + self.normal * self.offset;
        let h = self.radius * self.radius - self.offset * self.offset;
        let w = h.max(0.0).sqrt();
        [foot + along * w, foot - along * w]
            .into_iter()
            .filter(move |_| h >= 0.0)
    }

    /// The extent in x of what it covers of row `top..bottom`: its extreme
    /// points lie where the circle reaches furthest, where the circle or
    /// the line meets a row line, or where the line meets the circle.
    fn extent(&self, (top, bottom): (f64, f64)) -> Option<(f64, f64)> {
        let Self {
            centre: c,
            normal,
            offset,
            radius,
            ..
        } = *self;
        let within = |p: &Point| self.beyond(*p) && p.y >= top - SLACK && p.y <= bottom + SLACK;
        let along = normal.perp();
        let foot = c + normal * offset;
        let rows = [top, bottom].into_iter().flat_map(move |y| {
            let h = radius * radius - (y - c.y) * (y - c.y);
            let w = h.max(0.0).sqrt();
            let circle = [Point::new(c.x - w, y), Point::new(c.x + w, y)]
                .into_iter()
                .filter(move |_| h >= 0.0);
            let line = (along.y != 0.0).then(|| foot + along * ((y - foot.y) / along.y));
            circle.chain(line)
        });
        let mut xs = [Point::new(c.x - radius, c.y), Point::new(c.x + radius, c.y)]
            .into_iter()
            .chain(self.crossings())
            .chain(rows)
            .filter(within)
            .map(|p| p.x);
        let first = xs.next()?;
        let (lo, hi) = xs.fold((first, first), |(l, h), x| (l.min(x), h.max(x)));
        Some((lo - SLACK * (1.0 + lo.abs()), hi + SLACK * (1.0 + hi.abs())))
    }
}

/// The cells a spine proves covered, row by row from the top: what it keeps
/// between rows, and its scratch.
#[derive(Debug)]
pub(crate) struct Cover<'a> {
    spine: &'a Spine,
    within: Option<Bounds>,
    /// The capsules and vetoes entered so far, and those still reaching.
    capsules: usize,
    vetoes: usize,
    reaching: Vec<u32>,
    vetoing: Vec<u32>,
    /// The current row's cells, as first and last columns: within ρ of the
    /// centreline, vetoed, and proven.
    round: Vec<(f64, f64)>,
    vetoed: Vec<(f64, f64)>,
    cells: Vec<(f64, f64)>,
}

impl Cover<'_> {
    /// Appends to `chunks` the column ranges of row `top..top + 1` that
    /// hold only proven cells, at most `most` of them: the runs of proven
    /// cells. Rows come from the top down.
    ///
    /// # Errors
    ///
    /// Returns a meter's `Limit` error: the reaching capsules and vetoes
    /// and the sorts of their cells are charged before each pass.
    pub(crate) fn row(
        &mut self,
        top: f64,
        most: usize,
        chunks: &mut Vec<(f64, f64)>,
        (work, bytes): (&mut Meter, &mut Meter),
    ) -> Result<()> {
        let bottom = top + 1.0;
        if self
            .within
            .is_some_and(|w| top < w.top || bottom > w.bottom)
        {
            return Ok(());
        }
        let Spine { capsules, vetoes } = self.spine;
        while let Some(c) = capsules.get(self.capsules)
            && c.top <= top
        {
            work.charge(1)?;
            bytes.reserve_capacity(&mut self.reaching, 1)?;
            self.reaching.push(index(self.capsules));
            self.capsules += 1;
        }
        while let Some(v) = vetoes.get(self.vetoes)
            && v.top <= bottom
        {
            work.charge(1)?;
            bytes.reserve_capacity(&mut self.vetoing, 1)?;
            self.vetoing.push(index(self.vetoes));
            self.vetoes += 1;
        }
        work.charge(self.reaching.len().saturating_add(self.vetoing.len()))?;
        self.reaching
            .retain(|&i| capsules[i as usize].bottom >= bottom);
        self.vetoing.retain(|&i| vetoes[i as usize].bottom >= top);
        let count = self.reaching.len().saturating_add(self.vetoing.len());
        work.charge_product(count.saturating_mul(count.max(1).ilog2() as usize + 2), 3)?;
        for v in [&mut self.round, &mut self.vetoed, &mut self.cells] {
            v.clear();
        }
        bytes.reserve_capacity(&mut self.round, self.reaching.len())?;
        // The quads' cells and the round ones cut by the vetoes, at most
        // one more each than the vetoes.
        bytes.reserve_capacity(&mut self.cells, count)?;
        bytes.reserve_capacity(&mut self.vetoed, self.vetoing.len())?;
        for &i in &self.reaching {
            let c = &capsules[i as usize];
            if let Some(cells) = c.cells((top, bottom)) {
                if c.round {
                    self.round.push(cells);
                } else {
                    self.cells.push(cells);
                }
            }
        }
        merge(&mut self.round);
        // Only vetoes reaching round cells take their extents.
        for &i in &self.vetoing {
            let veto = &vetoes[i as usize];
            let (lo, hi) = (veto.centre.x - veto.radius, veto.centre.x + veto.radius);
            let near = self.round.partition_point(|&(_, last)| last + 1.0 < lo);
            if self.round.get(near).is_none_or(|&(first, _)| first > hi) {
                continue;
            }
            if let Some((lo, hi)) = veto.extent((top, bottom)) {
                self.vetoed.push((lo.floor(), hi.ceil() - 1.0));
            }
        }
        merge(&mut self.vetoed);
        // The round cells less the vetoed ones, then the quads' cells.
        let mut vetoed = self.vetoed.iter().peekable();
        for &(first, last) in &self.round {
            let mut from = first;
            while let Some(&&(a, b)) = vetoed.peek() {
                if b < from {
                    vetoed.next();
                    continue;
                }
                if a > last {
                    break;
                }
                if a > from {
                    self.cells.push((from, a - 1.0));
                }
                from = b + 1.0;
                if b > last {
                    break;
                }
                vetoed.next();
            }
            if from <= last {
                self.cells.push((from, last));
            }
        }
        merge(&mut self.cells);
        let (left, right) = self.within.map_or((f64::NEG_INFINITY, f64::INFINITY), |w| {
            (w.left.ceil(), w.right.floor() - 1.0)
        });
        for &(first, last) in &self.cells {
            let (first, last) = (first.max(left), last.min(right));
            if chunks.len() < most && last >= first {
                bytes.reserve_capacity(chunks, 1)?;
                chunks.push((first, last + 1.0));
            }
        }
        Ok(())
    }
}

/// Sorts runs of cells by their first column and merges those that touch.
fn merge(cells: &mut Vec<(f64, f64)>) {
    cells.sort_unstable_by(|p, q| p.0.total_cmp(&q.0));
    let mut kept = 0;
    for i in 0..cells.len() {
        let (first, last) = cells[i];
        if kept > 0 && first <= cells[kept - 1].1 + 1.0 {
            cells[kept - 1].1 = cells[kept - 1].1.max(last);
        } else {
            cells[kept] = (first, last);
            kept += 1;
        }
    }
    cells.truncate(kept);
}

/// A capsule's or veto's index, which the stroke's outline budget keeps
/// within u32.
fn index(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
