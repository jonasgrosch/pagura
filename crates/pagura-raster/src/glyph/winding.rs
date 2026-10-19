//! Winding certificate for signed-area glyph coverage (ADR 0004).
//!
//! A cell's signed area equals its nonzero area when the winding number is 0
//! or one common sign everywhere off the boundary. Each contour splits into
//! strictly y-monotone chains; horizontal edges bound no area. Between
//! consecutive chain endpoints, the active chains must keep their horizontal
//! order, and their directions must alternate from the left starting with the
//! common sign. The first meeting inside such a slab would be between chains
//! adjacent at its middle (Shamos and Hoey), so only neighbours are compared,
//! at every vertex height of either chain. Interpolated comparisons keep a
//! margin above their rounding error; only identical vertices may touch, and
//! only at the slab's ends. Any doubt leaves the glyph on the general path.
//!
//! The sweep counts the active chains of every slab and every segment step.
//! Past 32 E + 1024 steps for E edges, or when an allocation fails, the glyph
//! stays uncertified, so the sweep's work stays linear in E even for
//! geometries the cache does not keep.

use super::GlyphContour;
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
    // x at the slab's middle, and the x range within the slab.
    x: f64,
    range: [f64; 2],
}

/// The strictly y-monotone chains of a glyph polygon, before certification.
#[derive(Debug)]
pub(super) struct MonotoneChains {
    points: Vec<Point>,
    chains: Vec<Chain>,
    edges: usize,
    magnitude: f64,
}

impl MonotoneChains {
    /// Splits every contour into chains. Returns `None` for a geometric point
    /// contour or when an allocation fails.
    pub(super) fn new(contours: &[GlyphContour]) -> Option<Self> {
        let mut result = Self {
            points: Vec::new(),
            chains: Vec::new(),
            edges: 0,
            magnitude: 1.0,
        };
        let count = contours.len();
        for (i, contour) in contours.iter().enumerate() {
            let vertices = contour.points();
            // A final open one-point contour is omitted, as for paths.
            if i + 1 == count && !contour.is_closed() && vertices.len() == 1 {
                continue;
            }
            if contour.is_degenerate() {
                return None;
            }
            for p in vertices {
                result.magnitude = result.magnitude.max(p.x.abs()).max(p.y.abs());
            }
            result.edges += vertices.len();
            split(vertices, &mut result.points, &mut result.chains)?;
        }
        Some(result)
    }

    /// The number of monotone chains.
    pub(super) fn count(&self) -> usize {
        self.chains.len()
    }

    /// Whether the winding is certified to be 0 or one common sign, within the
    /// work cap.
    pub(super) fn certified(mut self) -> bool {
        let mut work = Work {
            done: 0,
            cap: self.edges.saturating_mul(32).saturating_add(1024),
        };
        sweep(&self.points, &mut self.chains, self.magnitude, &mut work).is_some()
    }
}

// Steps taken by the sweep; `add` fails once they exceed the cap.
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

fn sweep(points: &[Point], chains: &mut [Chain], magnitude: f64, work: &mut Work) -> Option<()> {
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
    // Interpolation errs by at most 11 units of roundoff of the largest
    // coordinate, so a difference of two such values by 22; keep 2^8 of them.
    let margin = magnitude * 2f64.powi(-45);
    // Every chain fits, so pushes never reallocate.
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
        if !active.len().is_multiple_of(2)
            || active
                .iter()
                .enumerate()
                .any(|(k, a)| a.chain.up != (first ^ (k % 2 == 1)))
        {
            return None;
        }
        for w in active.windows(2) {
            // Interpolated range ends err by less than half the margin.
            let separated = w[0].range[1] + margin < w[1].range[0]
                || apart(points, (w[0].segment, w[1].segment), (lo, hi), margin, work);
            if !separated {
                return None;
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
    margin: f64,
    work: &mut Work,
) -> bool {
    let mut y = lo;
    loop {
        let (left, exact_left) = at(points, i, y);
        let (right, exact_right) = at(points, j, y);
        let d = right - left;
        let separated = if !(exact_left && exact_right) {
            d > margin
        } else if y == lo || y == hi {
            d >= 0.0
        } else {
            d > 0.0
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

#[cfg(test)]
mod tests;
