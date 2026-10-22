//! The exact row resolver: the boundary of the covered set of one pixel
//! row, for rows whose windings the signed area alone does not determine
//! (8.5.3.3, 8.5.4).
//!
//! The row is cut at every end point and crossing height of its pieces, so
//! that no two pieces cross inside a sub-slab. There the pieces keep one
//! order; walked left to right they give each group (a mark, and each path
//! of its clip) its winding, and a point is covered where every group is
//! inside under its rule. The covered set of a sub-slab is a union of
//! trapezoids between ordered pieces; its sides leave as pieces of the
//! original edges, entering downward and leaving upward, joined across
//! sub-slabs while the same edge stays the same side. Their winding is 0 or
//! 1 everywhere, so the cells take their signed area as the covered area.

use super::edges::Line;
use crate::FillRule;
use pagura_core::{Error, Meter, Result};

/// One edge's part within the row being resolved.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Piece {
    /// The whole edge: crossings and sides are interpolated on it.
    pub(crate) line: Line,
    /// Its heights within the row, `lo < hi`.
    pub(crate) lo: f64,
    pub(crate) hi: f64,
    /// +1 when the edge runs downward, -1 upward.
    pub(crate) direction: i32,
    /// The group it belongs to: 0 for a mark, then each clipping path.
    pub(crate) group: u8,
}

impl Piece {
    /// The part of `line` within row `top..top + 1`, if it has height there.
    pub(crate) fn new(line: Line, top: f64, group: u8) -> Option<Self> {
        let Line { a, b } = line;
        if a.y == b.y {
            return None;
        }
        let (upper, lower, direction) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
        let lo = upper.y.max(top);
        let hi = lower.y.min(top + 1.0);
        (lo < hi).then_some(Self {
            line,
            lo,
            hi,
            direction,
            group,
        })
    }

    /// The leftmost and rightmost x of the piece.
    fn columns(&self) -> (f64, f64) {
        let (a, b) = (self.line.x(self.lo), self.line.x(self.hi));
        (a.min(b), a.max(b))
    }
}

/// A side open from an earlier sub-slab: its piece, +1 where coverage
/// starts at it, -1 where it stops, and the height it opened at.
#[derive(Debug, Clone, Copy)]
struct Open {
    piece: u32,
    side: i32,
    from: f64,
}

/// Reusable storage of the resolver.
#[derive(Debug, Default)]
pub(crate) struct Resolver {
    heights: Vec<f64>,
    order: Vec<(f64, u32)>,
    windings: Vec<i64>,
    by_left: Vec<(f64, f64, u32)>,
    /// Sides open from the sub-slab above, and this sub-slab's.
    open: Vec<Open>,
    sides: Vec<(u32, i32)>,
    next: Vec<Open>,
}

impl Resolver {
    /// Appends to `out` the sides of the covered set of row `top..top + 1`
    /// of `pieces`, where group `g` is inside under `rules[g]` and a point
    /// is covered when every group is inside. Charges `meter` before the
    /// work.
    ///
    /// # Errors
    ///
    /// Returns the meter's `Limit` error, or one for failed allocations.
    pub(crate) fn row(
        &mut self,
        pieces: &[Piece],
        rules: &[FillRule],
        top: f64,
        (out, meter): (&mut Vec<Line>, &mut Meter),
    ) -> Result<()> {
        let allocation = |_| Error::limit("row resolver allocation failed");
        let count = pieces.len();
        meter.charge(count.saturating_mul(4).saturating_add(8))?;
        self.heights.clear();
        self.heights
            .try_reserve(2 * count + 2)
            .map_err(allocation)?;
        self.heights.extend([top, top + 1.0]);
        for p in pieces {
            self.heights.extend([p.lo, p.hi]);
        }
        self.crossings(pieces, meter)?;
        self.heights.sort_unstable_by(f64::total_cmp);
        self.heights.dedup();
        let slabs = self.heights.len().saturating_sub(1);
        meter.charge(slabs.saturating_mul(count.saturating_mul(3).saturating_add(4)))?;
        self.windings.clear();
        self.windings.try_reserve(rules.len()).map_err(allocation)?;
        self.windings.resize(rules.len(), 0);
        self.open.clear();
        for v in [&mut self.open, &mut self.next] {
            v.clear();
            v.try_reserve(count).map_err(allocation)?;
        }
        self.sides.clear();
        self.sides.try_reserve(count).map_err(allocation)?;
        self.order.clear();
        self.order.try_reserve(count).map_err(allocation)?;
        out.try_reserve(2 * count).map_err(allocation)?;
        for k in 0..slabs {
            let (a, b) = (self.heights[k], self.heights[k + 1]);
            if b > a {
                self.slab(pieces, rules, (a, b));
                self.join(pieces, a, out);
            }
        }
        for open in self.open.drain(..) {
            push_side(out, pieces[open.piece as usize].line, open.side, (open.from, top + 1.0));
        }
        Ok(())
    }

    /// Adds the heights where two pieces cross strictly inside both: pairs
    /// whose columns overlap are compared at their common ends.
    fn crossings(&mut self, pieces: &[Piece], meter: &mut Meter) -> Result<()> {
        let allocation = |_| Error::limit("row resolver allocation failed");
        self.by_left.clear();
        self.by_left
            .try_reserve(pieces.len())
            .map_err(allocation)?;
        for (i, p) in (0_u32..).zip(pieces) {
            let (left, right) = p.columns();
            self.by_left.push((left, right, i));
        }
        self.by_left
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        for (n, &(_, right, i)) in self.by_left.iter().enumerate() {
            let p = pieces[i as usize];
            for &(left, _, j) in &self.by_left[n + 1..] {
                if left > right {
                    break;
                }
                meter.charge(1)?;
                let q = pieces[j as usize];
                let lo = p.lo.max(q.lo);
                let hi = p.hi.min(q.hi);
                if lo >= hi {
                    continue;
                }
                let d0 = p.line.x(lo) - q.line.x(lo);
                let d1 = p.line.x(hi) - q.line.x(hi);
                if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
                    let y = lo + (hi - lo) * (d0 / (d0 - d1));
                    if y > lo && y < hi {
                        self.heights.try_reserve(1).map_err(allocation)?;
                        self.heights.push(y);
                    }
                }
            }
        }
        Ok(())
    }

    /// The sides of sub-slab `a..b` in `sides`: the pieces across it in
    /// order, coincident ones together, and where coverage starts or stops.
    fn slab(&mut self, pieces: &[Piece], rules: &[FillRule], (a, b): (f64, f64)) {
        let mid = a.midpoint(b);
        self.order.clear();
        for (i, p) in (0_u32..).zip(pieces) {
            if p.lo <= a && p.hi >= b {
                self.order.push((p.line.x(mid), i));
            }
        }
        self.order
            .sort_unstable_by(|p, q| p.0.total_cmp(&q.0).then(p.1.cmp(&q.1)));
        self.windings.fill(0);
        self.sides.clear();
        let mut inside = false;
        let mut n = 0;
        while let Some(&(x, first)) = self.order.get(n) {
            while let Some(&(_, i)) = self.order.get(n).filter(|o| o.0 == x) {
                let p = pieces[i as usize];
                if let Some(w) = self.windings.get_mut(usize::from(p.group)) {
                    *w += i64::from(p.direction);
                }
                n += 1;
            }
            let now = self
                .windings
                .iter()
                .zip(rules)
                .all(|(&w, &rule)| covered(w, rule));
            if now != inside {
                self.sides.push((first, if now { 1 } else { -1 }));
                inside = now;
            }
        }
    }

    /// Joins the sides of the sub-slab starting at `a` with those open
    /// from above: a piece that stays the same side stays open, the others
    /// end at `a`, and new sides open there. Sides come in order, and so do
    /// the open ones, so one merge pass suffices.
    fn join(&mut self, pieces: &[Piece], a: f64, out: &mut Vec<Line>) {
        self.next.clear();
        for &(piece, side) in &self.sides {
            let kept = self
                .open
                .iter()
                .position(|o| o.piece == piece && o.side == side);
            let from = match kept {
                Some(k) => {
                    let from = self.open[k].from;
                    self.open[k].side = 0;
                    from
                }
                None => a,
            };
            self.next.push(Open { piece, side, from });
        }
        for open in self.open.drain(..) {
            if open.side != 0 {
                push_side(out, pieces[open.piece as usize].line, open.side, (open.from, a));
            }
        }
        std::mem::swap(&mut self.open, &mut self.next);
    }
}

/// Appends the side of `line` over heights `from..to`: downward when
/// coverage starts at it (`side` +1), upward when it stops.
fn push_side(out: &mut Vec<Line>, line: Line, side: i32, (from, to): (f64, f64)) {
    let (upper, lower) = if line.a.y < line.b.y {
        (line.a, line.b)
    } else {
        (line.b, line.a)
    };
    let from = from.max(upper.y);
    let to = to.min(lower.y);
    if from >= to {
        return;
    }
    let a = crate::Point::new(line.x(from), from);
    let b = crate::Point::new(line.x(to), to);
    out.push(if side > 0 {
        Line { a, b }
    } else {
        Line { a: b, b: a }
    });
}

fn covered(winding: i64, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}
