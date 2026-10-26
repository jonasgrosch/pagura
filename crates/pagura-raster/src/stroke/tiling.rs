//! Stroke pieces that meet without overlapping, for signed-area coverage.
//!
//! The component outline of [`super::draw`] overlaps itself inside every turn
//! and under every round cap, which only an exact union integration resolves.
//! Here each segment's quadrilateral ends, on the inner side of a turn, at the
//! joint's inner miter point (on the bisector of the turn) and, on the outer
//! side, at the segment's own normal. The outer wedge between two normals
//! takes the join. Round caps keep the component stroker's circle polygon:
//! its angular grid is part of the canonical outline. A circle overlaps its
//! end piece, so an open round-capped run is proven only where each end
//! segment holds its circle behind the end line: the end piece then takes
//! the part of the circle beyond its end, and the circle is not drawn. A
//! point's two round caps are one circle, and so are the round caps of run
//! ends that meet at one point: their circles are the same polygon, which
//! an end holding its cap there covers already.
//! Pieces that meet share their vertices bit for bit. Their nonzero filled
//! set is the components'; where proven, every point is covered once. A
//! reversal, or a turn tighter than the pen around a short segment, takes the components'
//! form instead, which overlaps inside the turn; such runs, and runs that
//! could bring the pen back over itself (an open centreline whose directions
//! span a half turn, a closed one that is not convex, and runs whose pens
//! may meet), are not proven to tile: their union is still the stroke, and
//! the rasterizer certifies or resolves its windings.
//! The pieces are not emitted one by one: their outline is, every side two
//! of them share left out, which keeps their nonzero winding everywhere.

use super::{Outline, Pen, Run, cap, circle, join_into, offsets, oriented};
use crate::{LineCap, LineJoin, Point, Stroke};
use pagura_core::{Error, Limits, Meter, Result};

/// The inner side of one turn: the miter point's offset from the joint, the
/// side it lies on, how far it reaches along each segment, and how far the
/// segment before reaches along the segment after (and back): the pen radius
/// times the sine of the turn.
#[derive(Clone, Copy)]
struct Turn {
    offset: Point,
    left: bool,
    reach: f64,
    span: f64,
}

/// How the pieces of a run meet at one of its points.
#[derive(Clone, Copy)]
enum Joint {
    /// An end, or no turn: the pieces end at the normal.
    Straight,
    /// A turn whose inner miter point lies within both segments.
    Turn(Turn),
    /// A reversal, or a turn tighter than the pen around a short segment:
    /// the pieces end at their normals and overlap inside the turn, as the
    /// components do.
    Overlap,
}

impl Joint {
    fn turn(self) -> Option<Turn> {
        match self {
            Self::Turn(turn) => Some(turn),
            Self::Straight | Self::Overlap => None,
        }
    }
}

/// Appends the outline of the pieces of `run` to `out`, whose union is the
/// components'; end pieces that take their round caps differ from it only
/// by slivers of rounding width (ADR 0004 §4). Returns whether no two
/// pieces overlap: no joint overlaps and the centreline is proven not to
/// return within the pen of itself.
pub(super) fn append(
    out: &mut impl Outline,
    run: &Run,
    (pen, stroke): (Pen, &Stroke),
    chains: &mut Chains,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<bool> {
    // The tiling certificate uses chord-normal end lines. A source curve's
    // tangent may cut the first/last piece differently; retain its component
    // union until a certificate covers that geometry too.
    if run.caps.iter().any(Option::is_some) || !run.joins.is_empty() {
        super::draw(out, run, pen, stroke, limits, bytes)?;
        return Ok(false);
    }
    // The chains stay charged, as the output does, for the runs after.
    let before = bytes
        .used()
        .saturating_sub(out.heap_bytes().saturating_add(chains.heap_bytes()));
    let result = (|| {
        let Pen { radius, .. } = pen;
        let points = &run.points;
        let [first, .., last] = points.as_slice() else {
            // A point's two caps; two round caps are the same circle.
            if let Some(&point) = points.first() {
                let tangent = run.tangent;
                let back = Point::new(-tangent.x, -tangent.y);
                if stroke.cap != LineCap::Round || chains.circle(point, bytes)? {
                    cap(out, point, tangent, pen, stroke.cap, limits, bytes)?;
                }
                if stroke.cap != LineCap::Round {
                    cap(out, point, back, pen, stroke.cap, limits, bytes)?;
                }
            }
            return Ok(true);
        };
        let joints = Joints {
            closed: run.closed,
            segments: if run.closed {
                points.len()
            } else {
                points.len() - 1
            },
        };
        let segments = segments(points, joints, bytes)?;
        let turns = turns(&segments, joints, radius, bytes)?;
        let tiles = !turns.iter().any(|j| matches!(j, Joint::Overlap))
            && advances(&segments, joints.closed);
        let caps = if tiles && !run.closed && stroke.cap == LineCap::Round {
            held_caps((*first, *last), &segments, &turns, pen, limits, bytes)?
        } else {
            None
        };
        let proven = tiles && (run.closed || stroke.cap != LineCap::Round || caps.is_some());
        if caps.is_some() {
            chains.hold(*first);
            chains.hold(*last);
        }
        if out
            .len()
            .checked_add(segments.len().saturating_mul(7))
            .is_none_or(|n| n > limits.path_segments)
        {
            return Err(Error::limit("stroke outline budget exceeded"));
        }
        let traced = Traced {
            points,
            segments: &segments,
            turns: &turns,
            joints,
            pen,
            stroke,
            room: limits.path_segments.saturating_sub(out.len()),
        };
        trace(out, &traced, caps.as_ref(), chains, limits, bytes)?;
        if let (false, None, LineCap::Round, [(start, _), ..], [.., (finish, _)]) = (
            run.closed,
            &caps,
            stroke.cap,
            segments.as_slice(),
            segments.as_slice(),
        ) {
            let back = Point::new(-start.x, -start.y);
            if chains.circle(*first, bytes)? {
                cap(out, *first, back, pen, stroke.cap, limits, bytes)?;
            }
            if chains.circle(*last, bytes)? {
                cap(out, *last, *finish, pen, stroke.cap, limits, bytes)?;
            }
        }
        Ok(proven)
    })();
    let kept = out.heap_bytes().saturating_add(chains.heap_bytes());
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(kept)));
    result
}

/// Whether the pens of `runs` stay apart: the bounding boxes of their
/// centrelines, grown by the farthest the stroke reaches from each, are
/// disjoint. Runs are compared in order of their left edges within a linear
/// number of comparisons; reaching it declines.
pub(super) fn apart<'a>(
    runs: impl ExactSizeIterator<Item = &'a Run>,
    radius: f64,
    stroke: &Stroke,
    bytes: &mut Meter,
) -> Result<bool> {
    let before = bytes.used();
    let result = (|| {
        if runs.len() < 2 {
            return Ok(true);
        }
        let cap = if stroke.cap == LineCap::ProjectingSquare {
            radius * std::f64::consts::SQRT_2
        } else {
            radius
        };
        let mut boxes = Vec::new();
        bytes.reserve(&mut boxes, runs.len())?;
        for run in runs {
            // Only a run with joints has miters.
            let reach = if stroke.join == LineJoin::Miter && (run.closed || run.points.len() > 2) {
                cap.max(radius * stroke.miter_limit)
            } else {
                cap
            };
            let mut extent = [f64::INFINITY, f64::INFINITY, -f64::INFINITY, -f64::INFINITY];
            for point in &run.points {
                extent = [
                    extent[0].min(point.x - reach),
                    extent[1].min(point.y - reach),
                    extent[2].max(point.x + reach),
                    extent[3].max(point.y + reach),
                ];
            }
            boxes.push(extent);
        }
        boxes.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]));
        let mut budget = boxes.len().saturating_mul(16);
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                if b[0] >= a[2] {
                    break;
                }
                if budget == 0 || (b[1] < a[3] && a[1] < b[3]) {
                    return Ok(false);
                }
                budget -= 1;
            }
        }
        Ok(true)
    })();
    bytes.refund(bytes.used().saturating_sub(before));
    result
}

/// How a run's joints border its segments: segment `i` runs from point `i`
/// to point `i + 1`, wrapping around in a closed run.
#[derive(Clone, Copy)]
struct Joints {
    closed: bool,
    segments: usize,
}

impl Joints {
    fn after(self, segment: usize) -> usize {
        if self.closed {
            (segment + 1) % self.segments
        } else {
            segment + 1
        }
    }

    /// The segments before and after point `j`; open ends have none.
    fn around(self, j: usize) -> Option<(usize, usize)> {
        if self.closed {
            Some(((j + self.segments - 1) % self.segments, j))
        } else {
            (j > 0 && j < self.segments).then(|| (j - 1, j))
        }
    }
}

/// Each segment's unit direction and length.
fn segments(points: &[Point], joints: Joints, bytes: &mut Meter) -> Result<Vec<(Point, f64)>> {
    let mut segments = Vec::new();
    bytes.reserve(&mut segments, joints.segments)?;
    for i in 0..joints.segments {
        let (a, b) = (points[i], points[joints.after(i)]);
        let length = (b.x - a.x).hypot(b.y - a.y);
        segments.push((
            Point::new((b.x - a.x) / length, (b.y - a.y) / length),
            length,
        ));
    }
    Ok(segments)
}

fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}

/// Whether the centreline cannot come back on itself: an open run's
/// directions stay within an open half turn, so it advances along one
/// direction throughout; a closed run turns one way, around once.
fn advances(segments: &[(Point, f64)], closed: bool) -> bool {
    let Some(&(first, _)) = segments.first() else {
        return true;
    };
    if closed {
        // Turns of one sign, and the directions pass the opposite of the
        // first one once.
        let mut sign = 0.0;
        let mut side = 0.0;
        let mut passes = 0;
        let next = segments.iter().skip(1).chain(segments.first());
        for (&(a, _), &(b, _)) in segments.iter().zip(next) {
            let turn = cross(a, b);
            if turn != 0.0 {
                if sign != 0.0 && turn.signum() != sign {
                    return false;
                }
                sign = turn.signum();
            }
            let at = cross(first, b);
            if at != 0.0 {
                if side != 0.0 && at.signum() != side {
                    passes += 1;
                }
                side = at.signum();
            }
        }
        return passes <= 1;
    }
    // The directions so far span the cone from `right` to `left`.
    let (mut right, mut left) = (first, first);
    for (&(a, _), &(b, _)) in segments.iter().zip(&segments[1..]) {
        let turn = cross(a, b);
        if turn > 0.0 && cross(left, b) > 0.0 {
            left = b;
        } else if turn < 0.0 && cross(b, right) > 0.0 {
            right = b;
        }
        let width = cross(right, left);
        if width < 0.0 || (width == 0.0 && right.x * left.x + right.y * left.y < 0.0) {
            return false;
        }
    }
    true
}

/// The joint at each point: a turn where its inner miter point and the
/// corners it cuts lie within both of its segments' pieces, else an overlap.
fn turns(
    segments: &[(Point, f64)],
    joints: Joints,
    radius: f64,
    bytes: &mut Meter,
) -> Result<Vec<Joint>> {
    let count = if joints.closed {
        joints.segments
    } else {
        joints.segments + 1
    };
    let mut turns = Vec::new();
    bytes.reserve(&mut turns, count)?;
    for j in 0..count {
        let Some((before, after)) = joints.around(j) else {
            turns.push(Joint::Straight);
            continue;
        };
        let (a, b) = (segments[before].0, segments[after].0);
        let turn = cross(a, b);
        let dot = a.x * b.x + a.y * b.y;
        if turn == 0.0 {
            turns.push(if dot > 0.0 {
                Joint::Straight
            } else {
                Joint::Overlap
            });
            continue;
        }
        // The tangent of half the turn, in the form that stays accurate near
        // a reversal, and the inner miter point it places on the inner offset
        // of the segment after; `join` forms the outer one.
        let half = if dot >= 0.0 {
            turn.abs() / (1.0 + dot)
        } else {
            (1.0 - dot) / turn.abs()
        };
        let left = turn > 0.0;
        let side = if left { radius } else { -radius };
        let reach = radius * half;
        let offset = Point::new(-b.y * side + b.x * reach, b.x * side + b.y * reach);
        if !(offset.x.is_finite() && offset.y.is_finite() && reach.is_finite()) {
            turns.push(Joint::Overlap);
            continue;
        }
        turns.push(Joint::Turn(Turn {
            offset,
            left,
            reach,
            span: radius * turn.abs(),
        }));
    }
    // A turn cuts the quadrilaterals on either side at its inner miter
    // point, which reaches into each segment by the turn's reach, and leaves
    // to the segment the corner the other quadrilateral loses there: the
    // triangle of the joint, the inner miter point and that quadrilateral's
    // inner corner, which reaches back along the segment by the turn's span
    // and, past a right angle, across to its outer side. Each side of a
    // segment must hold both inner miters, and each corner without reaching
    // past its other joint. A corner then lies within the segment's piece,
    // or behind the inner miter line of a turn the same way at the other
    // joint, where the piece beyond takes it, no deeper than that turn's own
    // span, which that piece holds in turn. Where a side cannot, the turns
    // there overlap instead, which only lowers what the segments beside them
    // must hold.
    for (i, &(_, length)) in segments.iter().enumerate() {
        let ends = [i, joints.after(i)];
        for left in [false, true] {
            // How far an end's inner miter reaches into this side, and how
            // far back its cut corner reaches.
            let cuts = ends.map(|j| match turns[j].turn() {
                Some(turn) if turn.left == left => (turn.reach, turn.reach.max(turn.span)),
                Some(turn) if turn.reach > turn.span => (0.0, turn.span),
                _ => (0.0, 0.0),
            });
            let [(reach0, corner0), (reach1, corner1)] = cuts;
            if (reach0 + reach1).max(corner0).max(corner1) > length {
                for (&j, &(_, corner)) in ends.iter().zip(&cuts) {
                    if corner > 0.0 {
                        turns[j] = Joint::Overlap;
                    }
                }
            }
        }
    }
    Ok(turns)
}

/// The chains beyond its end lines of an open round-capped run's cap
/// polygons, start then end, where each end segment holds its cap and every
/// cap vertex on or behind its end line lies across its piece. Then the cap
/// polygon behind the end line lies within the end piece, which reaches past
/// nothing else, so the end piece and the chain beyond together cover what
/// the piece and the polygon did, once.
///
/// An end segment of a longer run holds its cap when it is longer than the
/// pen radius plus a turn's inner reach at its other end. A lone segment
/// holds both: of each cap polygon, what lies beyond the other end line is
/// within sqrt(r^2 - length^2) of that end, inside the other polygon, whose
/// sides lie r cos(pi/n) from its centre. Vertices within eight rounding
/// units of their coordinates outside the piece's sides, and the rounding of
/// the two crossings per end, move the boundary by slivers that narrow,
/// within the kernel's own error. `None` when an end does not hold its cap.
fn held_caps(
    (first, last): (Point, Point),
    segments: &[(Point, f64)],
    turns: &[Joint],
    pen: Pen,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<Option<Caps>> {
    let radius = pen.radius;
    let (Some(&(start, start_length)), Some(&(finish, finish_length))) =
        (segments.first(), segments.last())
    else {
        return Ok(None);
    };
    let lone = segments.len() == 1;
    let reach = |j: usize| turns.get(j).and_then(|t| t.turn()).map_or(0.0, |t| t.reach);
    if radius <= 0.0
        || !lone
            && (start_length <= radius + reach(1)
                || finish_length <= radius + reach(segments.len() - 1))
    {
        return Ok(None);
    }
    // End-line crossings are rounded in pen space. Bound their possible
    // device-space slivers before replacing the canonical component union.
    let magnitude = (first.x.abs() + first.y.abs()).max(last.x.abs() + last.y.abs()) + radius;
    if 64.0 * f64::EPSILON * magnitude > pen.tolerance / 4096.0 {
        return Ok(None);
    }
    let before = bytes.used();
    let result = (|| {
        let circle = offsets(pen, limits, bytes)?;
        if lone {
            let count = f64::from(u32::try_from(circle.len()).unwrap_or(u32::MAX));
            let slack = 8.0
                * f64::EPSILON
                * (first.x.abs() + first.y.abs() + last.x.abs() + last.y.abs() + radius);
            let inner = radius * (std::f64::consts::PI / count).cos() - slack;
            let spill = ((radius - start_length) * (radius + start_length))
                .max(0.0)
                .sqrt();
            if spill >= inner {
                return Ok(None);
            }
        }
        let mut chains = Vec::new();
        bytes.reserve(&mut chains, 2 * circle.len() + 4)?;
        if !beyond(&circle, first, -start, radius, &mut chains) {
            return Ok(None);
        }
        let split = chains.len();
        if !beyond(&circle, last, finish, radius, &mut chains) {
            return Ok(None);
        }
        Ok(Some(Caps { chains, split }))
    })();
    let retained = result
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map_or(0, |caps| {
            caps.chains.capacity().saturating_mul(size_of::<Point>())
        });
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    result
}

/// The chains of a run's cap polygons beyond its end lines, the start's
/// then the end's, each from its piece's right side to its left.
struct Caps {
    chains: Vec<Point>,
    split: usize,
}

impl Caps {
    fn start(&self) -> &[Point] {
        &self.chains[..self.split]
    }

    fn end(&self) -> &[Point] {
        &self.chains[self.split..]
    }
}

/// Appends the chain of the cap polygon around `end`, its vertices `end`
/// plus `circle`, beyond the end line, `outward` its unit normal: where its
/// sides cross the line from the right, its vertices beyond, and where they
/// cross back on the left. Returns false unless that chain is one run, both
/// crossings lie on the quadrilateral's end, and every vertex on or behind
/// the line lies across the quadrilateral, of half-width `radius`.
fn beyond(
    circle: &[Point],
    end: Point,
    outward: Point,
    radius: f64,
    outline: &mut Vec<Point>,
) -> bool {
    let ring = circle.iter().map(|&offset| end + offset);
    let along = |v: Point| (v - end).dot(outward);
    let across = |v: Point| (v - end).dot(outward.perp());
    // Where the side from `v` to `w`, on opposite sides, crosses the line.
    let crossing = |v: Point, w: Point| v.lerp(w, along(v) / (along(v) - along(w)));
    // The vertices' own rounding, relative to their coordinates.
    let slack = 8.0 * f64::EPSILON * (end.x.abs() + end.y.abs() + radius);
    let Some(back) = ring.clone().next_back() else {
        return false;
    };
    let mut entry = None;
    let mut entries = 0_usize;
    let mut previous = back;
    for (i, v) in ring.clone().enumerate() {
        if along(v) <= 0.0 {
            if across(v).abs() > radius + slack {
                return false;
            }
        } else if along(previous) <= 0.0 {
            entry = Some((i, previous));
            entries += 1;
        }
        previous = v;
    }
    let (Some((first, behind)), 1) = (entry, entries) else {
        return false;
    };
    let mut chain = ring.cycle().skip(first);
    let Some(first) = chain.next() else {
        return false;
    };
    let enter = crossing(behind, first);
    if across(enter) < -radius - slack || across(enter) >= 0.0 {
        return false;
    }
    outline.extend([enter, first]);
    let mut last = first;
    for v in chain {
        if along(v) <= 0.0 {
            let leave = crossing(last, v);
            if across(leave) <= 0.0 || across(leave) > radius + slack {
                return false;
            }
            outline.push(leave);
            return true;
        }
        outline.push(v);
        last = v;
    }
    false
}

/// What a stroke's runs share as their outlines are traced: the buffers of
/// [`trace`], reused across the runs (an outline, and the left side it ends
/// with, which is traced forward and taken back), and the points where round
/// caps of several run ends meet.
#[derive(Default)]
pub(super) struct Chains {
    outline: Vec<Point>,
    left: Vec<Point>,
    /// Each point that two or more open runs end at, in order.
    shared: Vec<Meeting>,
    /// The shared points a round cap circle is asked for at, in the order
    /// asked: drawn once each after the runs, where no end holds its cap.
    asked: Vec<Point>,
}

/// A point where two or more open runs end, by its coordinates' bits. The
/// round cap circles there are one polygon, which covers what all of them
/// do; an end holding its cap there covers it with its chain beyond the end
/// line and its end piece behind it.
#[derive(Clone, Copy)]
struct Meeting {
    point: (u64, u64),
    /// Whether a circle is asked for there.
    asked: bool,
    /// Whether an end there holds its cap.
    held: bool,
}

/// A point by its coordinates' bits: equal keys give equal circles.
fn key(point: Point) -> (u64, u64) {
    (point.x.to_bits(), point.y.to_bits())
}

impl Chains {
    /// Chains for `runs`, with the points where round caps of their ends
    /// meet. Sorting the ends is charged to `work`.
    pub(super) fn new(
        runs: &[Run],
        stroke: &Stroke,
        limits: &Limits,
        work: &mut usize,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let mut chains = Self::default();
        if stroke.cap != LineCap::Round {
            return Ok(chains);
        }
        let ends = runs
            .iter()
            .filter(|run| !run.closed)
            .filter_map(|run| Some((*run.points.first()?, *run.points.last()?)));
        let count = ends.clone().count().saturating_mul(2);
        if count < 2 {
            return Ok(chains);
        }
        bytes.reserve(&mut chains.shared, count)?;
        super::charge(
            work,
            count.saturating_mul(count.ilog2() as usize + 1),
            limits.raster_work,
        )?;
        let shared = &mut chains.shared;
        let meeting = |point| Meeting {
            point: key(point),
            asked: false,
            held: false,
        };
        for (first, last) in ends {
            shared.extend([meeting(first), meeting(last)]);
        }
        shared.sort_unstable_by_key(|m| m.point);
        // One entry for each point at least two ends share.
        let (mut kept, mut start) = (0, 0);
        while let Some(&first) = shared.get(start) {
            let ends = shared[start..]
                .iter()
                .take_while(|m| m.point == first.point)
                .count();
            if ends > 1 {
                shared[kept] = first;
                kept += 1;
            }
            start += ends;
        }
        shared.truncate(kept);
        Ok(chains)
    }

    /// Heap bytes, charged while the chains are kept.
    pub(super) fn heap_bytes(&self) -> usize {
        self.outline
            .capacity()
            .saturating_add(self.left.capacity())
            .saturating_mul(size_of::<Point>())
            .saturating_add(self.asked.capacity().saturating_mul(size_of::<Point>()))
            .saturating_add(self.shared.capacity().saturating_mul(size_of::<Meeting>()))
    }

    fn meeting(&mut self, point: Point) -> Option<&mut Meeting> {
        let i = self
            .shared
            .binary_search_by_key(&key(point), |m| m.point)
            .ok()?;
        self.shared.get_mut(i)
    }

    /// Whether to draw the round cap circle at `point` now: not at a shared
    /// point, whose circle waits for the runs, once.
    fn circle(&mut self, point: Point, bytes: &mut Meter) -> Result<bool> {
        let Some(meeting) = self.meeting(point) else {
            return Ok(true);
        };
        if !std::mem::replace(&mut meeting.asked, true) {
            bytes.reserve_capacity(&mut self.asked, 1)?;
            self.asked.push(point);
        }
        Ok(false)
    }

    /// Records that a run end at `point` holds its round cap.
    fn hold(&mut self, point: Point) {
        if let Some(meeting) = self.meeting(point) {
            meeting.held = true;
        }
    }

    /// Draws the circles asked for at shared points where no end holds its
    /// cap.
    pub(super) fn finish(
        &mut self,
        out: &mut impl Outline,
        pen: Pen,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<()> {
        for i in 0..self.asked.len() {
            let point = self.asked[i];
            if !self.meeting(point).is_some_and(|m| m.held) {
                circle(out, point, pen, limits, bytes)?;
            }
        }
        self.asked.clear();
        Ok(())
    }
}

/// A run's geometry as its outline traces it: its points, its segments'
/// directions and lengths, and how they meet.
struct Traced<'a> {
    points: &'a [Point],
    segments: &'a [(Point, f64)],
    turns: &'a [Joint],
    joints: Joints,
    pen: Pen,
    stroke: &'a Stroke,
    /// The outline segments the output admitted when the run began.
    room: usize,
}

impl Traced<'_> {
    /// Refuses a chain that no polygon within the outline budget closes,
    /// before more joints are traced into it: a contour holds at least
    /// either of its chains, plus its close.
    fn fits(&self, chain: &[Point]) -> Result<()> {
        if chain.len() >= self.room {
            return Err(Error::limit("stroke outline budget exceeded"));
        }
        Ok(())
    }

    /// Segment `i`'s corner at its joint `j` on one side, as its piece has
    /// it.
    fn corner(&self, i: usize, j: usize, left: bool) -> Point {
        let (direction, _) = self.segments[i];
        let radius = self.pen.radius;
        let normal = Point::new(-direction.y * radius, direction.x * radius);
        match self.turns[j] {
            Joint::Turn(turn) if turn.left == left => self.points[j] + turn.offset,
            _ if left => self.points[j] + normal,
            _ => self.points[j] - normal,
        }
    }

    /// Whether the pieces on either side of inner joint `j` meet there:
    /// only collinear segments' normals can differ, in the last place.
    fn meets(&self, j: usize) -> bool {
        self.joints.around(j).is_some_and(|(a, b)| {
            !matches!(self.turns[j], Joint::Straight)
                || (self.corner(a, j, true) == self.corner(b, j, true)
                    && self.corner(a, j, false) == self.corner(b, j, false))
        })
    }

    /// The outline on one side across inner joint `j`, from the corner of
    /// the segment before to that of the one after: the inner miter point,
    /// the join's vertices, or the joint itself between the two normals.
    fn across(
        &self,
        j: usize,
        left: bool,
        chain: &mut Vec<Point>,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<()> {
        let Some((a, b)) = self.joints.around(j) else {
            return Ok(());
        };
        let (from, to) = (self.corner(a, j, left), self.corner(b, j, left));
        match self.turns[j] {
            Joint::Straight => {}
            Joint::Turn(turn) if turn.left == left => {}
            _ => {
                let tangents = [self.segments[a].0, self.segments[b].0];
                let cross = tangents[0].cross(tangents[1]);
                let drawn = cross != 0.0
                    || (self.stroke.join == LineJoin::Round && tangents[0].dot(tangents[1]) < 0.0);
                if drawn && (cross <= 0.0) == left {
                    let start = chain.len();
                    let point = self.points[j];
                    join_into(point, tangents, self.pen, self.stroke, limits, chain, bytes)?;
                    debug_assert_eq!((chain.get(start), chain.last()), (Some(&from), Some(&to)));
                } else {
                    bytes.reserve_capacity(chain, 3)?;
                    chain.extend([from, self.points[j], to]);
                }
                return self.fits(chain);
            }
        }
        bytes.reserve_capacity(chain, 1)?;
        chain.push(from);
        self.fits(chain)
    }

    /// One side of the part of `length` segments from segment `start`.
    fn side(
        &self,
        (start, length): (usize, usize),
        left: bool,
        chain: &mut Vec<Point>,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<()> {
        let count = self.segments.len();
        let joint = |t: usize| {
            if self.joints.closed {
                (start + t) % count
            } else {
                start + t
            }
        };
        bytes.reserve_capacity(chain, 1)?;
        chain.push(self.corner(start % count, joint(0), left));
        for t in 1..length {
            self.across(joint(t), left, chain, limits, bytes)?;
        }
        bytes.reserve_capacity(chain, 1)?;
        chain.push(self.corner((start + length - 1) % count, joint(length), left));
        Ok(())
    }

    /// The vertices of the end of an open run beyond its last piece, from
    /// its right corner to its left: the held round cap's chain or the
    /// square cap's far side. `back` takes the start, beyond the first.
    fn end(
        &self,
        back: bool,
        caps: Option<&Caps>,
        chain: &mut Vec<Point>,
        bytes: &mut Meter,
    ) -> Result<()> {
        let held = caps.map(|caps| if back { caps.start() } else { caps.end() });
        if let Some(held) = held {
            bytes.reserve_capacity(chain, held.len())?;
            chain.extend_from_slice(held);
        } else if self.stroke.cap == LineCap::ProjectingSquare {
            let (point, direction) = if back {
                let (direction, _) = self.segments[0];
                (self.points[0], Point::new(-direction.x, -direction.y))
            } else {
                let (direction, _) = self.segments[self.segments.len() - 1];
                (self.points[self.segments.len()], direction)
            };
            // As `cap` makes it: its side along the end line is the piece's.
            let n = direction.perp() * self.pen.radius;
            let tip = point + direction * self.pen.radius;
            bytes.reserve_capacity(chain, 2)?;
            chain.extend([tip - n, tip + n]);
        }
        Ok(())
    }

    /// Whether the outline is reversed to take the orientation that
    /// [`polygon`](super::polygon) gives the run's first piece.
    fn reversed(&self) -> bool {
        let k = self.joints.after(0);
        let quad = [
            self.corner(0, 0, false),
            self.corner(0, k, false),
            self.corner(0, k, true),
            self.corner(0, 0, true),
        ];
        let area: f64 = quad
            .iter()
            .zip(quad.iter().cycle().skip(1))
            .map(|(&a, &b)| (a - quad[0]).cross(b - quad[0]))
            .sum();
        area < 0.0
    }
}

/// Appends the outline of a run's pieces, joins and square caps: their
/// sides less the ones two of them share, which cancel in the nonzero
/// winding. Pieces that meet at a joint share their vertices bit for bit
/// (a turn's inner miter point, the join's ends at the normals, a square
/// cap's end line), so at a turn the outline passes the inner miter point
/// and the join's outer vertices, and beside an overlap also the joint
/// itself where no join lies. A closed run whose pieces all meet has two
/// loops; elsewhere each part between joints whose pieces do not meet is
/// one contour, its ends the run's caps or those joints' end lines. Every
/// contour takes the orientation the run's first piece takes.
fn trace(
    out: &mut impl Outline,
    run: &Traced<'_>,
    caps: Option<&Caps>,
    Chains { outline, left, .. }: &mut Chains,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    let count = run.segments.len();
    let closed = run.joints.closed;
    let reversed = run.reversed();
    outline.clear();
    left.clear();
    let split = (usize::from(!closed)..count).find(|&j| !run.meets(j));
    let (first, last) = match (closed, split) {
        (true, None) => {
            for j in 0..count {
                run.across(j, false, outline, limits, bytes)?;
                run.across(j, true, left, limits, bytes)?;
            }
            left.reverse();
            oriented(out, outline, reversed, limits, bytes)?;
            return oriented(out, left, reversed, limits, bytes);
        }
        (true, Some(split)) => (split, split + count),
        (false, _) => (0, count),
    };
    let mut start = first;
    while start < last {
        let end = (start + 1..last)
            .find(|&t| !run.meets(t % count))
            .unwrap_or(last);
        let part = (start, end - start);
        outline.clear();
        left.clear();
        run.side(part, false, outline, limits, bytes)?;
        run.side(part, true, left, limits, bytes)?;
        if !closed && end == count {
            run.end(false, caps, outline, bytes)?;
        }
        bytes.reserve_capacity(outline, left.len())?;
        outline.extend(left.iter().rev());
        if !closed && start == 0 {
            run.end(true, caps, outline, bytes)?;
        }
        oriented(out, outline, reversed, limits, bytes)?;
        start = end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::flatten::TOLERANCE;
    use crate::stroke::{Tiling, pieces};
    use crate::testing::{fill, stroke as band_stroke, window};
    use crate::{FillRule, LineCap, LineJoin, Path, PathBuilder, Point, Stroke, Transform};
    use pagura_core::Limits;

    const SIZE: u32 = 48;

    fn polyline(points: &[(f64, f64)], closed: bool) -> Path {
        let mut path = PathBuilder::new();
        for (i, &(x, y)) in points.iter().enumerate() {
            if i == 0 {
                path.move_to(Point::new(x, y));
            } else {
                path.line_to(Point::new(x, y)).unwrap();
            }
        }
        if closed {
            path.close();
        }
        path.finish()
    }

    fn arc() -> Path {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(3.0, 12.0));
        path.cubic_to(
            Point::new(3.0, 5.0),
            Point::new(8.0, 3.0),
            Point::new(12.0, 3.0),
        )
        .unwrap();
        path.line_to(Point::new(20.0, 3.5)).unwrap();
        path.finish()
    }

    fn exact(outline: &Path, pen: Transform) -> Vec<u8> {
        let limits = Limits::default();
        fill(
            window(SIZE, SIZE),
            (outline, pen, FillRule::NonZero),
            &[],
            &limits,
        )
        .unwrap()
    }

    /// The outline as pieces where every run is proven to tile, else as
    /// components.
    fn tiled(path: &Path, transform: Transform, stroke: &Stroke) -> (Path, Transform, Tiling) {
        let limits = Limits::default();
        pieces(
            path,
            transform,
            stroke,
            &limits,
            (Tiling::Proven, TOLERANCE),
            &mut crate::testing::bytes(),
        )
        .unwrap()
    }

    /// The band rasterizer's stroke, whose pieces, where proven to tile,
    /// integrate as signed area without resolving overlaps.
    fn cells(path: &Path, stroke: &Stroke) -> Vec<u8> {
        let limits = Limits::default();
        band_stroke(
            window(SIZE, SIZE),
            path,
            stroke,
            Transform::IDENTITY,
            &limits,
        )
        .unwrap()
    }

    fn largest_difference(a: &[u8], b: &[u8]) -> u8 {
        a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap()
    }

    fn strokes() -> Vec<(Path, Stroke)> {
        let zigzag = || polyline(&[(2.0, 2.5), (14.0, 4.0), (5.0, 12.0), (21.0, 20.5)], false);
        let mut cases = Vec::new();
        for width in [0.6, 1.5, 3.5] {
            for join in [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel] {
                for cap in [LineCap::Butt, LineCap::Round, LineCap::ProjectingSquare] {
                    let stroke = Stroke {
                        width,
                        cap,
                        join,
                        ..Stroke::default()
                    };
                    cases.push((zigzag(), stroke.clone()));
                    cases.push((arc(), stroke.clone()));
                    let triangle = polyline(&[(4.0, 4.0), (20.0, 6.0), (9.0, 19.0)], true);
                    cases.push((triangle, stroke));
                }
            }
        }
        // Dashes, including zero-length ones that are only caps, whose pens
        // stay apart.
        for cap in [LineCap::Round, LineCap::ProjectingSquare] {
            let stroke = Stroke {
                width: 2.0,
                cap,
                dash_array: vec![0.0, 5.0, 3.0, 4.0],
                ..Stroke::default()
            };
            cases.push((polyline(&[(2.0, 6.0), (21.0, 9.5)], false), stroke));
        }
        cases
    }

    #[test]
    fn pieces_cover_the_component_set() {
        let limits = Limits::default();
        for transform in [
            Transform::IDENTITY,
            Transform::scale(1.0, 0.6).then(Transform::translate(0.3, 4.1)),
        ] {
            for (path, stroke) in strokes() {
                let (components, pen) =
                    crate::stroke_outline(&path, transform, &stroke, &limits).unwrap();
                let (tiles, tiled_pen, tiling) = tiled(&path, transform, &stroke);
                assert_eq!(pen, tiled_pen);
                let round_caps = stroke.cap == LineCap::Round
                    && !matches!(path.segments().last(), Some(crate::Segment::Close));
                let source_caps = path
                    .segments()
                    .iter()
                    .any(|segment| matches!(segment, crate::Segment::CubicTo { .. }));
                if source_caps {
                    // This open arch's source tangent differs from its first
                    // chord: the chord-normal certificate must not certify it.
                    assert_eq!(tiling, Tiling::Components, "{stroke:?}");
                    assert_eq!(tiles.segments(), components.segments());
                } else if round_caps && tiling == Tiling::Components {
                    assert_eq!(tiles.segments(), components.segments());
                } else {
                    assert_eq!(tiling, Tiling::Proven, "{stroke:?}");
                }
                let difference = largest_difference(&exact(&tiles, pen), &exact(&components, pen));
                assert!(difference <= 1, "{stroke:?} {transform:?}: {difference}");
            }
        }
    }

    #[test]
    fn end_segments_that_hold_their_round_caps_tile() {
        let mut state = 0x6a09_e667_f3bc_c909_u64;
        let mut bits = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 random bits map to a uniform fraction"
        )]
        let mut next = || (bits() >> 11) as f64 / (1u64 << 53) as f64;
        let mut taken = 0;
        for case in 0..600 {
            let width = 0.2 + 5.0 * next();
            // Headings within a quarter turn either side of the first, so
            // the centreline advances; every fourth run along the axes.
            let first = std::f64::consts::TAU * next();
            let mut points = vec![(12.0 + 8.0 * next(), 12.0 + 8.0 * next())];
            for _ in 0..2 + case % 3 {
                let quarter = std::f64::consts::FRAC_PI_2;
                let heading = if case % 4 == 0 {
                    quarter * (first / quarter).floor() + quarter * (next() - 0.5).round()
                } else {
                    first + 0.95 * quarter * (2.0 * next() - 1.0)
                };
                let length = width * (0.3 + 3.0 * next());
                let &(x, y) = points.last().unwrap();
                points.push((x + length * heading.cos(), y + length * heading.sin()));
            }
            let path = polyline(&points, false);
            let stroke = Stroke {
                width,
                cap: LineCap::Round,
                join: [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel][case % 3],
                ..Stroke::default()
            };
            if let Some(difference) = pieces_match_the_union(&path, &stroke) {
                assert!(
                    difference <= 1,
                    "{case} {points:?} {stroke:?}: {difference}"
                );
                taken += 1;
            }
        }
        assert!(taken > 150, "{taken}");
    }

    #[test]
    fn signed_cells_of_pieces_integrate_the_union() {
        let limits = Limits::default();
        for (path, stroke) in strokes() {
            let (components, pen) =
                crate::stroke_outline(&path, Transform::IDENTITY, &stroke, &limits).unwrap();
            let difference = largest_difference(&cells(&path, &stroke), &exact(&components, pen));
            assert!(difference <= 1, "{stroke:?}: {difference}");
        }
    }

    /// Strokes whose pieces could overlap keep the component outline.
    fn components_kept(path: &Path, stroke: &Stroke) {
        let limits = Limits::default();
        let (components, _) =
            crate::stroke_outline(path, Transform::IDENTITY, stroke, &limits).unwrap();
        let (outline, _, tiling) = tiled(path, Transform::IDENTITY, stroke);
        assert_eq!(tiling, Tiling::Components, "{stroke:?}");
        assert_eq!(outline.segments(), components.segments());
    }

    #[test]
    fn possible_overlaps_keep_the_components() {
        let stroke = Stroke {
            width: 2.0,
            ..Stroke::default()
        };
        // A reversal.
        components_kept(
            &polyline(&[(2.0, 4.0), (16.0, 4.0), (6.0, 4.0)], false),
            &stroke,
        );
        // Segments shorter than the inner reach of their turns.
        components_kept(
            &polyline(&[(4.0, 4.0), (12.0, 4.0), (12.5, 4.8), (4.0, 9.0)], false),
            &stroke,
        );
        // An open centreline whose directions span a half turn, and a closed
        // one that crosses itself.
        components_kept(
            &polyline(&[(4.0, 4.0), (16.0, 4.0), (16.0, 12.0), (8.0, 12.0)], false),
            &stroke,
        );
        components_kept(
            &polyline(&[(3.0, 3.0), (20.0, 20.0), (20.0, 3.0), (3.0, 20.0)], true),
            &stroke,
        );
        // Runs whose pens may meet: a frame and a line across it.
        let mut crossed = PathBuilder::new();
        crossed.rect(3.0, 3.0, 16.0, 16.0);
        crossed.move_to(Point::new(1.0, 10.0));
        crossed.line_to(Point::new(21.0, 11.0)).unwrap();
        components_kept(&crossed.finish(), &stroke);
    }

    #[test]
    fn round_caps_not_held_keep_open_runs_unproven() {
        let limits = Limits::default();
        // A segment too short for either circle to lie within the other
        // beyond its end line, and an end segment shorter than the pen.
        for points in [
            vec![(8.0, 8.0), (8.031_25, 8.0)],
            vec![(8.0, 8.0), (18.0, 8.0), (18.5, 8.0)],
        ] {
            let path = polyline(&points, false);
            let stroke = Stroke {
                width: 2.0,
                cap: LineCap::Round,
                ..Stroke::default()
            };
            let (_, _, tiling) = pieces(
                &path,
                Transform::IDENTITY,
                &stroke,
                &limits,
                (Tiling::Unproven, TOLERANCE),
                &mut crate::testing::bytes(),
            )
            .unwrap();
            assert_eq!(tiling, Tiling::Unproven);
            components_kept(&path, &stroke);
        }
    }

    #[test]
    fn a_point_takes_its_round_cap_circle_once() {
        let limits = Limits::default();
        // A zero-length segment, and zero-length dashes whose pens stay apart.
        for (points, dash_array) in [
            (vec![(8.0, 8.0), (8.0, 8.0)], vec![]),
            (vec![(8.0, 8.0), (18.0, 8.0)], vec![0.0, 3.0]),
        ] {
            let path = polyline(&points, false);
            let stroke = Stroke {
                width: 2.0,
                cap: LineCap::Round,
                dash_array,
                ..Stroke::default()
            };
            let (components, pen) =
                crate::stroke_outline(&path, Transform::IDENTITY, &stroke, &limits).unwrap();
            let (tiles, _, tiling) = tiled(&path, Transform::IDENTITY, &stroke);
            assert_eq!(tiling, Tiling::Proven);
            assert_eq!(tiles.segments().len() * 2, components.segments().len());
            assert_eq!(exact(&tiles, pen), exact(&components, pen));
            assert_eq!(cells(&path, &stroke), exact(&components, pen));
        }
    }

    #[test]
    fn lone_round_capped_segments_take_one_convex_outline() {
        let limits = Limits::default();
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut bits = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 random bits map to a uniform fraction"
        )]
        let mut next = || (bits() >> 11) as f64 / (1u64 << 53) as f64;
        for case in 0..300 {
            let width = 0.2 + 7.0 * next();
            // Every fourth segment runs along an axis, where cap vertices
            // fall on the end line.
            let quarter = std::f64::consts::FRAC_PI_2;
            let heading = if case % 4 == 0 {
                quarter * (4.0 * next()).floor()
            } else {
                std::f64::consts::TAU * next()
            };
            // From a fraction of the pen radius, where each circle beyond
            // the other end line lies within the other, to well past it.
            let length = width * (0.15 + 3.0 * next());
            let (x, y) = (12.0 + 24.0 * next(), 12.0 + 24.0 * next());
            let path = polyline(
                &[
                    (x, y),
                    (x + length * heading.cos(), y + length * heading.sin()),
                ],
                false,
            );
            let stroke = Stroke {
                width,
                cap: LineCap::Round,
                ..Stroke::default()
            };
            for transform in [
                Transform::IDENTITY,
                Transform::scale(1.0, 0.6).then(Transform::translate(0.3, 4.1)),
            ] {
                let (components, pen) =
                    crate::stroke_outline(&path, transform, &stroke, &limits).unwrap();
                let (tiles, _, tiling) = tiled(&path, transform, &stroke);
                assert_eq!(tiling, Tiling::Proven, "{case}");
                assert_eq!(exact(&tiles, pen), exact(&components, pen), "{case}");
            }
            let (components, pen) =
                crate::stroke_outline(&path, Transform::IDENTITY, &stroke, &limits).unwrap();
            assert_eq!(cells(&path, &stroke), exact(&components, pen), "{case}");
        }
    }

    #[test]
    fn separate_runs_and_convex_closed_runs_take_pieces() {
        let limits = Limits::default();
        let stroke = Stroke {
            width: 2.0,
            join: LineJoin::Round,
            ..Stroke::default()
        };
        let mut separate = PathBuilder::new();
        for (x, y) in [(2.0, 3.0), (2.0, 9.0), (2.0, 15.0)] {
            separate.move_to(Point::new(x, y));
            separate.line_to(Point::new(x + 8.0, y + 1.0)).unwrap();
            separate.line_to(Point::new(x + 18.0, y + 3.0)).unwrap();
        }
        let pentagon = polyline(
            &[
                (12.0, 2.0),
                (21.0, 9.0),
                (17.0, 20.0),
                (7.0, 20.0),
                (3.0, 9.0),
            ],
            true,
        );
        for path in [separate.finish(), pentagon] {
            let (components, pen) =
                crate::stroke_outline(&path, Transform::IDENTITY, &stroke, &limits).unwrap();
            let (_, _, tiling) = tiled(&path, Transform::IDENTITY, &stroke);
            assert_eq!(tiling, Tiling::Proven);
            let union = exact(&components, pen);
            assert!(largest_difference(&cells(&path, &stroke), &union) <= 1);
        }
    }

    /// Whether the stroke keeps its components, or the largest difference
    /// of its pieces' signed cells from the components' exact union.
    fn pieces_match_the_union(path: &Path, stroke: &Stroke) -> Option<u8> {
        let limits = Limits::default();
        let (components, pen) =
            crate::stroke_outline(path, Transform::IDENTITY, stroke, &limits).unwrap();
        let (_, _, tiling) = tiled(path, Transform::IDENTITY, stroke);
        (tiling == Tiling::Proven)
            .then(|| largest_difference(&cells(path, stroke), &exact(&components, pen)))
    }

    #[test]
    fn inexact_reversals_keep_the_components() {
        let stroke = Stroke {
            width: 3.0,
            cap: LineCap::Round,
            ..Stroke::default()
        };
        // Out along a diagonal and back along it: rounding leaves the return
        // direction a hair off the reverse, so the turn is not exactly flat.
        for step in 1..64 {
            let t = f64::from(step) / 64.0;
            let (dx, dy) = (0.1 + 0.83 * t, 1.0 - 0.71 * t);
            let far = (6.0 + 30.0 * dx, 6.0 + 30.0 * dy);
            let back = (6.0 + 9.0 * dx * 1.1, 6.0 + 9.0 * dy * 1.1);
            let path = polyline(&[(6.0, 6.0), far, back], false);
            if let Some(difference) = pieces_match_the_union(&path, &stroke) {
                assert!(difference <= 1, "{step}: {difference}");
            }
        }
    }

    #[test]
    fn butt_ends_hold_the_neighbouring_quadrilateral() {
        // A wide pen turning 60 degrees onto a last segment shorter than the
        // reach of the segment before.
        let angle = std::f64::consts::FRAC_PI_3;
        let path = polyline(
            &[
                (6.0, 20.0),
                (26.0, 20.0),
                (26.0 + 5.0 * angle.cos(), 20.0 + 5.0 * angle.sin()),
            ],
            false,
        );
        let stroke = Stroke {
            width: 16.0,
            cap: LineCap::Butt,
            join: LineJoin::Round,
            ..Stroke::default()
        };
        if let Some(difference) = pieces_match_the_union(&path, &stroke) {
            assert!(difference <= 1, "{difference}");
        }
    }

    /// The largest difference of the exact union of a stroke's pieces,
    /// proven to tile or not, from the components' exact union.
    fn pieces_union_difference(path: &Path, transform: Transform, stroke: &Stroke) -> u8 {
        let limits = Limits::default();
        let (components, pen) = crate::stroke_outline(path, transform, stroke, &limits).unwrap();
        let (tiles, tiled_pen, _) = pieces(
            path,
            transform,
            stroke,
            &limits,
            (Tiling::Unproven, TOLERANCE),
            &mut crate::testing::bytes(),
        )
        .unwrap();
        largest_difference(&exact(&tiles, tiled_pen), &exact(&components, pen))
    }

    #[test]
    fn turns_beside_a_reversal_keep_their_cut_corners() {
        // Up, a little left, back right a little further, then a turn of 49
        // degrees: the corner the last quadrilateral loses at that turn
        // reaches back past the reversal, where nothing else covers it.
        let path = polyline(
            &[
                (14.47, 18.0),
                (14.47, 15.68),
                (14.33, 15.68),
                (14.61, 15.68),
                (15.03, 15.2),
                (15.59, 12.5),
            ],
            false,
        );
        for scale in [1.0, 2.0, 4.0] {
            let transform = Transform::translate(-14.0, -15.0)
                .then(Transform::scale(scale, scale))
                .then(Transform::translate(14.0, 15.0));
            for join in [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel] {
                let stroke = Stroke {
                    width: 1.0,
                    join,
                    ..Stroke::default()
                };
                let difference = pieces_union_difference(&path, transform, &stroke);
                assert!(difference <= 1, "{scale} {join:?}: {difference}");
            }
        }
    }

    #[test]
    fn seeded_pieces_cover_the_union_proven_or_not() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut bits = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 random bits map to a uniform fraction"
        )]
        let mut next = || (bits() >> 11) as f64 / (1u64 << 53) as f64;
        for case in 0..1500 {
            let width = 0.5 + 8.0 * next();
            let mut points = vec![(8.0 + 32.0 * next(), 8.0 + 32.0 * next())];
            for _ in 0..3 + usize::from(next() < 0.5) * 2 {
                let &(x, y) = points.last().unwrap();
                let (dx, dy) = match points.len() {
                    1 => (1.0, 0.0),
                    n => (x - points[n - 2].0, y - points[n - 2].1),
                };
                let heading = dy.atan2(dx);
                // Hops shorter than the pen beside turns of any angle, and
                // straight reversals.
                let turn = if next() < 0.25 {
                    std::f64::consts::PI
                } else {
                    (next() - 0.5) * 2.0 * std::f64::consts::PI
                };
                let length = width * (0.05 + 1.5 * next());
                let (x, y) = (
                    x + length * (heading + turn).cos(),
                    y + length * (heading + turn).sin(),
                );
                points.push((x.clamp(2.0, 46.0), y.clamp(2.0, 46.0)));
            }
            let path = polyline(&points, case % 7 == 3);
            let stroke = Stroke {
                width,
                cap: [LineCap::Butt, LineCap::Round, LineCap::ProjectingSquare][case % 3],
                join: [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel][(case / 3) % 3],
                ..Stroke::default()
            };
            let difference = pieces_union_difference(&path, Transform::IDENTITY, &stroke);
            assert!(
                difference <= 1,
                "{case} {points:?} {stroke:?}: {difference}"
            );
        }
    }

    #[test]
    fn seeded_bends_into_breaks_cover_the_union() {
        // Short segments bending one way, then a reversal, a turn the other
        // way or an end: the corners the bend's turns cut stay covered
        // whether or not the pieces are proven to tile.
        let mut state = 0x9e37_79b9_6c07_8965_u64;
        let mut bits = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 random bits map to a uniform fraction"
        )]
        let mut next = || (bits() >> 11) as f64 / (1u64 << 53) as f64;
        for case in 0..3000 {
            let width = 0.5 + 6.0 * next();
            let mut points = vec![(10.0 + 20.0 * next(), 10.0 + 20.0 * next())];
            let mut heading = next() * std::f64::consts::TAU;
            let mut sign = if next() < 0.5 { 1.0 } else { -1.0 };
            let segments = 2 + usize::from(next() < 0.5) + usize::from(next() < 0.5) * 3;
            for k in 0..segments {
                if k + 1 == segments && next() < 0.6 {
                    heading += if next() < 0.5 {
                        std::f64::consts::PI
                    } else {
                        -sign * (0.2 + 1.5 * next())
                    };
                } else if k > 0 {
                    if case % 5 == 4 && next() < 0.3 {
                        sign = -sign;
                    }
                    heading += sign * (0.05 + 1.2 * next());
                }
                let length = width * (0.025 + 1.25 * next());
                let &(x, y) = points.last().unwrap();
                points.push((
                    (x + length * heading.cos()).clamp(2.0, 46.0),
                    (y + length * heading.sin()).clamp(2.0, 46.0),
                ));
            }
            if next() < 0.5 {
                points.reverse();
            }
            let path = polyline(&points, case % 7 == 6);
            let stroke = Stroke {
                width,
                cap: [LineCap::Butt, LineCap::Round, LineCap::ProjectingSquare][case % 3],
                join: [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel][(case / 3) % 3],
                ..Stroke::default()
            };
            let difference = pieces_union_difference(&path, Transform::IDENTITY, &stroke);
            assert!(
                difference <= 1,
                "{case} {points:?} {stroke:?}: {difference}"
            );
        }
    }

    #[test]
    fn collinear_segments_whose_normals_differ_split_the_outline() {
        // Straight joints whose segments' unit directions differ in the last
        // place: their pieces do not meet, and the outline splits there.
        let mut found = 0;
        for k in 1..4000_u32 {
            let t = f64::from(k);
            let (dx, dy) = (0.3 + t * 0.000_731, 0.7 - t * 0.000_113);
            let path = [
                (6.0, 6.0),
                (6.0 + 9.1 * dx, 6.0 + 9.1 * dy),
                (6.0 + 23.7 * dx, 6.0 + 23.7 * dy),
            ];
            let points: Vec<Point> = path.iter().map(|&(x, y)| Point::new(x, y)).collect();
            let joints = super::Joints {
                closed: false,
                segments: 2,
            };
            let segments = super::segments(&points, joints, &mut crate::testing::bytes()).unwrap();
            let turns = super::turns(&segments, joints, 1.0, &mut crate::testing::bytes()).unwrap();
            let traced = super::Traced {
                points: &points,
                segments: &segments,
                turns: &turns,
                joints,
                pen: super::Pen {
                    radius: 1.0,
                    tolerance: TOLERANCE,
                },
                stroke: &Stroke::default(),
                room: usize::MAX,
            };
            if !matches!(turns[1], super::Joint::Straight) || traced.meets(1) {
                continue;
            }
            found += 1;
            for cap in [LineCap::Butt, LineCap::Round, LineCap::ProjectingSquare] {
                let stroke = Stroke {
                    width: 2.0,
                    cap,
                    ..Stroke::default()
                };
                let difference =
                    pieces_union_difference(&polyline(&path, false), Transform::IDENTITY, &stroke);
                assert!(difference <= 1, "{path:?} {cap:?}: {difference}");
            }
        }
        assert!(found > 0);
    }

    #[test]
    fn seeded_strokes_take_pieces_only_within_one_level_of_the_union() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut bits = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 random bits map to a uniform fraction"
        )]
        let mut next = || (bits() >> 11) as f64 / (1u64 << 53) as f64;
        let mut taken = 0;
        // Open round caps now retain overlapping canonical circles. Keep
        // enough eligible runs to exercise the proven-piece admission too.
        for case in 0..2000 {
            let count = 2 + usize::from(next() < 0.5) + usize::from(next() < 0.5) * 2;
            let mut points: Vec<(f64, f64)> = (0..count)
                .map(|_| (4.0 + 40.0 * next(), 4.0 + 40.0 * next()))
                .collect();
            if case % 4 == 0 {
                // Back along the last segment, short of where it started.
                let [.., (x0, y0), (x1, y1)] = points[..] else {
                    unreachable!()
                };
                let t = 0.2 + 0.7 * next();
                points.push((x1 + (x0 - x1) * t, y1 + (y0 - y1) * t));
            }
            let width = 0.3 + 16.0 * next();
            if case % 4 == 2 {
                // A turn of under a right angle onto a last segment about
                // as long as the pen radius.
                let [.., (x0, y0), (x1, y1)] = points[..] else {
                    unreachable!()
                };
                let turn = (0.3 + 1.2 * next()) * if next() < 0.5 { 1.0 } else { -1.0 };
                let heading = (y1 - y0).atan2(x1 - x0) + turn;
                let length = width * (0.3 + 0.4 * next());
                points.push((x1 + length * heading.cos(), y1 + length * heading.sin()));
            }
            let path = polyline(&points, case % 5 == 1);
            let stroke = Stroke {
                width,
                cap: [LineCap::Butt, LineCap::Round, LineCap::ProjectingSquare][case % 3],
                join: [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel][(case / 3) % 3],
                ..Stroke::default()
            };
            if let Some(difference) = pieces_match_the_union(&path, &stroke) {
                assert!(
                    difference <= 1,
                    "{case} {points:?} {stroke:?}: {difference}"
                );
                taken += 1;
            }
        }
        assert!(taken > 200, "{taken}");
    }

    /// A closed star of 40 spikes, each tip and notch nearly a reversal:
    /// round joins make most of its outline, on both sides.
    fn star() -> Vec<(f64, f64)> {
        (0..80_u32)
            .map(|i| {
                let angle = std::f64::consts::TAU * f64::from(i) / 80.0;
                let radius = if i % 2 == 0 { 200.0 } else { 20.0 };
                (radius * angle.cos(), radius * angle.sin())
            })
            .collect()
    }

    #[test]
    fn traced_outlines_refuse_exactly_at_the_outline_budget() {
        let path = polyline(&star(), true);
        let stroke = Stroke {
            width: 30.0,
            join: LineJoin::Round,
            ..Stroke::default()
        };
        let outline = |path_segments| {
            let limits = Limits {
                path_segments,
                ..Limits::default()
            };
            pieces(
                &path,
                Transform::IDENTITY,
                &stroke,
                &limits,
                (Tiling::Unproven, TOLERANCE),
                &mut crate::testing::bytes(),
            )
        };
        let (full, _, tiling) = outline(Limits::default().path_segments).unwrap();
        assert_eq!(tiling, Tiling::Unproven);
        let needed = full.segments().len();
        assert!(needed > 7 * 80, "{needed}");
        assert_eq!(outline(needed).unwrap().0, full);
        let error = outline(needed - 1).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (
                pagura_core::ErrorKind::Limit,
                "stroke outline budget exceeded"
            )
        );
    }

    #[test]
    fn tracing_stops_within_the_outline_budget() {
        // Both chains of a closed run grow joint by joint before either
        // closes; they stop where the budget refuses them, not after every
        // join arc of the run.
        let run = super::super::Run {
            points: star().into_iter().map(|(x, y)| Point::new(x, y)).collect(),
            closed: true,
            tangent: Point::new(1.0, 0.0),
            caps: [None; 2],
            joins: Vec::new(),
        };
        let pen = super::Pen {
            radius: 15.0,
            tolerance: 0.001,
        };
        let stroke = Stroke {
            width: 30.0,
            join: LineJoin::Round,
            ..Stroke::default()
        };
        let trace = |limits: &Limits, chains: &mut super::Chains| {
            let mut out = PathBuilder::new();
            super::append(
                &mut out,
                &run,
                (pen, &stroke),
                chains,
                limits,
                &mut crate::testing::bytes(),
            )
            .map(|_| out.len())
        };
        let full = trace(&Limits::default(), &mut super::Chains::default()).unwrap();
        let limits = Limits {
            path_segments: 7 * 80,
            ..Limits::default()
        };
        assert!(full > 8 * limits.path_segments, "{full}");
        let mut chains = super::Chains::default();
        let error = trace(&limits, &mut chains).unwrap_err();
        assert_eq!(error.message, "stroke outline budget exceeded");
        let traced = chains.outline.len().max(chains.left.len());
        assert!(traced < 2 * limits.path_segments, "{traced} of {full}");
    }

    /// One path of the open subpaths through `parts`' points.
    fn subpaths(parts: &[&[(f64, f64)]]) -> Path {
        let mut path = PathBuilder::new();
        for part in parts {
            for (i, &(x, y)) in part.iter().enumerate() {
                if i == 0 {
                    path.move_to(Point::new(x, y));
                } else {
                    path.line_to(Point::new(x, y)).unwrap();
                }
            }
        }
        path.finish()
    }

    #[test]
    fn round_caps_of_meeting_run_ends_take_one_circle() {
        // Two dots, and a run that doubles back so that it holds neither
        // cap, all at one point; the run's far end and a third dot apart.
        let dot: &[(f64, f64)] = &[(12.0, 12.0), (12.0, 12.0)];
        let back: &[(f64, f64)] = &[(12.0, 12.0), (20.0, 15.0), (16.0, 13.5)];
        let apart: &[(f64, f64)] = &[(30.0, 30.0), (30.0, 30.0)];
        let stroke = Stroke {
            width: 3.0,
            cap: LineCap::Round,
            ..Stroke::default()
        };
        let outline = |parts: &[&[(f64, f64)]]| {
            pieces(
                &subpaths(parts),
                Transform::IDENTITY,
                &stroke,
                &Limits::default(),
                (Tiling::Unproven, TOLERANCE),
                &mut crate::testing::bytes(),
            )
            .unwrap()
            .0
        };
        let circle = outline(&[dot]).segments().len();
        let alone = outline(&[back]).segments().len();
        let parts: &[&[(f64, f64)]] = &[dot, back, dot, apart];
        let meeting = outline(parts).segments().len();
        // Of five circles, the second dot's and the run's first cap repeat
        // the first dot's; the run's far cap and the dot apart do not.
        assert_eq!(meeting, alone + circle);
        assert_eq!(
            pieces_union_difference(&subpaths(parts), Transform::IDENTITY, &stroke),
            0
        );
    }

    #[test]
    fn a_held_cap_covers_the_circles_of_ends_meeting_it() {
        // A segment holding both caps, a dot at each of its ends and one
        // apart: only the dot apart takes a circle.
        let held: &[(f64, f64)] = &[(12.0, 12.0), (20.0, 12.0)];
        let dots: [&[(f64, f64)]; 3] = [
            &[(12.0, 12.0), (12.0, 12.0)],
            &[(20.0, 12.0), (20.0, 12.0)],
            &[(30.0, 30.0), (30.0, 30.0)],
        ];
        let stroke = Stroke {
            width: 3.0,
            cap: LineCap::Round,
            ..Stroke::default()
        };
        let outline = |parts: &[&[(f64, f64)]]| {
            pieces(
                &subpaths(parts),
                Transform::IDENTITY,
                &stroke,
                &Limits::default(),
                (Tiling::Unproven, TOLERANCE),
                &mut crate::testing::bytes(),
            )
            .unwrap()
        };
        let (alone, _, tiling) = outline(&[held]);
        assert_eq!(tiling, Tiling::Proven);
        let circle = outline(&[dots[2]]).0.segments().len();
        let parts = [dots[0], held, dots[1], dots[2]];
        let (meeting, _, _) = outline(&parts);
        assert_eq!(meeting.segments().len(), alone.segments().len() + circle);
        assert_eq!(
            pieces_union_difference(&subpaths(&parts), Transform::IDENTITY, &stroke),
            0
        );
    }

    #[test]
    fn asked_circles_are_admitted_before_they_wait() {
        let runs: Vec<_> = (0..4_u32)
            .map(|i| super::super::Run {
                points: vec![Point::new(0.0, 0.0), Point::new(f64::from(i), 9.0)],
                closed: false,
                tangent: Point::new(1.0, 0.0),
                caps: [None; 2],
                joins: Vec::new(),
            })
            .collect();
        let stroke = Stroke {
            cap: LineCap::Round,
            ..Stroke::default()
        };
        let shared = 8 * size_of::<super::Meeting>();
        let ask = |bytes: usize| {
            let mut meter = pagura_core::Meter::new(bytes, crate::band::RENDER_BYTES);
            let mut chains =
                super::Chains::new(&runs, &stroke, &Limits::default(), &mut 0, &mut meter).unwrap();
            // The four runs share their first point only.
            assert_eq!(chains.shared.len(), 1);
            let point = Point::new(0.0, 0.0);
            let asked = chains.circle(point, &mut meter);
            asked.map(|draw| {
                (
                    draw,
                    chains.circle(point, &mut meter).unwrap(),
                    meter.used(),
                )
            })
        };
        let room = shared + size_of::<Point>();
        // Asked once, it waits; asked again, nothing more is charged.
        assert_eq!(ask(room).unwrap(), (false, false, room));
        assert_eq!(
            ask(room - 1).unwrap_err().message,
            crate::band::RENDER_BYTES
        );
    }

    #[test]
    fn meeting_ends_are_admitted_before_they_are_sorted() {
        // 40 runs whose ends meet at 12 points, two runs or more at each.
        let runs: Vec<_> = (0..40_u32)
            .map(|i| super::super::Run {
                points: vec![
                    Point::new(f64::from(i % 7), 0.0),
                    Point::new(f64::from(i % 5), 9.0),
                ],
                closed: false,
                tangent: Point::new(1.0, 0.0),
                caps: [None; 2],
                joins: Vec::new(),
            })
            .collect();
        let stroke = Stroke {
            cap: LineCap::Round,
            ..Stroke::default()
        };
        let new = |bytes: usize, raster_work: usize| {
            let limits = Limits {
                raster_work,
                ..Limits::default()
            };
            let mut meter = pagura_core::Meter::new(bytes, crate::band::RENDER_BYTES);
            super::Chains::new(&runs, &stroke, &limits, &mut 0, &mut meter)
                .map(|chains| (meter.used(), chains.heap_bytes(), chains.shared.len()))
        };
        let size = 80 * size_of::<super::Meeting>();
        // 80 ends, sorted in at most 80 (log2 80 + 1) comparisons.
        let work = 80 * 7;
        assert_eq!(new(size, work).unwrap(), (size, size, 12));
        let refused = new(size - 1, work).unwrap_err();
        assert_eq!(refused.message, crate::band::RENDER_BYTES);
        let refused = new(size, work - 1).unwrap_err();
        assert_eq!(refused.message, crate::band::RASTER_WORK);
    }
}
