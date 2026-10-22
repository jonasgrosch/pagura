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
//! The sweep's steps are capped at 32 per vertex plus 1024, so certifying
//! costs time linear in the vertices beyond the sorts.

use super::curve::{Mono, Piece, Spline};
use super::edges::Polygon;
use crate::Point;

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

/// An upper bound of the polygon certificate's simultaneous vector
/// capacities: split points, chains, heights, active chains and failed
/// slabs. Geometric growth costs less than twice each maximum length;
/// eight spare entries also cover the minimum allocation capacities.
pub(crate) fn scratch_bytes(vertices: usize) -> usize {
    vertices.saturating_add(8).saturating_mul(
        4 * size_of::<Point>()
            + 2 * size_of::<Chain>()
            + 4 * size_of::<f64>()
            + 2 * size_of::<Active>()
            + 4 * size_of::<[f64; 2]>(),
    )
}

/// Whether the winding of `polygon` is 0 or one common sign of magnitude one
/// everywhere, with neighbouring chains at least `margin` apart where they
/// do not share a vertex. Geometric-point cells are never certified.
pub(crate) fn certified(polygon: &Polygon, margin: f64) -> bool {
    uncertified(polygon, margin).is_some_and(|slabs| slabs.is_empty())
}

/// The heights `[lo, hi]` of the slabs [`certified`] cannot certify, merged
/// where they meet; every other slab holds winding 0 or the common sign.
/// `None` when no slab is certified: geometric points, or the sweep's work
/// or storage ran out.
pub(crate) fn uncertified(polygon: &Polygon, margin: f64) -> Option<Vec<[f64; 2]>> {
    if polygon.points_cells || polygon.points.len() > u32::MAX as usize {
        return None;
    }
    let mut points = Vec::new();
    let mut chains = Vec::new();
    let mut magnitude = 1.0_f64;
    for contour in polygon.contours() {
        for p in contour {
            magnitude = magnitude.max(p.x.abs()).max(p.y.abs());
        }
        split(contour, &mut points, &mut chains)?;
    }
    let mut work = Work {
        done: 0,
        cap: polygon.vertices().saturating_mul(32).saturating_add(1024),
    };
    // Interpolation errs by at most 11 units of roundoff of the largest
    // coordinate, so a difference of two such values by 22; keep 2^8 of them.
    let margins = Margins {
        rounding: margin + magnitude * 2f64.powi(-45),
        vertices: margin,
    };
    let mut failing = Vec::new();
    sweep(&points, &mut chains, margins, &mut work, &mut failing)?;
    Some(failing)
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
    work: &mut Work,
    failing: &mut Vec<[f64; 2]>,
) -> Option<()> {
    let mut heights = Vec::new();
    heights.try_reserve(2 * chains.len()).ok()?;
    heights.extend(
        chains
            .iter()
            .flat_map(|c| [points[c.start].y, points[c.end - 1].y]),
    );
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    chains.sort_unstable_by(|a, b| points[a.start].y.total_cmp(&points[b.start].y));
    let mut active: Vec<Active> = Vec::new();
    active.try_reserve(chains.len()).ok()?;
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
        work.add(active.len())?;
        let mid = lo.midpoint(hi);
        for a in &mut active {
            while points[a.segment + 1].y <= lo {
                a.segment += 1;
                work.add(1)?;
            }
            let mut segment = a.segment;
            let start = at(points, segment, lo).0;
            let mut range = [start, start];
            let mut middle = None;
            while points[segment + 1].y < hi {
                if middle.is_none() && points[segment + 1].y >= mid {
                    middle = Some(at(points, segment, mid).0);
                }
                segment += 1;
                work.add(1)?;
                range = [
                    range[0].min(points[segment].x),
                    range[1].max(points[segment].x),
                ];
            }
            let end = at(points, segment, hi).0;
            a.x = middle.unwrap_or_else(|| at(points, segment, mid).0);
            a.range = [range[0].min(end), range[1].max(end)];
        }
        active.sort_unstable_by(|a, b| a.x.total_cmp(&b.x));
        let first = *sign.get_or_insert(active.first().is_some_and(|a| a.chain.up));
        // Each slab stands alone: its order, directions and separation
        // decide its own winding, whatever another slab holds.
        let ordered = active.len().is_multiple_of(2)
            && active
                .iter()
                .enumerate()
                .all(|(k, a)| a.chain.up == (first ^ (k % 2 == 1)));
        let separated = ordered
            && active.windows(2).all(|w| {
                w[0].range[1] + margins.rounding < w[1].range[0]
                    || apart(
                        points,
                        (w[0].segment, w[1].segment),
                        (lo, hi),
                        margins,
                        work,
                    )
            });
        work.add(0)?;
        if !separated {
            match failing.last_mut() {
                Some(last) if last[1] >= lo => last[1] = hi,
                _ => {
                    failing.try_reserve(1).ok()?;
                    failing.push([lo, hi]);
                }
            }
        }
    }
    Some(())
}

// Appends the strictly y-monotone chains of one closed contour, each in
// increasing y. A chain begins where the direction changes, so a cyclic walk
// from such an edge never splits one.
fn split(vertices: &[Point], points: &mut Vec<Point>, chains: &mut Vec<Chain>) -> Option<()> {
    let n = vertices.len();
    let direction = |i: usize| {
        let (a, b) = (vertices[i].y, vertices[(i + 1) % n].y);
        if a < b {
            1
        } else if a > b {
            -1
        } else {
            0
        }
    };
    let Some(first) =
        (0..n).find(|&i| direction(i) != 0 && direction((i + n - 1) % n) != direction(i))
    else {
        return Some(());
    };
    points.try_reserve(2 * n).ok()?;
    let mut current: Option<Chain> = None;
    for step in 0..n {
        let i = (first + step) % n;
        let up = match direction(i) {
            0 => {
                close(current.take(), points, chains)?;
                continue;
            }
            d => d > 0,
        };
        if let Some(chain) = current.as_mut().filter(|c| c.up == up) {
            chain.end += 1;
        } else {
            close(current.take(), points, chains)?;
            points.push(vertices[i]);
            current = Some(Chain {
                start: points.len() - 1,
                end: points.len() + 1,
                up,
            });
        }
        points.push(vertices[(i + 1) % n]);
    }
    close(current, points, chains)
}

fn close(chain: Option<Chain>, points: &mut [Point], chains: &mut Vec<Chain>) -> Option<()> {
    if let Some(chain) = chain {
        if !chain.up {
            points[chain.start..chain.end].reverse();
        }
        chains.try_reserve(1).ok()?;
        chains.push(chain);
    }
    Some(())
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
    let mut y = lo;
    loop {
        let (left, exact_left) = at(points, i, y);
        let (right, exact_right) = at(points, j, y);
        let d = right - left;
        let separated = if !(exact_left && exact_right) {
            d > margins.rounding
        } else if y == lo || y == hi {
            // Identical vertices may touch at a slab's ends; distinct ones
            // keep the margin.
            d == 0.0 || d > margins.vertices.max(-0.0)
        } else {
            d > margins.vertices
        };
        if !separated {
            return false;
        }
        if y == hi {
            return true;
        }
        if points[i + 1].y == y {
            i += 1;
        }
        if points[j + 1].y == y {
            j += 1;
        }
        if work.add(1).is_none() {
            return false;
        }
        y = points[i + 1].y.min(points[j + 1].y).min(hi);
    }
}

/// One y-monotone piece of a spline chain, oriented downward (`a.y <
/// b.y`): a line, or a monotone quadratic.
#[derive(Debug, Clone, Copy)]
struct Down {
    curve: Option<Mono>,
    a: Point,
    b: Point,
}

impl Down {
    /// The piece oriented downward, and whether it ran downward.
    fn new(piece: Piece) -> Option<(Self, bool)> {
        let (curve, a, b) = match piece {
            Piece::Line(line) => (None, line.a, line.b),
            Piece::Curve(q) => (Some(Mono::new(q)), q.points[0], q.points[2]),
        };
        if a.y == b.y {
            return None;
        }
        let down = a.y < b.y;
        let (a, b) = if down { (a, b) } else { (b, a) };
        Some((Self { curve, a, b }, down))
    }

    /// x at height `y` within the piece, and whether it is an end's value.
    fn x(&self, y: f64) -> (f64, bool) {
        if y <= self.a.y {
            return (self.a.x, y == self.a.y);
        }
        if y >= self.b.y {
            return (self.b.x, y == self.b.y);
        }
        match &self.curve {
            None => (
                self.a.x + (y - self.a.y) / (self.b.y - self.a.y) * (self.b.x - self.a.x),
                false,
            ),
            Some(m) => (m.x_at(y), false),
        }
    }

    /// The directions bounding the piece seen from its end at height `v`,
    /// away from it: toward its control (or along it) and along its chord
    /// to height `far`.
    fn fan(&self, v: f64, far: f64) -> [Point; 2] {
        let vertex = if v == self.a.y { self.a } else { self.b };
        let end = Point::new(self.x(far).0, far);
        let chord = Point::new(end.x - vertex.x, end.y - vertex.y);
        let control = self.curve.map_or(chord, |m| {
            let c = m.control();
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
/// everywhere, by the sweep of [`certified`] over chains of monotone pieces:
/// neighbours are compared by the x ranges of monotone sub-pieces, halved
/// where they overlap, and at a shared end by the fans of their control and
/// chord directions, which meet only at the end when strictly ordered.
/// Steps are capped as the polygon's are; doubt leaves it uncertified.
pub(crate) fn spline_certified(spline: &Spline) -> bool {
    spline_sweep(spline).is_some_and(|ok| ok)
}

fn spline_sweep(spline: &Spline) -> Option<bool> {
    let mut downs: Vec<Down> = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    downs.try_reserve(spline.pieces.len()).ok()?;
    let mut magnitude = 1.0_f64;
    for contour in spline.contours() {
        split_pieces(contour, &mut downs, &mut runs, &mut magnitude)?;
    }
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
    heights.try_reserve(2 * runs.len()).ok()?;
    heights.extend(runs.iter().flat_map(|r| [top(r), bottom(r)]));
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    let mut order: Vec<usize> = (0..runs.len()).collect();
    order.sort_unstable_by(|&a, &b| top(&runs[a]).total_cmp(&top(&runs[b])));
    // Active runs: run, piece at the slab's top, x at mid-slab, x range.
    let mut active: Vec<(usize, usize, f64, [f64; 2])> = Vec::new();
    active.try_reserve(runs.len()).ok()?;
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
        work.add(active.len())?;
        let mid = lo.midpoint(hi);
        for a in &mut active {
            let end = runs[a.0].end;
            while a.1 + 1 < end && downs[a.1].b.y <= lo {
                a.1 += 1;
            }
            let mut k = a.1;
            let first = downs[k].x(lo).0;
            let mut range = [first, first];
            loop {
                work.add(1)?;
                let piece = &downs[k];
                if piece.a.y <= mid && mid <= piece.b.y {
                    a.2 = piece.x(mid).0;
                }
                if piece.b.y >= hi || k + 1 >= end {
                    let x = piece.x(hi).0;
                    range = [range[0].min(x), range[1].max(x)];
                    break;
                }
                range = [range[0].min(piece.b.x), range[1].max(piece.b.x)];
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
            return Some(false);
        }
        for w in active.windows(2) {
            if w[0].3[1] + margin < w[1].3[0] {
                continue;
            }
            let left = (w[0].1, runs[w[0].0].end);
            let right = (w[1].1, runs[w[1].0].end);
            if !chains_apart(&downs, (left, right), (lo, hi), margin, &mut work)? {
                return Some(false);
            }
        }
    }
    Some(true)
}

/// Appends the y-monotone chains of one closed contour of pieces, each
/// oriented downward. A chain begins where the direction changes, so a
/// cyclic walk from such a piece never splits one; horizontal pieces end
/// chains.
fn split_pieces(
    contour: &[Piece],
    downs: &mut Vec<Down>,
    runs: &mut Vec<Run>,
    magnitude: &mut f64,
) -> Option<()> {
    let n = contour.len();
    let direction = |p: &Piece| match Down::new(*p) {
        Some((_, true)) => 1,
        Some((_, false)) => -1,
        None => 0,
    };
    // A contour without height bounds no area.
    let Some(first) = (0..n).find(|&i| {
        let d = direction(&contour[i]);
        d != 0 && direction(&contour[(i + n - 1) % n]) != d
    }) else {
        return Some(());
    };
    let mut current: Option<(usize, bool)> = None;
    let close = |current: &mut Option<(usize, bool)>,
                 downs: &mut Vec<Down>,
                 runs: &mut Vec<Run>|
     -> Option<()> {
        if let Some((start, down)) = current.take() {
            if !down {
                downs.get_mut(start..)?.reverse();
            }
            runs.try_reserve(1).ok()?;
            runs.push(Run {
                start,
                end: downs.len(),
                down,
            });
        }
        Some(())
    };
    for step in 0..n {
        let piece = contour[(first + step) % n];
        let Some((down_piece, down)) = Down::new(piece) else {
            close(&mut current, downs, runs)?;
            continue;
        };
        *magnitude = magnitude
            .max(down_piece.a.x.abs())
            .max(down_piece.b.x.abs())
            .max(down_piece.a.y.abs())
            .max(down_piece.b.y.abs());
        if current.is_none_or(|(_, d)| d != down) {
            close(&mut current, downs, runs)?;
            current = Some((downs.len(), down));
        }
        downs.try_reserve(1).ok()?;
        downs.push(down_piece);
    }
    close(&mut current, downs, runs)
}

/// Whether the right chain stays right of the left one on the open slab
/// `(lo, hi)`; each chain is given by its piece at `lo` and its end.
fn chains_apart(
    downs: &[Down],
    ((mut i, ie), (mut j, je)): ((usize, usize), (usize, usize)),
    (lo, hi): (f64, f64),
    margin: f64,
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
            &downs[i],
            &downs[j],
            (a, b),
            (a == lo, b == hi),
            margin,
            work,
            12,
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
    left: &Down,
    right: &Down,
    (a, b): (f64, f64),
    ends: (bool, bool),
    margin: f64,
    work: &mut Work,
    depth: u32,
) -> Option<bool> {
    work.add(1)?;
    let (la, left_a) = left.x(a);
    let (lb, left_b) = left.x(b);
    let (ra, right_a) = right.x(a);
    let (rb, right_b) = right.x(b);
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
        if fans_apart(left.fan(v, far), right.fan(v, far), far > v, margin) {
            return Some(true);
        }
    }
    if depth == 0 {
        return Some(false);
    }
    let m = a.midpoint(b);
    Some(
        pieces_apart(
            left,
            right,
            (a, m),
            (ends.0, false),
            margin,
            work,
            depth - 1,
        )? && pieces_apart(
            left,
            right,
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
        assert!(certified(&polygon(&[&square]), 0.0));
        assert!(certified(&polygon(&[&square, &hole]), 0.0));
    }

    #[test]
    fn overlaps_crossings_and_nested_same_direction_do_not() {
        let square = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        let inner = [(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)];
        let bowtie = [(0.0, 0.0), (4.0, 4.0), (4.0, 0.0), (0.0, 4.0)];
        assert!(!certified(&polygon(&[&square, &inner]), 0.0));
        assert!(!certified(&polygon(&[&bowtie]), 0.0));
    }

    #[test]
    fn a_margin_rejects_close_chains() {
        let a = [(0.0, 0.0), (1.0, 0.0), (1.0, 4.0), (0.0, 4.0)];
        let b = [(1.001, 0.0), (2.0, 0.0), (2.0, 4.0), (1.001, 4.0)];
        assert!(certified(&polygon(&[&a, &b]), 0.0));
        assert!(!certified(&polygon(&[&a, &b]), 1.0 / 128.0));
    }
}
