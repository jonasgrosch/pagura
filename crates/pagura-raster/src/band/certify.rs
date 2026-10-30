//! The winding certificate that lets a shape's cells take their signed area.
//!
//! A cell's signed area is its covered area under either fill rule when the
//! winding number is 0 or one common sign of magnitude one everywhere off the
//! boundary. Each contour splits into strictly y-monotone chains; horizontal
//! edges bound no area. Between consecutive chain end heights the active
//! chains must keep their horizontal order and their directions must
//! alternate from the left, starting with the common sign. The first meeting
//! inside such a slab would be between chains adjacent at its middle
//! (Shamos and Hoey), so only neighbours are compared, at every vertex height
//! of either. Comparisons keep `margin` plus their rounding error; only
//! identical vertices may touch, and only at a slab's ends. Any doubt leaves
//! the shape uncertified, and its cells are analysed one by one.
//!
//! A doubtful slab across row boundaries is checked again row by row, as
//! slabs of their own, so that only the rows holding the doubt need their
//! cells analysed.
//!
//! The sweep's steps are capped at 32 per vertex plus 1024, and the rows'
//! checks as many again, so certifying costs time linear in the vertices
//! beyond the sorts.

use super::curve::{Mono, Piece, Spline};
use super::edges::Polygon;
use crate::Point;
use crate::geometry::{greater, lesser};
use pagura_core::{Meter, Result};
use std::cmp::Ordering;

mod cover;
pub(crate) use cover::{Covered, covered};

#[derive(Debug, Clone, Copy)]
struct Chain {
    start: usize,
    end: usize,
    up: bool,
}

#[derive(Debug, Clone, Copy)]
struct Active {
    chain: Chain,
    segment: usize,
    x: f64,
    range: [f64; 2],
}

/// Whether the winding of `polygon` is 0 or one common sign of magnitude one
/// everywhere, with neighbouring chains at least `margin` apart where they
/// do not share a vertex. Point cells participate as their emitted square
/// contours, with the same winding requirements as every other contour.
#[cfg(test)]
pub(crate) fn certified(polygon: &Polygon, margin: f64, bytes: &mut Meter) -> Result<bool> {
    let before = bytes.used();
    let result = uncertified(polygon, margin, bytes)
        .map(|slabs| slabs.is_some_and(|slabs| slabs.is_empty()));
    bytes.refund(bytes.used().saturating_sub(before));
    result
}

/// The heights `[lo, hi]` of the slabs the certificate cannot certify, merged
/// where they meet; every other slab holds winding 0 or the common sign.
/// `None` when vertex indexing or the sweep's work bound is exceeded.
/// Byte exhaustion is a typed error from `bytes`.
pub(crate) fn uncertified(
    polygon: &Polygon,
    margin: f64,
    bytes: &mut Meter,
) -> Result<Option<Vec<[f64; 2]>>> {
    Ok(doubtful(polygon, margin, bytes)?.map(|doubtful| doubtful.slabs))
}

/// The slabs [`uncertified`] finds and the common sign of the others.
pub(crate) struct Doubtful {
    pub(crate) slabs: Vec<[f64; 2]>,
    /// Whether the leftmost chain of every certified slab runs in
    /// increasing y, so that their winding is 0 or 1 rather than 0 or -1.
    pub(crate) positive: bool,
}

/// [`uncertified`] with the common sign.
pub(crate) fn doubtful(
    polygon: &Polygon,
    margin: f64,
    bytes: &mut Meter,
) -> Result<Option<Doubtful>> {
    let before = bytes.used();
    let result = certificate(polygon, margin, bytes);
    let retained = result
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map_or(0, |doubtful| {
            doubtful
                .slabs
                .capacity()
                .saturating_mul(size_of::<[f64; 2]>())
        });
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    result
}

fn certificate(polygon: &Polygon, margin: f64, bytes: &mut Meter) -> Result<Option<Doubtful>> {
    if polygon.points.len() > u32::MAX as usize {
        return Ok(None);
    }
    let mut points = Vec::new();
    let mut chains = Vec::new();
    let mut magnitude = [1.0_f64; 2];
    for contour in polygon.contours() {
        for p in contour {
            magnitude = [magnitude[0].max(p.x.abs()), magnitude[1].max(p.y.abs())];
        }
        split(contour, &mut points, &mut chains, bytes)?;
    }
    let magnitude = magnitude[0].max(magnitude[1]);
    let cap = polygon.vertices().saturating_mul(32).saturating_add(1024);
    let mut work = Work { done: 0, cap };
    let mut refine = Work { done: 0, cap };
    // Interpolation errs by at most 11 units of roundoff of the largest
    // coordinate, so a difference of two such values by 22; keep 2^8 of them.
    let margins = Margins {
        rounding: margin + magnitude * 2f64.powi(-45),
        vertices: margin,
    };
    let mut failing = Vec::new();
    let Some(positive) = sweep(
        &points,
        &mut chains,
        margins,
        (&mut work, &mut refine),
        &mut failing,
        bytes,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(Doubtful {
        slabs: failing,
        positive,
    }))
}

/// The separation chains need: `rounding` between interpolated positions,
/// `vertices` between vertices (0 lets identical vertices touch at a slab's
/// ends).
#[derive(Debug, Clone, Copy)]
struct Margins {
    rounding: f64,
    vertices: f64,
}

#[derive(Debug)]
struct Work {
    done: usize,
    cap: usize,
}

impl Work {
    fn add(&mut self, steps: usize) -> Option<()> {
        self.done = self.done.saturating_add(steps);
        (self.done <= self.cap).then_some(())
    }
}

fn sweep(
    points: &[Point],
    chains: &mut [Chain],
    margins: Margins,
    (work, refine): (&mut Work, &mut Work),
    failing: &mut Vec<[f64; 2]>,
    bytes: &mut Meter,
) -> Result<Option<bool>> {
    let mut heights = Vec::new();
    bytes.reserve(&mut heights, chains.len().saturating_mul(2))?;
    heights.extend(
        chains
            .iter()
            .flat_map(|c| [points[c.start].y, points[c.end - 1].y]),
    );
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    chains.sort_unstable_by(|a, b| points[a.start].y.total_cmp(&points[b.start].y));
    let mut active: Vec<Active> = Vec::new();
    bytes.reserve(&mut active, chains.len())?;
    let mut next = 0;
    let mut sign = None;
    for pair in heights.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        active.retain(|a| points[a.chain.end - 1].y > lo);
        while let Some(&chain) = chains.get(next).filter(|c| points[c.start].y <= lo) {
            active.push(Active {
                chain,
                segment: chain.start,
                x: 0.0,
                range: [0.0; 2],
            });
            next += 1;
        }
        if active.is_empty() {
            continue;
        }
        if place(points, &mut active, (lo, hi), work).is_none() {
            return Ok(None);
        }
        let first = *sign.get_or_insert(active.first().is_some_and(|a| a.chain.up));
        let held = holds(points, &active, (lo, hi), first, margins, work);
        if work.add(0).is_none() {
            return Ok(None);
        }
        if held {
            continue;
        }
        // A doubtful slab across row boundaries may leave some of its rows
        // certain: each row's part is a slab of its own, checked on a budget
        // of its own. Parts past that budget stay in doubt.
        let rows = lo.floor() + 1.0 < hi;
        let mut from = lo;
        while from < hi {
            let mut to = if rows {
                (from.floor() + 1.0).min(hi)
            } else {
                hi
            };
            let held = rows
                && to > from
                && place(points, &mut active, (from, to), refine).is_some()
                && holds(points, &active, (from, to), first, margins, refine)
                && refine.add(0).is_some();
            if !held {
                // Stop walking the slab when refinement cannot make progress
                // or its budget is spent; keep all remaining rows doubtful.
                if to <= from || refine.add(0).is_none() {
                    to = hi;
                }
                match failing.last_mut() {
                    Some(last) if last[1] >= from => last[1] = to,
                    _ => {
                        bytes.reserve_capacity(failing, 1)?;
                        failing.push([from, to]);
                    }
                }
            }
            from = to;
        }
    }
    Ok(Some(sign.unwrap_or(true)))
}

/// Places the active chains on the slab (lo, hi): from the segments holding
/// `lo`, each chain's x at the middle and its x range over the slab, in
/// order of those x. Counts its steps on `work`; `None` past the cap.
fn place(
    points: &[Point],
    active: &mut [Active],
    (lo, hi): (f64, f64),
    work: &mut Work,
) -> Option<()> {
    work.add(active.len())?;
    let mid = lo.midpoint(hi);
    for a in active.iter_mut() {
        // A step per segment passed, counted once the chain is walked.
        let passed = a.segment;
        while points[a.segment + 1].y <= lo {
            a.segment += 1;
        }
        let mut segment = a.segment;
        let start = at(points, segment, lo).0;
        let mut range = [start, start];
        let mut walk = |to: f64| {
            while points[segment + 1].y < to {
                segment += 1;
                range = [
                    lesser(range[0], points[segment].x),
                    greater(range[1], points[segment].x),
                ];
            }
            segment
        };
        a.x = at(points, walk(mid), mid).0;
        let end = at(points, walk(hi), hi).0;
        work.add(segment - passed)?;
        a.range = [lesser(range[0], end), greater(range[1], end)];
    }
    order(active);
    Some(())
}

/// Puts the placed chains in order: of x, and chains at the same x by their
/// first point. The chains kept from the last slab mostly keep its order
/// and the joining ones sit at the end, so each chain out of order moves
/// into place by binary search; past twice as many chains moved as there
/// are, the sort takes over. The order is total, so both give the same.
///
/// Chains at the same x meet at the slab's middle. Unless they are the same
/// segment across the whole slab they are not apart, so the slab is in doubt
/// in either order: the certificate is sound for any order of them, as it
/// was for the one the sort used to leave to chance.
#[inline(never)]
fn order(active: &mut [Active]) {
    let mut moved = 0;
    for i in 1..active.len() {
        if before(&active[i - 1], &active[i]).is_lt() {
            continue;
        }
        let to = active[..i].partition_point(|a| before(a, &active[i]).is_lt());
        moved += i - to;
        if moved > 2 * active.len() {
            active.sort_unstable_by(before);
            return;
        }
        active[to..=i].rotate_right(1);
    }
}

/// The order of placed chains: chains take points of their own, so no two
/// share their first.
fn before(a: &Active, b: &Active) -> Ordering {
    a.x.total_cmp(&b.x).then(a.chain.start.cmp(&b.chain.start))
}

/// Whether the placed chains hold winding 0 or the common sign on the slab
/// (lo, hi): directions alternating from the left starting with `first`,
/// and neighbours apart. Each slab stands alone: its order, directions and
/// separation decide its own winding, whatever another slab holds.
fn holds(
    points: &[Point],
    active: &[Active],
    (lo, hi): (f64, f64),
    first: bool,
    margins: Margins,
    work: &mut Work,
) -> bool {
    let ordered = active.len().is_multiple_of(2)
        && active
            .iter()
            .enumerate()
            .all(|(k, a)| a.chain.up == (first ^ (k % 2 == 1)));
    ordered
        && active.windows(2).all(|w| {
            w[0].range[1] + margins.rounding < w[1].range[0]
                || apart(
                    points,
                    (w[0].segment, w[1].segment),
                    (lo, hi),
                    margins,
                    work,
                )
        })
}

// Appends the strictly y-monotone chains of one closed contour, each in
// increasing y. A chain begins where the direction changes, so a cyclic walk
// from such an edge never splits one.
fn split(
    vertices: &[Point],
    points: &mut Vec<Point>,
    chains: &mut Vec<Chain>,
    bytes: &mut Meter,
) -> Result<()> {
    let Some(&last) = vertices.last() else {
        return Ok(());
    };
    let n = vertices.len();
    let next = |i: usize| if i + 1 == n { 0 } else { i + 1 };
    let direction = |a: Point, b: Point| i8::from(a.y < b.y) - i8::from(a.y > b.y);
    // One pass finds the first side that starts a chain and counts the
    // points the chains take: one per side with height, one more per chain.
    let (mut previous, mut first, mut count) = (direction(last, vertices[0]), None, 0_usize);
    let mut tally = |i: usize, d: i8| {
        if d != 0 {
            let starts = d != previous;
            first = first.or(starts.then_some(i));
            count += 1 + usize::from(starts);
        }
        previous = d;
    };
    for (i, side) in vertices.windows(2).enumerate() {
        tally(i, direction(side[0], side[1]));
    }
    tally(n - 1, direction(last, vertices[0]));
    let Some(mut i) = first else {
        return Ok(());
    };
    bytes.reserve_capacity(points, count)?;
    // Each run of sides with one direction is a chain of its vertices.
    let mut left = n;
    while left > 0 {
        let d = direction(vertices[i], vertices[next(i)]);
        let mut end = next(i);
        left -= 1;
        if d == 0 {
            i = end;
            continue;
        }
        while left > 0 && direction(vertices[end], vertices[next(end)]) == d {
            end = next(end);
            left -= 1;
        }
        let start = points.len();
        if i < end {
            points.extend_from_slice(&vertices[i..=end]);
        } else {
            points.extend_from_slice(&vertices[i..]);
            points.extend_from_slice(&vertices[..=end]);
        }
        if d < 0 {
            points[start..].reverse();
        }
        bytes.reserve_capacity(chains, 1)?;
        chains.push(Chain {
            start,
            end: points.len(),
            up: d > 0,
        });
        i = end;
    }
    Ok(())
}

// x on segment `i` at height y, and whether it is a vertex value.
fn at(points: &[Point], i: usize, y: f64) -> (f64, bool) {
    let (a, b) = (points[i], points[i + 1]);
    if y == a.y {
        (a.x, true)
    } else if y == b.y {
        (b.x, true)
    } else {
        (a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x), false)
    }
}

// Whether the right chain stays strictly right of the left one on the open
// slab (lo, hi) within the work cap. Segments start at the ones containing lo.
fn apart(
    points: &[Point],
    (mut i, mut j): (usize, usize),
    (lo, hi): (f64, f64),
    margins: Margins,
    work: &mut Work,
) -> bool {
    // A value `at` gives lies within its segment's x range widened by
    // 4ε·magnitude, far below `rounding`. Where the current segments' ranges
    // stay twice `rounding` apart, the comparison below would pass, as the
    // margin is not negative.
    let clear = 2.0 * margins.rounding;
    let mut y = lo;
    let (mut ends, mut steps) = ([points[i + 1].y, points[j + 1].y], 0);
    let separated = loop {
        let left = greater(points[i].x, points[i + 1].x);
        if left + clear >= lesser(points[j].x, points[j + 1].x) {
            let (left, exact_left) = at(points, i, y);
            let (right, exact_right) = at(points, j, y);
            let d = right - left;
            let separated = if !(exact_left && exact_right) {
                d > margins.rounding
            } else if y == lo || y == hi {
                // Identical vertices may touch at a slab's ends; distinct
                // ones keep the margin.
                d == 0.0 || d > margins.vertices.max(-0.0)
            } else {
                d > margins.vertices
            };
            if !separated {
                break false;
            }
        }
        if y == hi {
            break true;
        }
        // Past `lo`, `y` is the lower segment end: the segments ending
        // there pass it.
        if y > lo {
            let [a, b] = ends;
            (i, j) = (i + usize::from(a <= b), j + usize::from(b <= a));
            ends = [points[i + 1].y, points[j + 1].y];
        }
        steps += 1;
        y = ends[0].min(ends[1]).min(hi);
    };
    // Past the cap, the certificate fails whatever this pair holds.
    work.add(steps).is_some() && separated
}

/// One y-monotone piece of a spline chain, oriented downward (`a.y <
/// b.y`): a line, or a monotone quadratic, by its index in the spline's
/// curves.
#[derive(Debug, Clone, Copy)]
struct Down {
    curve: Option<usize>,
    a: Point,
    b: Point,
}

impl Down {
    /// The piece oriented downward, and whether it ran downward; a curve is
    /// the spline's curve `curve`.
    fn new(piece: Piece, curve: usize) -> Option<(Self, bool)> {
        let (curve, a, b) = match piece {
            Piece::Line(line) => (None, line.a, line.b),
            Piece::Curve(q) => (Some(curve), q.points[0], q.points[2]),
        };
        if a.y == b.y {
            return None;
        }
        let down = a.y < b.y;
        let (a, b) = if down { (a, b) } else { (b, a) };
        Some((Self { curve, a, b }, down))
    }

    /// x at height `y` within the piece, and whether it is an end's value.
    fn x(&self, curves: &[Mono], y: f64) -> (f64, bool) {
        if y <= self.a.y {
            return (self.a.x, y == self.a.y);
        }
        if y >= self.b.y {
            return (self.b.x, y == self.b.y);
        }
        match self.curve {
            None => (
                self.a.x + (y - self.a.y) / (self.b.y - self.a.y) * (self.b.x - self.a.x),
                false,
            ),
            Some(c) => (curves[c].x_at(y), false),
        }
    }

    /// The directions bounding the piece seen from its end at height `v`,
    /// away from it: toward its control (or along it) and along its chord
    /// to height `far`.
    fn fan(&self, curves: &[Mono], v: f64, far: f64) -> [Point; 2] {
        let vertex = if v == self.a.y { self.a } else { self.b };
        let end = Point::new(self.x(curves, far).0, far);
        let chord = Point::new(end.x - vertex.x, end.y - vertex.y);
        let control = self.curve.map_or(chord, |c| {
            let c = curves[c].control();
            Point::new(c.x - vertex.x, c.y - vertex.y)
        });
        [control, chord]
    }
}

/// A chain of a spline: its pieces in `downs`, and its direction.
#[derive(Debug, Clone, Copy)]
struct Run {
    start: usize,
    end: usize,
    down: bool,
}

/// Whether `spline`'s winding is 0 or one common sign of magnitude one
/// everywhere, by the sweep of [`uncertified`] over chains of monotone pieces:
/// neighbours are compared by the x ranges of monotone sub-pieces, halved
/// where they overlap, and at a shared end by the fans of their control and
/// chord directions, which meet only at the end when strictly ordered.
/// Steps are capped as the polygon's are; doubt leaves it uncertified.
pub(crate) fn spline_certified(
    spline: &Spline,
    curves: &[Mono],
    bytes: &mut Meter,
) -> Result<bool> {
    let before = bytes.used();
    let result = spline_sweep(spline, curves, bytes).map(|ok| ok.is_some_and(|ok| ok));
    bytes.refund(bytes.used().saturating_sub(before));
    result
}

fn spline_runs(spline: &Spline, bytes: &mut Meter) -> Result<(Vec<Down>, Vec<Run>, f64)> {
    let mut downs: Vec<Down> = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    let count = spline
        .pieces
        .iter()
        .filter(|p| {
            let [a, b] = p.ends();
            a.y != b.y
        })
        .count();
    bytes.reserve(&mut downs, count)?;
    let (mut magnitude, mut curve) = (1.0_f64, 0);
    for contour in spline.contours() {
        split_pieces(
            contour,
            &mut curve,
            &mut downs,
            &mut runs,
            &mut magnitude,
            bytes,
        )?;
    }
    Ok((downs, runs, magnitude))
}

fn spline_sweep(spline: &Spline, curves: &[Mono], bytes: &mut Meter) -> Result<Option<bool>> {
    let (downs, runs, magnitude) = spline_runs(spline, bytes)?;
    let mut work = Work {
        done: 0,
        cap: spline.pieces.len().saturating_mul(32).saturating_add(1024),
    };
    // Root solving errs by a few units of roundoff of the largest
    // coordinate; keep 2^13 of them.
    let margin = magnitude * 2f64.powi(-40);
    let top = |r: &Run| downs[r.start].a.y;
    let bottom = |r: &Run| downs[r.end - 1].b.y;
    let mut heights: Vec<f64> = Vec::new();
    bytes.reserve(&mut heights, runs.len().saturating_mul(2))?;
    heights.extend(runs.iter().flat_map(|r| [top(r), bottom(r)]));
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    let mut order = Vec::new();
    bytes.reserve(&mut order, runs.len())?;
    order.extend(0..runs.len());
    order.sort_unstable_by(|&a, &b| top(&runs[a]).total_cmp(&top(&runs[b])));
    // Active runs: run, piece at the slab's top, x at mid-slab, x range.
    let mut active: Vec<(usize, usize, f64, [f64; 2])> = Vec::new();
    bytes.reserve(&mut active, runs.len())?;
    let mut next = 0;
    let mut sign = None;
    for pair in heights.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        active.retain(|a| bottom(&runs[a.0]) > lo);
        while let Some(&r) = order.get(next).filter(|&&r| top(&runs[r]) <= lo) {
            active.push((r, runs[r].start, 0.0, [0.0; 2]));
            next += 1;
        }
        if active.is_empty() {
            continue;
        }
        if work.add(active.len()).is_none() {
            return Ok(None);
        }
        let mid = lo.midpoint(hi);
        for a in &mut active {
            let end = runs[a.0].end;
            while a.1 + 1 < end && downs[a.1].b.y <= lo {
                a.1 += 1;
            }
            let mut k = a.1;
            let first = downs[k].x(curves, lo).0;
            let mut range = [first, first];
            loop {
                if work.add(1).is_none() {
                    return Ok(None);
                }
                let piece = &downs[k];
                if piece.a.y <= mid && mid <= piece.b.y {
                    a.2 = piece.x(curves, mid).0;
                }
                if piece.b.y >= hi || k + 1 >= end {
                    let x = piece.x(curves, hi).0;
                    range = [lesser(range[0], x), greater(range[1], x)];
                    break;
                }
                range = [lesser(range[0], piece.b.x), greater(range[1], piece.b.x)];
                k += 1;
            }
            a.3 = range;
        }
        active.sort_unstable_by(|a, b| a.2.total_cmp(&b.2));
        let first = *sign.get_or_insert(runs[active[0].0].down);
        let ordered = active.len().is_multiple_of(2)
            && active
                .iter()
                .enumerate()
                .all(|(k, a)| runs[a.0].down == (first ^ (k % 2 == 1)));
        if !ordered {
            return Ok(Some(false));
        }
        for w in active.windows(2) {
            if w[0].3[1] + margin < w[1].3[0] {
                continue;
            }
            let left = (w[0].1, runs[w[0].0].end);
            let right = (w[1].1, runs[w[1].0].end);
            match chains_apart(
                (&downs, curves),
                (left, right),
                (lo, hi),
                (margin, 12),
                &mut work,
            ) {
                Some(true) => {}
                Some(false) => return Ok(Some(false)),
                None => return Ok(None),
            }
        }
    }
    Ok(Some(true))
}

/// Appends the y-monotone chains of one closed contour of pieces, each
/// oriented downward. A chain begins where the direction changes, so a
/// cyclic walk from such a piece never splits one; horizontal pieces end
/// chains.
fn split_pieces(
    contour: &[Piece],
    curve: &mut usize,
    downs: &mut Vec<Down>,
    runs: &mut Vec<Run>,
    magnitude: &mut f64,
    bytes: &mut Meter,
) -> Result<()> {
    let n = contour.len();
    // The spline's index of the contour's first curve; `curve` moves past
    // its last.
    let curved = |p: &Piece| usize::from(matches!(p, Piece::Curve(_)));
    let first_curve = *curve;
    *curve += contour.iter().map(curved).sum::<usize>();
    let direction = |p: &Piece| {
        let [a, b] = p.ends();
        if a.y == b.y {
            0
        } else if a.y < b.y {
            1
        } else {
            -1
        }
    };
    // A contour without height bounds no area.
    let Some(first) = (0..n).find(|&i| {
        let d = direction(&contour[i]);
        d != 0 && direction(&contour[(i + n - 1) % n]) != d
    }) else {
        return Ok(());
    };
    let mut current: Option<(usize, bool)> = None;
    let close = |current: &mut Option<(usize, bool)>,
                 downs: &mut Vec<Down>,
                 runs: &mut Vec<Run>,
                 bytes: &mut Meter|
     -> Result<()> {
        if let Some((start, down)) = current.take() {
            if !down {
                debug_assert!(start <= downs.len());
                downs[start..].reverse();
            }
            bytes.reserve_capacity(runs, 1)?;
            runs.push(Run {
                start,
                end: downs.len(),
                down,
            });
        }
        Ok(())
    };
    let mut next = first_curve + contour[..first].iter().map(curved).sum::<usize>();
    for k in (first..n).chain(0..first) {
        let piece = contour[k];
        let index = if k == 0 { first_curve } else { next };
        next = index + curved(&piece);
        let Some((down_piece, down)) = Down::new(piece, index) else {
            close(&mut current, downs, runs, bytes)?;
            continue;
        };
        *magnitude = magnitude
            .max(down_piece.a.x.abs())
            .max(down_piece.b.x.abs())
            .max(down_piece.a.y.abs())
            .max(down_piece.b.y.abs());
        if current.is_none_or(|(_, d)| d != down) {
            close(&mut current, downs, runs, bytes)?;
            current = Some((downs.len(), down));
        }
        bytes.reserve_capacity(downs, 1)?;
        downs.push(down_piece);
    }
    close(&mut current, downs, runs, bytes)
}

/// Whether the right chain stays right of the left one on the open slab
/// `(lo, hi)`, halving curved pieces up to `depth` times; each chain is
/// given by its piece at `lo` and its end.
fn chains_apart(
    (downs, curves): (&[Down], &[Mono]),
    ((mut i, ie), (mut j, je)): ((usize, usize), (usize, usize)),
    (lo, hi): (f64, f64),
    (margin, depth): (f64, u32),
    work: &mut Work,
) -> Option<bool> {
    let mut a = lo;
    while a < hi {
        while i + 1 < ie && downs[i].b.y <= a {
            i += 1;
        }
        while j + 1 < je && downs[j].b.y <= a {
            j += 1;
        }
        let b = downs[i].b.y.min(downs[j].b.y).min(hi);
        if b <= a {
            return Some(false);
        }
        if !pieces_apart(
            (&downs[i], &downs[j], curves),
            (a, b),
            (a == lo, b == hi),
            margin,
            work,
            depth,
        )? {
            return Some(false);
        }
        a = b;
    }
    Some(true)
}

/// Whether piece `right` stays right of piece `left` over `(a, b)`:
/// straight pieces by their ends, curved ones by the x ranges of their
/// monotone parts, halved up to `depth` times; at a shared end of the slab
/// by their fans.
fn pieces_apart(
    (left, right, curves): (&Down, &Down, &[Mono]),
    (a, b): (f64, f64),
    ends: (bool, bool),
    margin: f64,
    work: &mut Work,
    depth: u32,
) -> Option<bool> {
    work.add(1)?;
    let (la, left_a) = left.x(curves, a);
    let (lb, left_b) = left.x(curves, b);
    let (ra, right_a) = right.x(curves, a);
    let (rb, right_b) = right.x(curves, b);
    // Identical ends may touch at the slab's ends.
    let touch_a = ends.0 && left_a && right_a && la == ra;
    let touch_b = ends.1 && left_b && right_b && lb == rb;
    if left.curve.is_none() && right.curve.is_none() {
        return Some((touch_a || ra - la > margin) && (touch_b || rb - lb > margin));
    }
    if la.max(lb) + margin < ra.min(rb) {
        return Some(true);
    }
    if touch_a || touch_b {
        let (v, far) = if touch_a { (a, b) } else { (b, a) };
        let fans = (left.fan(curves, v, far), right.fan(curves, v, far));
        if fans_apart(fans.0, fans.1, far > v, margin) {
            return Some(true);
        }
    }
    if depth == 0 {
        return Some(false);
    }
    let m = a.midpoint(b);
    Some(
        pieces_apart(
            (left, right, curves),
            (a, m),
            (ends.0, false),
            margin,
            work,
            depth - 1,
        )? && pieces_apart(
            (left, right, curves),
            (m, b),
            (false, ends.1),
            margin,
            work,
            depth - 1,
        )?,
    )
}

/// Whether every direction of `left` lies strictly left of every direction
/// of `right`, both leaving a shared vertex downward (`down`) or upward:
/// directions on opposite sides of the vertical are ordered whatever their
/// slopes, others by their cross product beyond its rounding.
fn fans_apart(left: [Point; 2], right: [Point; 2], down: bool, margin: f64) -> bool {
    left.iter().all(|u| {
        right.iter().all(|w| {
            if u.x <= 0.0 && w.x >= 0.0 && (u.x < 0.0 || w.x > 0.0) {
                return true;
            }
            let cross = u.x * w.y - w.x * u.y;
            let scale = (u.x.abs() + u.y.abs()) * (w.x.abs() + w.y.abs());
            if down {
                cross < -margin * scale
            } else {
                cross > margin * scale
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::Limits;

    fn certifies(polygon: &Polygon, margin: f64) -> bool {
        certified(polygon, margin, &mut crate::testing::bytes()).unwrap()
    }

    fn polygon(contours: &[&[(f64, f64)]]) -> Polygon {
        let mut polygon = Polygon::default();
        for contour in contours {
            polygon
                .begin(
                    &Limits::default(),
                    contour.len(),
                    &mut crate::testing::bytes(),
                )
                .unwrap();
            polygon
                .points
                .extend(contour.iter().map(|&(x, y)| Point::new(x, y)));
            polygon.end();
        }
        polygon
    }

    #[test]
    fn simple_and_nested_opposite_contours_certify() {
        let square = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let hole = [(1.0, 1.0), (1.0, 3.0), (3.0, 3.0), (3.0, 1.0)];
        assert!(certifies(&polygon(&[&square]), 0.0));
        assert!(certifies(&polygon(&[&square, &hole]), 0.0));
    }

    #[test]
    fn overlaps_crossings_and_nested_same_direction_do_not() {
        let square = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let inner = [(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)];
        let bowtie = [(0.0, 0.0), (4.0, 4.0), (4.0, 0.0), (0.0, 4.0)];
        assert!(!certifies(&polygon(&[&square, &inner]), 0.0));
        assert!(!certifies(&polygon(&[&bowtie]), 0.0));
    }

    #[test]
    fn a_margin_rejects_close_chains() {
        let a = [(0.0, 0.0), (1.0, 0.0), (1.0, 4.0), (0.0, 4.0)];
        let b = [(1.001, 0.0), (2.0, 0.0), (2.0, 4.0), (1.001, 4.0)];
        assert!(certifies(&polygon(&[&a, &b]), 0.0));
        assert!(!certifies(&polygon(&[&a, &b]), 1.0 / 128.0));
    }

    #[test]
    fn a_doubtful_slab_keeps_only_the_rows_of_its_overlap() {
        // Both contours span one slab from 0 to 10. The parallelogram's left
        // side meets the rectangle's right side at y = 5.5 and runs inside
        // it from there: rows 0 to 4 hold winding 0 or the common sign.
        let rectangle = [(0.0, 0.0), (5.0, 0.0), (5.0, 10.0), (0.0, 10.0)];
        let slanted = [(6.1, 0.0), (8.0, 0.0), (6.0, 10.0), (4.1, 10.0)];
        let slabs = uncertified(
            &polygon(&[&rectangle, &slanted]),
            0.0,
            &mut crate::testing::bytes(),
        )
        .unwrap();
        assert_eq!(slabs, Some(vec![[5.0, 10.0]]));
    }

    #[test]
    fn doubtful_refinement_stops_at_its_budget_or_an_unrepresentable_row() {
        for (lo, hi) in [
            (0.0, 1e12),
            (-2f64.powi(54), 10.0),
            (2f64.powi(53), 2f64.powi(53) + 8.0),
        ] {
            let rectangle = [(0.0, lo), (5.0, lo), (5.0, hi), (0.0, hi)];
            let slabs = uncertified(
                &polygon(&[&rectangle, &rectangle]),
                0.0,
                &mut crate::testing::bytes(),
            )
            .unwrap();
            assert_eq!(slabs, Some(vec![[lo, hi]]));
        }
    }

    #[test]
    fn polygon_certificate_scratch_shares_the_source_allowance() {
        let square = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let polygon = polygon(&[&square]);
        let source = polygon.heap_bytes();
        let mut bytes = Meter::new(
            source + 4 * size_of::<Point>() + size_of::<Chain>() - 1,
            "certificate bytes",
        );
        bytes.charge(source).unwrap();
        let error = uncertified(&polygon, 0.0, &mut bytes).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (pagura_core::ErrorKind::Limit, "certificate bytes")
        );
        assert_eq!(bytes.used(), source);
        assert_eq!(polygon.points.len(), 4);
        let mut enough = crate::testing::bytes();
        enough.charge(source).unwrap();
        assert!(certified(&polygon, 0.0, &mut enough).unwrap());
        assert_eq!(enough.used(), source);
    }

    #[test]
    fn spline_certificate_checks_storage_before_copying_pieces() {
        let mut path = crate::PathBuilder::new();
        path.rect(0.0, 0.0, 4.0, 4.0);
        let mut construction = crate::testing::bytes();
        let spline = super::super::curve::spline(
            &path.finish(),
            crate::Transform::IDENTITY,
            &Limits::default(),
            &mut construction,
        )
        .unwrap()
        .unwrap();
        let source = spline.retained_bytes();
        let mut bytes = Meter::new(source + 2 * size_of::<Down>() - 1, "certificate bytes");
        bytes.charge(source).unwrap();
        assert_eq!(
            spline_certified(&spline, &[], &mut bytes)
                .unwrap_err()
                .message,
            "certificate bytes"
        );
        assert_eq!(bytes.used(), source);
        assert!(spline_certified(&spline, &[], &mut construction).unwrap());
        assert_eq!(construction.used(), source);
    }

    #[test]
    fn point_cells_certify_only_their_actual_contour_winding() {
        let limits = Limits::default();
        let mut bytes = crate::testing::bytes();
        let mut cells = Polygon::default();
        cells
            .cell(Point::new(-1.2, 0.3), &limits, &mut bytes)
            .unwrap();
        assert!(certifies(&cells, 0.0));
        cells
            .cell(Point::new(3.8, 0.9), &limits, &mut bytes)
            .unwrap();
        assert!(certifies(&cells, 0.0));
        // Coincident cells have winding two and must still resolve.
        cells
            .cell(Point::new(3.2, 0.1), &limits, &mut bytes)
            .unwrap();
        assert!(!certifies(&cells, 0.0));
        let mut tight = Meter::new(cells.heap_bytes(), "point certificate bytes");
        tight.charge(cells.heap_bytes()).unwrap();
        assert_eq!(
            uncertified(&cells, 0.0, &mut tight).unwrap_err().message,
            "point certificate bytes"
        );
        assert_eq!(tight.used(), cells.heap_bytes());

        let opposite = [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
        let mut cancelled = polygon(&[&opposite]);
        cancelled
            .cell(Point::new(0.2, 0.7), &limits, &mut bytes)
            .unwrap();
        assert!(!certifies(&cancelled, 0.0));
    }

    /// The fields of a placed chain, `x` by its bits.
    fn key(a: &Active) -> (usize, usize, bool, usize, u64, [u64; 2]) {
        let (c, r) = (a.chain, a.range);
        (
            c.start,
            c.end,
            c.up,
            a.segment,
            a.x.to_bits(),
            r.map(f64::to_bits),
        )
    }

    #[test]
    fn ordering_placed_chains_matches_the_sort() {
        // Kept chains in order of x alone with a few neighbours swapped, then
        // joining chains, on a coarse grid of x so that ties are common;
        // coincident chains of opposite directions share x and range, and -0
        // and +0 differ in their bits. Every chain has a start of its own.
        let mut state = 0x2026_u64;
        let mut next = |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(state >> 33).unwrap() % bound
        };
        let real = |n: usize| f64::from(u32::try_from(n).unwrap());
        let grids = [0.0, -0.0, 0.25, 1.0, 1.5, 3.0, 7.75];
        for case in 0..6_000 {
            let len = [2, 5, 17, 64, 300][case % 5] + next(8);
            let coarse = case % 3 == 0;
            let mut active: Vec<Active> = (0..len)
                .map(|k| {
                    let x = if coarse {
                        grids[next(grids.len())]
                    } else {
                        real(next(1 << 20)) / 64.0
                    };
                    let chain = Chain {
                        start: k,
                        end: k + 1 + next(4),
                        up: next(2) == 0,
                    };
                    Active {
                        chain,
                        segment: k,
                        x,
                        range: [x - real(next(3)), x + real(next(3))],
                    }
                })
                .collect();
            let joining = next(1 + len / [1, 4, 16, 64][case % 4]);
            let kept = len - joining.min(len);
            active[..kept].sort_unstable_by(|a, b| a.x.total_cmp(&b.x));
            for _ in 0..[0, 0, 1, 3, 40][case / 5 % 5] {
                let k = next(len);
                if k + 1 < kept {
                    active.swap(k, k + 1);
                }
            }
            if case % 7 == 0 && len > 2 {
                let (k, at) = (next(len), next(len));
                let twin = Chain {
                    start: len + at,
                    end: len + at + 1,
                    up: !active[k].chain.up,
                };
                active[at] = Active {
                    chain: twin,
                    ..active[k]
                };
            }
            let mut sorted = active.clone();
            sorted.sort_by(before);
            order(&mut active);
            assert!(
                active.iter().map(key).eq(sorted.iter().map(key)),
                "case {case}"
            );
        }
    }
}
