//! Stroke pieces that meet without overlapping, for signed-area coverage.
//!
//! The component outline of [`super::draw`] overlaps itself inside every turn
//! and under every round cap, which only an exact union integration resolves.
//! Here each segment's quadrilateral ends, on the inner side of a turn, at the
//! joint's inner miter point (on the bisector of the turn) and, on the outer
//! side, at the segment's own normal. The outer wedge between two normals
//! takes the join. Round caps keep the component stroker's circle polygon:
//! its angular grid is part of the canonical outline. Those circles overlap
//! the adjacent pieces, so open round-capped runs remain unproven, except a
//! lone segment longer than the pen: its quadrilateral and the parts of its
//! circles beyond its ends make one convex outline.
//! Pieces that meet share their vertices bit for bit. Their nonzero filled
//! set is the components'; where proven, every point is covered once. A
//! reversal, or a turn tighter than the pen around a short segment, takes the components'
//! form instead, which overlaps inside the turn; such runs, and runs that
//! could bring the pen back over itself (an open centreline whose directions
//! span a half turn, a closed one that is not convex, and runs whose pens
//! may meet), are not proven to tile: their union is still the stroke, and
//! the rasterizer certifies or resolves its windings.

use super::{Pen, Run, cap, join, polygon, ring};
use crate::{LineCap, LineJoin, PathBuilder, Point, Stroke};
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

/// Appends the pieces of `run` to `out`, whose union is the components'; a
/// lone round-capped segment's capsule differs from it only by slivers of
/// rounding width (ADR 0004 §4). Returns whether no two pieces overlap: no joint overlaps and the
/// centreline is proven not to return within the pen of itself.
pub(super) fn append(
    out: &mut PathBuilder,
    run: &Run,
    pen: Pen,
    stroke: &Stroke,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<bool> {
    let before = bytes.used().saturating_sub(out.heap_bytes());
    let result = (|| {
        let Pen { radius, .. } = pen;
        let points = centreline(run, bytes)?;
        let [first, .., last] = points.as_slice() else {
            if let Some(&point) = points.first() {
                let tangent = run.tangent;
                let back = Point::new(-tangent.x, -tangent.y);
                cap(out, point, tangent, pen, stroke.cap, limits, bytes)?;
                cap(out, point, back, pen, stroke.cap, limits, bytes)?;
            }
            return Ok(stroke.cap != LineCap::Round);
        };
        let joints = Joints {
            closed: run.closed,
            segments: if run.closed {
                points.len()
            } else {
                points.len() - 1
            },
        };
        let segments = segments(&points, joints, bytes)?;
        if let (false, LineCap::Round, &[segment]) = (run.closed, stroke.cap, segments.as_slice())
            && capsule(out, (*first, *last), segment, pen, limits, bytes)?
        {
            return Ok(true);
        }
        let turns = turns(&segments, joints, radius, stroke.cap, bytes)?;
        let overlaps = turns.iter().any(|j| matches!(j, Joint::Overlap));
        let proven = !overlaps
            && advances(&segments, joints.closed)
            && (run.closed || stroke.cap != LineCap::Round);
        if out
            .len()
            .checked_add(segments.len().saturating_mul(7))
            .is_none_or(|n| n > limits.path_segments)
        {
            return Err(Error::limit("stroke outline budget exceeded"));
        }
        for (i, &(direction, _)) in segments.iter().enumerate() {
            let k = joints.after(i);
            let ends = ((points[i], turns[i]), (points[k], turns[k]));
            piece(out, ends, direction, radius, limits, bytes)?;
        }
        for (j, joint) in turns.iter().enumerate() {
            if let (Joint::Turn(_) | Joint::Overlap, Some((before, after))) =
                (joint, joints.around(j))
            {
                let tangents = [segments[before].0, segments[after].0];
                join(out, points[j], tangents, pen, stroke, limits, bytes)?;
            }
        }
        if let (false, [(start, _), ..], [.., (finish, _)]) =
            (run.closed, segments.as_slice(), segments.as_slice())
        {
            let back = Point::new(-start.x, -start.y);
            cap(out, *first, back, pen, stroke.cap, limits, bytes)?;
            cap(out, *last, *finish, pen, stroke.cap, limits, bytes)?;
        }
        Ok(proven)
    })();
    bytes.refund(
        bytes
            .used()
            .saturating_sub(before.saturating_add(out.heap_bytes())),
    );
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

/// The run's points without repeats, a closed run's closing point removed,
/// as [`super::draw`] strokes them.
pub(super) fn centreline(run: &Run, bytes: &mut Meter) -> Result<Vec<Point>> {
    let mut points = Vec::new();
    bytes.reserve(&mut points, run.points.len())?;
    for &point in &run.points {
        if points.last() != Some(&point) {
            points.push(point);
        }
    }
    if run.closed && points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    Ok(points)
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

/// The joint at each point: a turn where its inner miter point lies within
/// both of its segments, else an overlap. Beside a butt end nothing covers
/// what the neighbouring segment's quadrilateral reaches past it, so the end
/// segment must hold that too.
fn turns(
    segments: &[(Point, f64)],
    joints: Joints,
    radius: f64,
    cap: LineCap,
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
    // Each side of a segment must hold the inner reach of both its joints;
    // where it cannot, both overlap instead, which only lowers what the
    // segments beside them must hold.
    let last = joints.segments - 1;
    for (i, &(_, length)) in segments.iter().enumerate() {
        let butt = cap == LineCap::Butt && !joints.closed && (i == 0 || i == last);
        let ends = [i, joints.after(i)];
        for left in [false, true] {
            let need: f64 = ends
                .iter()
                .filter_map(|&j| turns[j].turn())
                .filter(|turn| turn.left == left)
                .map(|turn| {
                    if butt {
                        turn.reach.max(turn.span)
                    } else {
                        turn.reach
                    }
                })
                .sum();
            if need > length {
                for &j in &ends {
                    if turns[j].turn().is_some_and(|turn| turn.left == left) {
                        turns[j] = Joint::Overlap;
                    }
                }
            }
        }
    }
    Ok(turns)
}

/// Appends a lone round-capped segment from `p` to `q` as one convex
/// outline, its quadrilateral joined to the chain of each cap polygon beyond
/// its end line, and returns whether it did. That is the union of the
/// components where both caps stay within the segment behind their end
/// lines: the segment is longer than the pen, and every cap vertex on or
/// behind its end line lies across the quadrilateral. Vertices within eight
/// rounding units of their coordinates outside its sides, and the rounding
/// of the two crossings per end, move the boundary by slivers that narrow.
/// Admission also bounds that rounding after the pen-to-device map.
fn capsule(
    out: &mut PathBuilder,
    (p, q): (Point, Point),
    (direction, length): (Point, f64),
    pen: Pen,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<bool> {
    let radius = pen.radius;
    if radius <= 0.0 || length <= 2.0 * radius {
        return Ok(false);
    }
    // The cap/end-line crossings introduce new vertices in construction
    // space. Coordinate ulps can become visible under magnification, even
    // for a short device-space stroke. The pen tolerance already includes
    // the linear map's stretch; reserve only 1/4096 of it for these slivers.
    // Sixty-four units cover the dot products, interpolation and side slack.
    let magnitude = (p.x.abs() + p.y.abs()).max(q.x.abs() + q.y.abs()) + radius;
    if 64.0 * f64::EPSILON * magnitude > pen.tolerance / 4096.0 {
        return Ok(false);
    }
    let before = bytes.used().saturating_sub(out.heap_bytes());
    let result = (|| {
        let normal = Point::new(-direction.y * radius, direction.x * radius);
        let far = ring(q, pen, limits, bytes)?;
        let near = ring(p, pen, limits, bytes)?;
        let mut outline = Vec::new();
        bytes.reserve(&mut outline, far.len() + near.len() + 8)?;
        outline.extend([p - normal, q - normal]);
        if !beyond(&far, q, direction, radius, &mut outline) {
            return Ok(false);
        }
        outline.extend([q + normal, p + normal]);
        if !beyond(&near, p, -direction, radius, &mut outline) {
            return Ok(false);
        }
        polygon(out, &outline, limits, bytes)?;
        Ok(true)
    })();
    bytes.refund(
        bytes
            .used()
            .saturating_sub(before.saturating_add(out.heap_bytes())),
    );
    result
}

/// Appends the chain of the cap polygon `ring` around `end` beyond the end
/// line, `outward` its unit normal: where its sides cross the line from the
/// right, its vertices beyond, and where they cross back on the left.
/// Returns false unless that chain is one run, both crossings lie on the
/// quadrilateral's end, and every vertex on or behind the line lies across
/// the quadrilateral, of half-width `radius`.
fn beyond(
    ring: &[Point],
    end: Point,
    outward: Point,
    radius: f64,
    outline: &mut Vec<Point>,
) -> bool {
    let along = |v: Point| (v - end).dot(outward);
    let across = |v: Point| (v - end).dot(outward.perp());
    // Where the side from `v` to `w`, on opposite sides, crosses the line.
    let crossing = |v: Point, w: Point| v.lerp(w, along(v) / (along(v) - along(w)));
    // The vertices' own rounding, relative to their coordinates.
    let slack = 8.0 * f64::EPSILON * (end.x.abs() + end.y.abs() + radius);
    let Some(&back) = ring.last() else {
        return false;
    };
    let mut entry = None;
    let mut entries = 0_usize;
    let mut previous = back;
    for (i, &v) in ring.iter().enumerate() {
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
    let mut chain = ring.iter().cycle().skip(first);
    let Some(&first) = chain.next() else {
        return false;
    };
    let enter = crossing(behind, first);
    if across(enter) < -radius - slack || across(enter) >= 0.0 {
        return false;
    }
    outline.extend([enter, first]);
    let mut last = first;
    for &v in chain {
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

/// One segment's piece between its joints: the inner miter point on a
/// turn's inner side (through the joint itself), the normal elsewhere.
fn piece(
    out: &mut PathBuilder,
    ((p, start), (q, finish)): ((Point, Joint), (Point, Joint)),
    direction: Point,
    radius: f64,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<()> {
    let normal = Point::new(-direction.y * radius, direction.x * radius);
    let corner = |point: Point, joint: Joint, left: bool| match joint {
        Joint::Turn(turn) if turn.left == left => point + turn.offset,
        _ if left => point + normal,
        _ => point - normal,
    };
    let mut piece = [Point::new(0.0, 0.0); 6];
    let mut n = 0;
    let mut push = |point: Point| {
        piece[n] = point;
        n += 1;
    };
    // Through the joint itself, where the join's edges meet it.
    push(corner(p, start, false));
    if !matches!(start, Joint::Straight) {
        push(p);
    }
    push(corner(p, start, true));
    push(corner(q, finish, true));
    if !matches!(finish, Joint::Straight) {
        push(q);
    }
    push(corner(q, finish, false));
    polygon(out, &piece[..n], limits, bytes)
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
                assert_eq!(
                    tiling,
                    if round_caps {
                        Tiling::Components
                    } else {
                        Tiling::Proven
                    },
                    "{stroke:?}"
                );
                if round_caps {
                    assert_eq!(tiles.segments(), components.segments());
                }
                let difference = largest_difference(&exact(&tiles, pen), &exact(&components, pen));
                assert!(difference <= 1, "{stroke:?} {transform:?}: {difference}");
            }
        }
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
    fn round_cap_circles_keep_open_runs_unproven() {
        let limits = Limits::default();
        for (points, dash_array) in [
            (vec![(8.0, 8.0), (8.0, 8.0)], vec![]),
            (vec![(8.0, 8.0), (8.125, 8.0)], vec![]),
            (vec![(8.0, 8.0), (18.0, 8.0)], vec![0.0, 3.0]),
        ] {
            let path = polyline(&points, false);
            let stroke = Stroke {
                width: 2.0,
                cap: LineCap::Round,
                dash_array,
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
            let length = width * (1.01 + 2.0 * next());
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
}
