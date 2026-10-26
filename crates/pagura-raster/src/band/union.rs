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
//!
//! The sub-slabs are swept downward, keeping the pieces that span the
//! current one in order: a piece costs the sub-slabs it spans, not every
//! sub-slab of the row, and pieces that meet without crossing (the shared
//! sides of a union, the floor cells of points) add none.
//!
//! A row of many pieces is cut at columns into chunks of a few dozen, each
//! swept with its pieces clipped to its columns and starting from the
//! windings left of it, which the chunk before carries over as a function
//! of height. The sweeps then cost what the chunks hold, as cells would,
//! rather than every crossing and end of the row times every piece across
//! it. Coverage continuing across a cut needs no side there.
//!
//! Most crossings of overlapping marks lie inside their union: a stroke
//! whose segments are shorter than its pen fans out into pieces that cross
//! each other many times over within the covered set. Where all that
//! happens at a height is such crossings, and the heights around it are
//! further from it than a crossing found can be from where its pieces
//! cross, the pieces crossing there take their order below and every side
//! stays as it is: that sub-slab costs the swaps, not its pieces.

use super::edges::Line;
use crate::FillRule;
use crate::geometry::{greater, lesser};
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
        let lo = greater(upper.y, top);
        let hi = lesser(lower.y, top + 1.0);
        (lo < hi).then_some(Self {
            line,
            lo,
            hi,
            direction,
            group,
        })
    }

    /// The x of the piece at its upper and lower ends.
    fn ends(&self) -> (f64, f64) {
        (self.line.x(self.lo), self.line.x(self.hi))
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

/// The fewest pieces a sweep places by plain interpolation first.
const PLAIN_PIECES: usize = 32;

/// The pieces a row takes in one sweep: a row of more is cut into chunks.
const CHUNK: usize = 64;

/// How far a crossing found may lie from where its pieces cross for the
/// swaps: sub-slabs further than twice this from both neighbours.
const SURE: f64 = 1.0 / (1_u64 << 30) as f64;

/// A piece's place across a sub-slab, at its middle height, and its index:
/// ordered by place, then index.
type Place = (f64, u32);

fn ordered(p: &Place, q: &Place) -> std::cmp::Ordering {
    p.0.total_cmp(&q.0).then(p.1.cmp(&q.1))
}

/// A piece's extent along one axis, `(start, end, piece)`.
type Span = (f64, f64, u32);

/// Work units for sorting `count` values and passing over them once.
fn sort_work(count: usize) -> usize {
    count.saturating_mul(count.max(1).ilog2() as usize + 2)
}

/// Reusable storage of the resolver.
#[derive(Debug, Default)]
pub(crate) struct Resolver {
    heights: Vec<f64>,
    /// The pieces' columns and heights, `(start, end, piece)`, each by
    /// start: the sweep takes the pieces by their upper end.
    columns: Vec<Span>,
    rows: Vec<Span>,
    /// The pieces' x at their upper and lower ends, by piece.
    ends: Vec<(f64, f64)>,
    /// The pieces' edges prepared for plain interpolation, and the largest
    /// error of their plain places.
    lanes: Vec<Lane>,
    lane_error: f64,
    /// The pieces spanning the current sub-slab, in order across it.
    order: Vec<Place>,
    /// The pieces joining at the current sub-slab.
    joining: Vec<Place>,
    windings: Vec<i64>,
    /// Sides open from the sub-slab above, and this sub-slab's.
    open: Vec<Open>,
    sides: Vec<(u32, i32)>,
    next: Vec<Open>,
    /// Per piece, the side it is open as from above.
    slots: Vec<Slot>,
    /// The windings left of the current chunk: from each height on, the
    /// windings of every group.
    carry: Vec<f64>,
    carried: Vec<i64>,
    /// Those the current chunk leaves for the next.
    after: Vec<f64>,
    carried_after: Vec<i64>,
    /// The pieces' first and last columns, `(first, last, piece)`, by first
    /// column; their last columns in order; the columns the row is cut at.
    /// Their x at their ends, by piece.
    spans: Vec<Span>,
    row_ends: Vec<(f64, f64)>,
    lasts: Vec<f64>,
    cuts: Vec<f64>,
    /// Sort keys of heights and spans, and spans in their sorted order.
    height_keys: Vec<u64>,
    span_keys: Vec<u128>,
    sorted: Vec<Span>,
    /// Where a sweep may swap neighbours without a sub-slab: the crossings
    /// found, each as its height's sort key above the two pieces, by
    /// height; per place in the order, the winding left of it; per piece,
    /// its place; and the pieces by lower end, each as its height's sort key
    /// above the piece, with how many have ended.
    crosses: Vec<u128>,
    lefts: Vec<i64>,
    at: Vec<usize>,
    lowers: Vec<u128>,
    ended: usize,
    /// Whether the coverage pass keeps them, for the current sweep.
    keep: bool,
    /// The heights where pairs may cross further than [`SURE`] from a
    /// crossing found, or that a pair found not to cross may cross near an
    /// end, with the pair; the heights whose sub-slabs each reaches, by the
    /// first; per height how many reach it; those reaching the heights last
    /// swapped at, and the next to reach.
    doubtful: Vec<(f64, f64, u32, u32)>,
    bands: Vec<(usize, usize, u32, u32)>,
    reached: Vec<i32>,
    reaching: Vec<(usize, u32, u32)>,
    band: usize,
    /// Turns the swaps off, for comparison with the full sweep, and counts
    /// the sub-slabs they took.
    #[cfg(test)]
    full: bool,
    #[cfg(test)]
    swapped: usize,
    /// The pieces reaching the current chunk, and its pieces clipped to it.
    live: Vec<u32>,
    local: Vec<Piece>,
}

/// A piece's side open from above: +1 or -1 (0 when none), since when, and
/// whether the current sub-slab keeps it.
#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    side: i32,
    from: f64,
    kept: bool,
}

/// The meters a row is charged to: work, and the bytes of the resolver's
/// scratch and of the sides it appends.
pub(crate) struct Meters<'a> {
    pub(crate) work: &'a mut Meter,
    pub(crate) bytes: &'a mut Meter,
}

/// Makes room for `extra` more values in `values`, charging `bytes` for the
/// capacity added: at least twice the old, so that growth stays amortized.
pub(crate) fn room<T>(values: &mut Vec<T>, extra: usize, bytes: &mut Meter) -> Result<()> {
    bytes.reserve_capacity(values, extra)
}

impl Resolver {
    /// Appends to `out` the sides of the covered set of row `top..top + 1`
    /// of `pieces`, where group `g` is inside under `rules[g]` and a point
    /// is covered when every group is inside.
    ///
    /// Each stage is charged before it runs: the sorts of the pieces by
    /// column and by height and the counts of the pairs overlapping in
    /// each, a crossing test per pair overlapping in the axis where fewer
    /// do, the sort of the heights found, and per sub-slab a unit per piece
    /// spanning it, one per move restoring their order and a sort of those
    /// joining.
    ///
    /// # Errors
    ///
    /// Returns a meter's `Limit` error, or one for failed allocations.
    pub(crate) fn row(
        &mut self,
        pieces: &[Piece],
        rules: &[FillRule],
        top: f64,
        (out, meters): (&mut Vec<Line>, Meters<'_>),
    ) -> Result<()> {
        let Meters { work, bytes } = meters;
        u32::try_from(pieces.len()).map_err(|_| Error::limit("row resolver pieces exceed u32"))?;
        self.carry.clear();
        self.carried.clear();
        room(&mut self.carry, 1, bytes)?;
        room(&mut self.carried, rules.len(), bytes)?;
        self.carry.push(top);
        self.carried.resize(rules.len(), 0);
        if pieces.len() <= CHUNK {
            return self.sweep(pieces, (rules, top), (out, work, bytes), false);
        }
        self.cut(pieces, (work, bytes))?;
        let mut live = std::mem::take(&mut self.live);
        let mut local = std::mem::take(&mut self.local);
        let result = self.chunks(
            pieces,
            (rules, top),
            (&mut live, &mut local),
            (out, work, bytes),
        );
        self.live = live;
        self.local = local;
        result
    }

    /// Chooses the columns to cut the row at: walking the first columns of
    /// the pieces left to right, a chunk ends at the first column where it
    /// holds `CHUNK` pieces and at most a quarter of them reach on, so that
    /// the chunks hold about as many pieces as the row.
    fn cut(&mut self, pieces: &[Piece], (work, bytes): (&mut Meter, &mut Meter)) -> Result<()> {
        let count = pieces.len();
        work.charge_product(sort_work(count), 3)?;
        room(&mut self.spans, count, bytes)?;
        room(&mut self.lasts, count, bytes)?;
        room(&mut self.row_ends, count, bytes)?;
        self.spans.clear();
        self.lasts.clear();
        self.row_ends.clear();
        for (i, p) in (0_u32..).zip(pieces) {
            let (top, bottom) = p.ends();
            self.row_ends.push((top, bottom));
            let (first, last) = (lesser(top, bottom).floor(), greater(top, bottom).floor());
            self.spans.push((first, last, i));
            self.lasts.push(last);
        }
        room(&mut self.span_keys, count, bytes)?;
        room(&mut self.sorted, count, bytes)?;
        room(&mut self.height_keys, count, bytes)?;
        sort_spans(&mut self.spans, (&mut self.span_keys, &mut self.sorted));
        sort_values(&mut self.lasts, &mut self.height_keys);
        self.cuts.clear();
        let (mut entered, mut reached) = (0, 0);
        for n in 1..count {
            let column = self.spans[n].0;
            if column == self.spans[n - 1].0 {
                continue;
            }
            // Pieces starting left of `column` that end at or after it.
            let reaching = n - self.lasts.partition_point(|&last| last < column);
            let held = n - entered + reached;
            if held >= CHUNK && reaching.saturating_mul(4) <= held {
                room(&mut self.cuts, 1, bytes)?;
                self.cuts.push(column);
                (entered, reached) = (n, reaching);
            }
        }
        Ok(())
    }

    /// Sweeps the chunks between the cuts left to right, each with the
    /// pieces reaching it clipped to its columns.
    fn chunks(
        &mut self,
        pieces: &[Piece],
        (rules, top): (&[FillRule], f64),
        (live, local): (&mut Vec<u32>, &mut Vec<Piece>),
        (out, work, bytes): (&mut Vec<Line>, &mut Meter, &mut Meter),
    ) -> Result<()> {
        live.clear();
        let mut entered = 0;
        let chunks = self.cuts.len() + 1;
        for k in 0..chunks {
            let left = k.checked_sub(1).map_or(f64::NEG_INFINITY, |c| self.cuts[c]);
            let right = self.cuts.get(k).copied().unwrap_or(f64::INFINITY);
            work.charge(live.len().saturating_add(1))?;
            live.retain(|&i| self.spans[i as usize].1 >= left);
            while let Some(&(first, _, _)) = self.spans.get(entered)
                && first < right
            {
                room(live, 1, bytes)?;
                live.push(u32::try_from(entered).unwrap_or(u32::MAX));
                entered += 1;
            }
            work.charge(live.len())?;
            local.clear();
            room(local, live.len(), bytes)?;
            for &n in live.iter() {
                let i = self.spans[n as usize].2 as usize;
                local.extend(clip(pieces[i], self.row_ends[i], (left, right)));
            }
            self.sweep(local, (rules, top), (out, work, bytes), k + 1 < chunks)?;
        }
        Ok(())
    }

    /// Sweeps `pieces` from the windings carried from the left, appending
    /// the sides where coverage starts or stops among them to `out`, and
    /// when `carry` is set, the windings right of them for the next chunk.
    fn sweep(
        &mut self,
        pieces: &[Piece],
        (rules, top): (&[FillRule], f64),
        (out, work, bytes): (&mut Vec<Line>, &mut Meter, &mut Meter),
        carry: bool,
    ) -> Result<()> {
        let total = u32::try_from(pieces.len())
            .map_err(|_| Error::limit("row resolver pieces exceed u32"))?;
        let count = pieces.len();
        work.charge_product(sort_work(count), 4)?;
        room(&mut self.columns, count, bytes)?;
        room(&mut self.rows, count, bytes)?;
        room(&mut self.ends, count, bytes)?;
        let breaks = self.carry.len().saturating_sub(1);
        room(&mut self.heights, 2 * count + 2 + breaks, bytes)?;
        self.heights.clear();
        self.heights.extend([top, top + 1.0]);
        self.heights.extend_from_slice(&self.carry[1..]);
        for p in pieces {
            self.heights.extend([p.lo, p.hi]);
        }
        // A few pieces cross a sub-slab or two each: preparing their lanes
        // costs more than placing them exactly.
        self.lanes.clear();
        if count >= PLAIN_PIECES {
            room(&mut self.lanes, count, bytes)?;
            self.lanes.extend(pieces.iter().map(|p| Lane::new(p.line)));
        }
        self.lane_error = self.lanes.iter().fold(0.0, |e, lane| e.max(lane.error));
        // Swaps take one group, and the many pieces that plain places serve.
        let swaps = !self.lanes.is_empty() && rules.len() == 1;
        #[cfg(test)]
        let swaps = swaps && !self.full;
        self.crosses.clear();
        self.crossings(pieces, total, swaps, (work, bytes))?;
        // The crossings are among the heights the charge sorts.
        work.charge(sort_work(self.heights.len()))?;
        room(&mut self.height_keys, self.heights.len(), bytes)?;
        sort_values(&mut self.heights, &mut self.height_keys);
        self.heights.dedup();
        self.keep = swaps;
        if swaps {
            self.prepare_swaps(pieces, work, bytes)?;
        }
        self.windings.clear();
        room(&mut self.windings, rules.len(), bytes)?;
        self.windings.resize(rules.len(), 0);
        for v in [&mut self.order, &mut self.joining] {
            v.clear();
            room(v, count, bytes)?;
        }
        for v in [&mut self.open, &mut self.next] {
            v.clear();
            room(v, count, bytes)?;
        }
        self.sides.clear();
        room(&mut self.sides, count, bytes)?;
        self.slots.clear();
        room(&mut self.slots, count, bytes)?;
        self.slots.resize(count, Slot::default());
        room(out, 2 * count, bytes)?;
        self.after.clear();
        self.carried_after.clear();
        let (mut joined, mut segment, mut cross, mut through) = (0, 0, 0, 0);
        for k in 1..self.heights.len() {
            let (a, b) = (self.heights[k - 1], self.heights[k]);
            if b > a && k > through {
                let carried = segment;
                while self.carry.get(segment + 1).is_some_and(|&y| y <= a) {
                    segment += 1;
                }
                if swaps
                    && segment == carried
                    && let Some((after, taken, started)) = self.take_swaps(
                        pieces,
                        rules[0],
                        (k - 1, cross, joined, segment),
                        (out, work, bytes),
                    )?
                {
                    (cross, through, joined) = (cross + taken, after, joined + started);
                    continue;
                }
                joined = self.place(pieces, (a, b), joined, work)?;
                work.charge(
                    self.order
                        .len()
                        .saturating_add(self.open.len())
                        .saturating_add(1),
                )?;
                self.slab(pieces, rules, segment);
                if carry {
                    self.carry_on(a, bytes)?;
                }
                self.join(pieces, a, (out, bytes))?;
                cross = self.pass(a, cross);
            }
        }
        for open in self.open.drain(..) {
            let line = pieces[open.piece as usize].line;
            push_side((out, bytes), line, open.side, (open.from, top + 1.0))?;
        }
        if carry {
            std::mem::swap(&mut self.carry, &mut self.after);
            std::mem::swap(&mut self.carried, &mut self.carried_after);
        }
        Ok(())
    }

    /// Records the windings right of the current chunk from height `a` on,
    /// unless they equal those above.
    fn carry_on(&mut self, a: f64, bytes: &mut Meter) -> Result<()> {
        let groups = self.windings.len();
        let last = self.carried_after.len().saturating_sub(groups);
        if !self.after.is_empty() && self.carried_after[last..] == self.windings[..] {
            return Ok(());
        }
        room(&mut self.after, 1, bytes)?;
        room(&mut self.carried_after, groups, bytes)?;
        self.after.push(a);
        self.carried_after.extend_from_slice(&self.windings);
        Ok(())
    }

    /// Adds the heights where two pieces cross strictly inside both, and
    /// when `record` is set, the crossings with their pieces. Only pairs
    /// overlapping in both columns and heights can cross; they are
    /// enumerated along the axis where fewer pairs overlap, so that pieces
    /// side by side across the row, or short ones one below another, cost
    /// about their sort.
    fn crossings(
        &mut self,
        pieces: &[Piece],
        total: u32,
        record: bool,
        (work, bytes): (&mut Meter, &mut Meter),
    ) -> Result<()> {
        self.columns.clear();
        self.rows.clear();
        self.ends.clear();
        for (i, p) in (0..total).zip(pieces) {
            let (top, bottom) = p.ends();
            self.ends.push((top, bottom));
            self.columns
                .push((lesser(top, bottom), greater(top, bottom), i));
            self.rows.push((p.lo, p.hi, i));
        }
        // A pair overlaps from the lower of their upper ends to the higher
        // of their lower ends: a piece's x there is often one of its ends.
        let x = |k: u32, y: f64| {
            let (p, (top, bottom)) = (&pieces[k as usize], self.ends[k as usize]);
            if y.to_bits() == p.lo.to_bits() {
                top
            } else if y.to_bits() == p.hi.to_bits() {
                bottom
            } else {
                p.line.x(y)
            }
        };
        // A pair whose plain places keep one side throughout, further apart
        // than their errors at both ends, cannot cross.
        let lanes = &self.lanes;
        let apart = |i: u32, j: u32, (lo, hi): (f64, f64)| {
            let (p, q) = (&pieces[i as usize], &pieces[j as usize]);
            let (a, b) = (&lanes[i as usize], &lanes[j as usize]);
            let error = a.error + b.error;
            let (d0, d1) = (
                a.x(&p.line, lo) - b.x(&q.line, lo),
                a.x(&p.line, hi) - b.x(&q.line, hi),
            );
            (d0 > error && d1 > error) || (d0 < -error && d1 < -error)
        };
        // A difference of two places errs by at most this.
        let reach = self
            .ends
            .iter()
            .fold(1.0, |m, &(t, b)| greater(m, t.abs().max(b.abs())));
        let error = 16.0 * f64::EPSILON * (1.0 + reach);
        self.doubtful.clear();
        room(&mut self.span_keys, pieces.len(), bytes)?;
        room(&mut self.sorted, pieces.len(), bytes)?;
        sort_spans(&mut self.columns, (&mut self.span_keys, &mut self.sorted));
        sort_spans(&mut self.rows, (&mut self.span_keys, &mut self.sorted));
        let (across, down) = (overlapping(&self.columns), overlapping(&self.rows));
        let spans = if down < across {
            &self.rows
        } else {
            &self.columns
        };
        work.charge(across.min(down))?;
        for (n, &(_, end, i)) in spans.iter().enumerate() {
            let p = pieces[i as usize];
            for &(start, _, j) in &spans[n + 1..] {
                if start > end {
                    break;
                }
                let q = pieces[j as usize];
                let lo = greater(p.lo, q.lo);
                let hi = lesser(p.hi, q.hi);
                if lo >= hi || (!lanes.is_empty() && apart(i, j, (lo, hi))) {
                    continue;
                }
                let d0 = x(i, lo) - x(j, lo);
                let d1 = x(i, hi) - x(j, hi);
                if record
                    && let Some(heights) = doubtful((p.line, q.line), (lo, hi), (d0, d1), error)
                {
                    room(&mut self.doubtful, 1, bytes)?;
                    self.doubtful.push((heights.0, heights.1, i, j));
                }
                if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
                    let y = lo + (hi - lo) * (d0 / (d0 - d1));
                    if y > lo && y < hi {
                        room(&mut self.heights, 1, bytes)?;
                        self.heights.push(y);
                        if record {
                            room(&mut self.crosses, 1, bytes)?;
                            self.crosses.push(
                                u128::from(total_key(y.to_bits())) << 64
                                    | u128::from(i) << 32
                                    | u128::from(j),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Orders the pieces spanning sub-slab `a..b`: those from above placed
    /// anew and dropped where they ended, their order restored (they swap
    /// only where they crossed, at `a`), and those starting at `a` merged
    /// in. Returns how many pieces, by upper end, have joined.
    ///
    /// Pieces are placed by plain interpolation first. Where neighbours stay
    /// apart by more than twice the largest error, every piece left of the
    /// gap is left of every piece right of it in the exact places too; the
    /// runs between such gaps take their exact places and order. So the
    /// order and the coincident pieces are those of the exact places.
    fn place(
        &mut self,
        pieces: &[Piece],
        (a, b): (f64, f64),
        mut joined: usize,
        work: &mut Meter,
    ) -> Result<usize> {
        let mid = a.midpoint(b);
        work.charge(self.order.len())?;
        self.order.retain(|&(_, i)| pieces[i as usize].hi >= b);
        self.joining.clear();
        while let Some(&(lo, hi, i)) = self.rows.get(joined)
            && lo <= a
        {
            if hi >= b {
                self.joining.push((0.0, i));
            }
            joined += 1;
        }
        let exact = |i: u32| pieces[i as usize].line.x(mid);
        if self.lanes.is_empty() {
            arrange((&mut self.order, &mut self.joining), exact, work)?;
        } else {
            let lanes = &self.lanes;
            let plain = |i: u32| lanes[i as usize].x(&pieces[i as usize].line, mid);
            arrange((&mut self.order, &mut self.joining), plain, work)?;
            let error = self.lane_error;
            let mut start = 0;
            for end in 1..=self.order.len() {
                let apart = self
                    .order
                    .get(end)
                    .is_none_or(|&(x, _)| self.order[end - 1].0 + error < x - error);
                if !apart {
                    continue;
                }
                let run = &mut self.order[start..end];
                if run.len() > 1 {
                    work.charge(run.len())?;
                    for entry in run.iter_mut() {
                        entry.0 = exact(entry.1);
                    }
                    settle(run, work)?;
                }
                start = end;
            }
        }
        Ok(joined)
    }

    /// The sides of the current sub-slab in `sides`: the pieces across it
    /// in order, coincident ones together, and where coverage starts or
    /// stops, from the windings carried in `segment`. Leaves the windings
    /// right of the pieces.
    fn slab(&mut self, pieces: &[Piece], rules: &[FillRule], segment: usize) {
        let groups = self.windings.len();
        let from = segment.saturating_mul(groups);
        match self.carried.get(from..from.saturating_add(groups)) {
            Some(carried) => self.windings.copy_from_slice(carried),
            None => self.windings.fill(0),
        }
        self.sides.clear();
        // One group, as for most marks: its winding stays in a register.
        if let ([winding], &[rule]) = (&mut self.windings[..], rules) {
            let mut inside = covered(*winding, rule);
            let mut n = 0;
            while let Some(&(x, first)) = self.order.get(n) {
                while let Some(&(_, i)) = self.order.get(n).filter(|o| o.0 == x) {
                    let p = &pieces[i as usize];
                    // Kept for the swaps, where the sweep makes them.
                    if self.keep {
                        self.lefts[n] = *winding;
                        self.at[i as usize] = n;
                    }
                    if p.group == 0 {
                        *winding += i64::from(p.direction);
                    }
                    n += 1;
                }
                let now = covered(*winding, rule);
                if now != inside {
                    self.sides.push((first, if now { 1 } else { -1 }));
                    inside = now;
                }
            }
            return;
        }
        let mut inside = self
            .windings
            .iter()
            .zip(rules)
            .all(|(&w, &rule)| covered(w, rule));
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
    /// end at `a`, and new sides open there.
    fn join(
        &mut self,
        pieces: &[Piece],
        a: f64,
        (out, bytes): (&mut Vec<Line>, &mut Meter),
    ) -> Result<()> {
        for open in &self.open {
            self.slots[open.piece as usize] = Slot {
                side: open.side,
                from: open.from,
                kept: false,
            };
        }
        self.next.clear();
        for &(piece, side) in &self.sides {
            let slot = &mut self.slots[piece as usize];
            slot.kept = slot.side == side;
            let from = if slot.kept { slot.from } else { a };
            self.next.push(Open { piece, side, from });
        }
        for open in self.open.drain(..) {
            let slot = std::mem::take(&mut self.slots[open.piece as usize]);
            if !slot.kept {
                let line = pieces[open.piece as usize].line;
                push_side((&mut *out, &mut *bytes), line, open.side, (open.from, a))?;
            }
        }
        std::mem::swap(&mut self.open, &mut self.next);
        Ok(())
    }

    /// Orders the crossings found by height and `pieces` by lower end, makes
    /// room for their windings and places, finds for each doubtful pair the heights
    /// whose sub-slabs above or below reach into its heights, and counts per
    /// height the pairs reaching it. The crossings are among the heights the
    /// sweep's charge sorts; the pairs' two searches each, their sort and the
    /// count over the heights are charged here.
    fn prepare_swaps(
        &mut self,
        pieces: &[Piece],
        work: &mut Meter,
        bytes: &mut Meter,
    ) -> Result<()> {
        self.crosses.sort_unstable();
        work.charge(sort_work(pieces.len()))?;
        self.lowers.clear();
        room(&mut self.lowers, pieces.len(), bytes)?;
        self.lowers.extend(
            (0_u32..)
                .zip(pieces)
                .map(|(i, p)| u128::from(total_key(p.hi.to_bits())) << 32 | u128::from(i)),
        );
        self.lowers.sort_unstable();
        self.ended = 0;
        let pieces = pieces.len();
        // Every entry is written before it is read, so earlier sweeps' stay.
        if self.lefts.len() < pieces {
            room(&mut self.lefts, pieces, bytes)?;
            room(&mut self.at, pieces, bytes)?;
            self.lefts.resize(pieces, 0);
            self.at.resize(pieces, 0);
        }
        let count = self.heights.len();
        self.bands.clear();
        self.reached.clear();
        self.reaching.clear();
        self.band = 0;
        if self.doubtful.is_empty() {
            return Ok(());
        }
        let searches = self
            .doubtful
            .len()
            .saturating_mul(2 * (count.ilog2() as usize + 1));
        work.charge(
            sort_work(self.doubtful.len())
                .saturating_add(searches)
                .saturating_add(count),
        )?;
        room(&mut self.bands, self.doubtful.len(), bytes)?;
        room(&mut self.reached, count + 1, bytes)?;
        room(&mut self.reaching, self.doubtful.len(), bytes)?;
        self.reached.resize(count + 1, 0);
        let heights = &self.heights;
        let middle = |t: usize| heights[t - 1].midpoint(heights[t]);
        for &(lo, hi, i, j) in &self.doubtful {
            // The heights whose sub-slabs above and below reach into `lo..hi`.
            let above = heights.partition_point(|&h| h <= lo);
            let first = match above {
                0 => 0,
                n if n < count && middle(n) <= lo => n,
                n => n - 1,
            };
            let below = heights.partition_point(|&h| h < hi);
            let last = match below {
                0 => 0,
                n if n < count && middle(n) < hi => n,
                n => n - 1,
            };
            self.bands.push((first, last, i, j));
            self.reached[first] += 1;
            self.reached[last + 1] -= 1;
        }
        self.bands.sort_unstable_by_key(|band| band.0);
        let mut reaching = 0;
        for reached in &mut self.reached {
            reaching += *reached;
            *reached = reaching;
        }
        Ok(())
    }

    /// The crossing after those at or above `a` from `cross` on, passing the
    /// pieces that ended at `a` too.
    fn pass(&mut self, a: f64, mut cross: usize) -> usize {
        let height = u128::from(total_key(a.to_bits()));
        while self.crosses.get(cross).is_some_and(|&c| c >> 64 <= height) {
            cross += 1;
        }
        while self
            .lowers
            .get(self.ended)
            .is_some_and(|&e| e >> 32 <= height)
        {
            self.ended += 1;
        }
        cross
    }

    /// Copies into `pairs` the doubtful pairs that may change places
    /// between the middles around heights `k..=last`, at most as many as
    /// it holds, returning how many: none where more reach. Each pair joins
    /// and leaves the list once, and the list is passed only where few
    /// reach.
    fn reaching(
        &mut self,
        (k, last): (usize, usize),
        pairs: &mut [(u32, u32); 8],
        work: &mut Meter,
    ) -> Result<Option<usize>> {
        let reach = self
            .reached
            .get(k..=last)
            .and_then(|reached| reached.iter().copied().max())
            .map_or(0, |most| usize::try_from(most).unwrap_or(0));
        if reach == 0 {
            return Ok(Some(0));
        }
        if reach > pairs.len() {
            return Ok(None);
        }
        while let Some(&(first, end, i, j)) = self.bands.get(self.band)
            && first <= last
        {
            self.reaching.push((end, i, j));
            self.band += 1;
        }
        work.charge(self.reaching.len())?;
        self.reaching.retain(|&(end, _, _)| end >= k);
        if self.reaching.len() > pairs.len() {
            return Ok(None);
        }
        for (pair, &(_, i, j)) in pairs.iter_mut().zip(&self.reaching) {
            *pair = (i, j);
        }
        Ok(Some(self.reaching.len()))
    }

    /// Takes the sub-slabs from `heights[k]` on, through the heights closer
    /// than twice [`SURE`] after it, from the one above them, where the
    /// heights before and after them are further away and all that happens
    /// there under the nonzero rule is crossings, `crosses[cross..]`, which
    /// [`Self::swap_inside`] swaps, and contours continuing inside the
    /// covered set, whose next piece takes the place of the last. Each
    /// sub-slab it takes is charged at least what placing and joining it
    /// would be. Returns the index of the height after them, how many
    /// crossings they took and how many pieces joined, where it did.
    fn take_swaps(
        &mut self,
        pieces: &[Piece],
        rule: FillRule,
        (k, cross, joined, segment): (usize, usize, usize, usize),
        (out, work, bytes): (&mut Vec<Line>, &mut Meter, &mut Meter),
    ) -> Result<Option<(usize, usize, usize)>> {
        let previous = k
            .checked_sub(1)
            .map_or(f64::NEG_INFINITY, |p| self.heights[p]);
        if self.heights[k] - previous <= 2.0 * SURE || rule != FillRule::NonZero {
            return Ok(None);
        }
        let mut last = k;
        while let Some(&next) = self.heights.get(last + 1)
            && next - self.heights[last] <= 2.0 * SURE
        {
            last += 1;
        }
        // A cluster starts after a gap, so each height is passed once.
        work.charge(last - k + 1)?;
        // Nothing comes in from the left there.
        let (below, Some(&after)) = (self.heights[last], self.heights.get(last + 1)) else {
            return Ok(None);
        };
        if self.carry.get(segment + 1).is_some_and(|&y| y <= below) {
            return Ok(None);
        }
        let until = u128::from(total_key(below.to_bits()));
        let taken = self.crosses[cross..]
            .iter()
            .take_while(|&&c| c >> 64 <= until)
            .count();
        let ending = self.lowers[self.ended..]
            .iter()
            .take_while(|&&e| e >> 32 <= until)
            .count();
        let starting = self.rows[joined..]
            .iter()
            .take_while(|&&(lo, _, _)| lo <= below)
            .count();
        if (taken == 0 && ending == 0) || ending != starting || ending > 8 {
            return Ok(None);
        }
        // The doubtful pairs that may change places there are taken as
        // crossing.
        let mut doubtful = [(0, 0); 8];
        let Some(reaching) = self.reaching((k, last), &mut doubtful, work)? else {
            return Ok(None);
        };
        let doubtful = &doubtful[..reaching];
        // Each piece ending there is continued by one starting at its lower
        // end, away from what else moves there.
        let mid = below.midpoint(after);
        let (mut continued, mut sided) = ([(0, 0); 8], [None; 8]);
        let continued = &mut continued[..ending];
        work.charge(ending.saturating_mul(starting.saturating_add(4)))?;
        for m in 0..ending {
            let end = self.lowers[self.ended + m];
            let piece = u32::try_from(end & u128::from(u32::MAX)).unwrap_or(u32::MAX);
            let starts = &self.rows[joined..joined + starting];
            let Some((next, side)) =
                self.continuation(pieces, (piece, starts), (below, mid), &continued[..m])
            else {
                return Ok(None);
            };
            (continued[m], sided[m]) = (next, side);
        }
        if !self.apart(continued, cross..cross + taken, doubtful) {
            return Ok(None);
        }
        // Per sub-slab a unit per piece placed and joined, one to join, one
        // move per crossing, and the sort and merge of those joining,
        // charged before the swaps are made.
        let after_index = last + 1;
        let joining = if starting == 0 {
            0
        } else {
            sort_work(starting).saturating_add(self.order.len())
        };
        let each = self
            .order
            .len()
            .saturating_mul(2)
            .saturating_add(self.open.len())
            .saturating_add(1)
            .saturating_add(joining);
        let charge = each.saturating_mul(after_index - k).saturating_add(taken);
        work.charge(charge)?;
        if (taken > 0 || !doubtful.is_empty())
            && self
                .swap_inside(pieces, (cross..cross + taken, doubtful), mid)
                .is_none()
        {
            work.refund(charge);
            return Ok(None);
        }
        self.continue_places(pieces, (continued, &sided), (out, bytes))?;
        self.ended += ending;
        // The order is the exact order below them.
        #[cfg(debug_assertions)]
        for pair in self.order.windows(2) {
            let x = |i: u32| pieces[i as usize].line.x(mid);
            debug_assert!(x(pair[0].1) <= x(pair[1].1));
        }
        #[cfg(test)]
        {
            self.swapped += after_index - k;
        }
        Ok(Some((after_index, taken, starting)))
    }

    /// Puts each continued piece's next in its place, and passes the open
    /// sides among them on: each ends where its piece does and opens there
    /// for the next, in order of height, then of place, as joining would.
    fn continue_places(
        &mut self,
        pieces: &[Piece],
        (continued, sided): (&[(usize, u32)], &[Option<usize>]),
        (out, bytes): (&mut Vec<Line>, &mut Meter),
    ) -> Result<()> {
        for &(n, next) in continued {
            self.order[n].1 = next;
            self.at[next as usize] = n;
        }
        let mut sides = [(0.0, 0, 0); 8];
        let mut count = 0;
        for (&(_, next), side) in continued.iter().zip(sided) {
            if let Some(index) = *side {
                sides[count] = (pieces[next as usize].lo, index, next);
                count += 1;
            }
        }
        let sides = &mut sides[..count];
        sides.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for &(at, index, next) in &*sides {
            let open = self.open[index];
            let line = pieces[open.piece as usize].line;
            push_side((&mut *out, &mut *bytes), line, open.side, (open.from, at))?;
            self.open[index] = Open {
                piece: next,
                from: at,
                ..open
            };
        }
        Ok(())
    }

    /// The place of `piece`, which ends at its lower end at or above
    /// `below`, and the piece of `starts` not yet `taken` that continues it
    /// from that point in its direction, beyond `below`, where the next
    /// piece lies strictly between its neighbours at `mid`; and where
    /// `piece` is a side rather than inside the covered set on both sides,
    /// its open side, if it is the side alone at its place.
    fn continuation(
        &self,
        pieces: &[Piece],
        (piece, starts): (u32, &[Span]),
        (below, mid): (f64, f64),
        taken: &[(usize, u32)],
    ) -> Option<((usize, u32), Option<usize>)> {
        let ends = |p: &Piece| {
            let Line { a, b } = p.line;
            if a.y < b.y { (a, b) } else { (b, a) }
        };
        let p = pieces.get(piece as usize)?;
        let n = *self.at.get(piece as usize)?;
        let lower = ends(p).1;
        if self.order.get(n)?.1 != piece || p.hi != lower.y || p.group != 0 {
            return None;
        }
        let &(_, _, next) = starts.iter().find(|&&(lo, hi, q)| {
            let q_piece = &pieces[q as usize];
            ends(q_piece).0 == lower
                && lo == lower.y
                && hi > below
                && q_piece.direction == p.direction
                && q_piece.group == 0
                && taken.iter().all(|&(_, t)| t != q)
        })?;
        // Inside on both sides, or a side alone at its place: then its open
        // side passes to the next piece.
        let w = self.lefts[n];
        let side = if w == 0 || w + i64::from(p.direction) == 0 {
            let alone = |m: usize| self.order.get(m).is_none_or(|o| o.0 != self.order[n].0);
            if !(n.checked_sub(1).is_none_or(alone) && alone(n + 1)) {
                return None;
            }
            Some(self.open.iter().position(|o| o.piece == piece)?)
        } else {
            None
        };
        let x = |i: u32| pieces[i as usize].line.x(mid);
        let at = x(next);
        let between = n.checked_sub(1).is_none_or(|m| x(self.order[m].1) < at)
            && self.order.get(n + 1).is_none_or(|o| at < x(o.1));
        between.then_some(((n, next), side))
    }

    /// Whether the continued places lie two or more apart from each other
    /// and from the places of the pieces crossing, or perhaps changing
    /// places, there, and none of those pieces ends or starts there.
    fn apart(
        &self,
        continued: &[(usize, u32)],
        crossings: std::ops::Range<usize>,
        doubtful: &[(u32, u32)],
    ) -> bool {
        if continued.is_empty() {
            return true;
        }
        let piece = |pair: u128, shift: u32| {
            u32::try_from((pair >> shift) & u128::from(u32::MAX)).unwrap_or(u32::MAX)
        };
        let moving = self.crosses[crossings]
            .iter()
            .flat_map(|&pair| [piece(pair, 32), piece(pair, 0)])
            .chain(doubtful.iter().flat_map(|&(i, j)| [i, j]));
        let continuing = |i: u32| {
            continued
                .iter()
                .any(|&(n, next)| next == i || self.order[n].1 == i)
        };
        for i in moving {
            if continuing(i) {
                return false;
            }
            if let Some(&m) = self.at.get(i as usize)
                && self.order.get(m).is_some_and(|o| o.1 == i)
                && continued.iter().any(|&(n, _)| n.abs_diff(m) < 2)
            {
                return false;
            }
        }
        continued
            .iter()
            .enumerate()
            .all(|(k, &(n, _))| continued[..k].iter().all(|&(m, _)| m.abs_diff(n) >= 2))
    }

    /// Makes the swaps of `crosses[crossings]`, which are the only ones
    /// between the middles of the sub-slabs around them, the lower at
    /// `mid`, where every piece crossing there is inside the covered set
    /// however the pieces crossing beside it are ordered: those pieces take
    /// their order at `mid`, and every side stays open as it is. Returns
    /// whether it did.
    fn swap_inside(
        &mut self,
        pieces: &[Piece],
        (crossings, doubtful): (std::ops::Range<usize>, &[(u32, u32)]),
        mid: f64,
    ) -> Option<()> {
        let direction = |i: u32| {
            let p = &pieces[i as usize];
            if p.group == 0 {
                i64::from(p.direction)
            } else {
                0
            }
        };
        // Where the order holds a piece, and a crossing's.
        let place_of = |i: usize| {
            let n = *self.at.get(i)?;
            (self.order.get(n)?.1 as usize == i).then_some(n)
        };
        let place = |pair: u128, shift: u32| {
            place_of(usize::try_from((pair >> shift) & u128::from(u32::MAX)).ok()?)
        };
        // Inside the covered set whatever the order of pieces from `n` on
        // with these directions.
        let inside = |n: usize, (up, down): (i64, i64)| {
            let w = self.lefts[n];
            w + down > 0 || w + up < 0
        };
        // One crossing, as most are: the two neighbours swap.
        if let ([pair], []) = (&self.crosses[crossings.clone()], doubtful) {
            let pair = *pair;
            let (p, q) = (place(pair, 32)?, place(pair, 0)?);
            let n = p.min(q);
            let (l, r) = (
                direction(self.order.get(n)?.1),
                direction(self.order.get(n + 1)?.1),
            );
            if p.abs_diff(q) != 1 || !inside(n, (l.max(0) + r.max(0), l.min(0) + r.min(0))) {
                return None;
            }
            self.order.swap(n, n + 1);
            let (left, right) = (self.order[n].1, self.order[n + 1].1);
            self.at[left as usize] = n;
            self.at[right as usize] = n + 1;
            self.lefts[n + 1] = self.lefts[n] + r;
            return Some(());
        }
        // The places of the pieces crossing there, and of the neighbours
        // that may change places there.
        let mut places = [0_usize; 16];
        let mut held = 0;
        let mut hold = |n: usize| {
            if !places[..held].contains(&n) {
                *places.get_mut(held)? = n;
                held += 1;
            }
            Some(())
        };
        for &pair in &self.crosses[crossings] {
            hold(place(pair, 32)?)?;
            hold(place(pair, 0)?)?;
        }
        let mut doubted = false;
        for &(i, j) in doubtful {
            if let (Some(p), Some(q)) = (place_of(i as usize), place_of(j as usize)) {
                if p.abs_diff(q) != 1 {
                    return None;
                }
                hold(p)?;
                hold(q)?;
                doubted = true;
            }
        }
        let places = &mut places[..held];
        places.sort_unstable();
        // Pieces that cross pass the pieces between them, so these make runs
        // of neighbours, and nothing else moves. Each run must be inside the
        // covered set however its pieces are ordered: in each sub-slab there
        // they are in some order, and every side stays as it is.
        let places = &*places;
        let runs = || {
            places
                .chunk_by(|a, b| *b == a + 1)
                .map(|run| run[0]..run[run.len() - 1] + 1)
        };
        for run in runs() {
            let start = run.start;
            let ways = self.order[run].iter().fold((0, 0), |(up, down), &(_, i)| {
                let d = direction(i);
                (up + d.max(0), down + d.min(0))
            });
            if !inside(start, ways) {
                return None;
            }
        }
        // The heights around them are further from them than any crossing
        // found from where its pieces cross, so the runs take their order
        // below them, and the others keep theirs.
        for run in runs() {
            let w = self.lefts[run.start];
            let entries = &mut self.order[run.clone()];
            // Two pieces that cross swap; more, and those that may change
            // places, take their exact places.
            if let ([one, other], false) = (&mut *entries, doubted) {
                std::mem::swap(one, other);
            } else {
                for entry in entries.iter_mut() {
                    entry.0 = pieces[entry.1 as usize].line.x(mid);
                }
                entries.sort_unstable_by(ordered);
            }
            let mut winding = w;
            for (n, &(_, i)) in run.zip(entries.iter()) {
                self.at[i as usize] = n;
                self.lefts[n] = winding;
                winding += direction(i);
            }
        }
        Some(())
    }
}

/// The heights within `lo..hi` where two pieces whose places differ by
/// `d0` at `lo` and `d1` at `hi`, each within `error`, may cross further
/// than [`SURE`] from the crossing found, or, coming that close at an end
/// without one, near that end: a crossing found lies within the pair's
/// height times twice `error` over the change of their difference of where
/// they cross. Even two edges from one vertex are placed only so closely
/// near it. The same edge twice, as the shared sides of a union give, and
/// two verticals keep their order.
fn doubtful(
    (p, q): (Line, Line),
    (lo, hi): (f64, f64),
    (d0, d1): (f64, f64),
    error: f64,
) -> Option<(f64, f64)> {
    let crossing = (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0);
    if !crossing && d0.abs().min(d1.abs()) > 2.0 * error {
        return None;
    }
    let apart = ((d0 - d1).abs() - 2.0 * error).max(0.0);
    let reach = (hi - lo) * 2.0 * error;
    let parallel = (p.a.x == p.b.x && q.a.x == q.b.x)
        || (p.a == q.a && p.b == q.b)
        || (p.a == q.b && p.b == q.a);
    if reach <= SURE / 2.0 * apart || parallel {
        return None;
    }
    let doubt = reach / apart + SURE / 2.0;
    let near = if crossing {
        lo + (hi - lo) * (d0 / (d0 - d1))
    } else if d0.abs() <= d1.abs() {
        lo
    } else {
        hi
    };
    Some((greater(lo, near - doubt), lesser(hi, near + doubt)))
}

/// `piece`, whose x at its ends is `(start, end)`, within columns
/// `left..right`: its heights where its x is there, cut where it crosses
/// `left` or `right`, at the same heights for the chunks on either side.
fn clip(piece: Piece, (start, end): (f64, f64), (left, right): (f64, f64)) -> Option<Piece> {
    // A vertical edge has no inverse crossing. Chunks own their left side
    // and exclude their right, independently of the contour's direction.
    if piece.line.a.x == piece.line.b.x {
        return (left <= piece.line.a.x && piece.line.a.x < right).then_some(piece);
    }
    let rising = end > start;
    let (mut lo, mut hi) = (piece.lo, piece.hi);
    if start.min(end) < left {
        let y = piece.line.y(left).clamp(lo, hi);
        if rising {
            lo = y;
        } else {
            hi = y;
        }
    }
    if start.max(end) >= right {
        let y = piece.line.y(right).clamp(lo, hi);
        if rising {
            hi = y;
        } else {
            lo = y;
        }
    }
    (lo < hi).then_some(Piece { lo, hi, ..piece })
}

/// The total order of f64, as `f64::total_cmp` gives it, as unsigned
/// integers; the map is its own inverse once the top bit is flipped back.
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "bit reinterpretation of the total order"
)]
fn total_key(bits: u64) -> u64 {
    let bits = bits as i64;
    (bits ^ (((bits >> 63) as u64) >> 1) as i64) as u64 ^ (1 << 63)
}

#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "bit reinterpretation of the total order"
)]
fn from_total_key(key: u64) -> u64 {
    let bits = (key ^ (1 << 63)) as i64;
    (bits ^ (((bits >> 63) as u64) >> 1) as i64) as u64
}

/// Sorts `values` in the total order of f64 through integer keys: the same
/// values in the same order as sorting them by `f64::total_cmp`.
fn sort_values(values: &mut [f64], keys: &mut Vec<u64>) {
    keys.clear();
    keys.extend(values.iter().map(|v| total_key(v.to_bits())));
    keys.sort_unstable();
    for (value, &key) in values.iter_mut().zip(keys.iter()) {
        *value = f64::from_bits(from_total_key(key));
    }
}

/// Sorts `spans`, listed by piece, by start in the total order of f64 and
/// then by piece, through one integer key each: the order sorting them by
/// `f64::total_cmp` and then piece gives.
fn sort_spans(spans: &mut Vec<Span>, (keys, sorted): (&mut Vec<u128>, &mut Vec<Span>)) {
    keys.clear();
    keys.extend(
        spans
            .iter()
            .map(|&(start, _, i)| u128::from(total_key(start.to_bits())) << 32 | u128::from(i)),
    );
    keys.sort_unstable();
    sorted.clear();
    sorted.extend(keys.iter().map(|&key| {
        // The low word is the piece, and the spans are listed by piece.
        let piece = u32::try_from(key & u128::from(u32::MAX)).unwrap_or(u32::MAX);
        spans[piece as usize]
    }));
    std::mem::swap(spans, sorted);
}

/// The pairs of `spans`, ordered by start, that overlap: for each, the
/// later ones starting at or before its end.
fn overlapping(spans: &[Span]) -> usize {
    spans
        .iter()
        .enumerate()
        .map(|(n, &(_, end, _))| spans[n + 1..].partition_point(|s| s.0 <= end))
        .fold(0, usize::saturating_add)
}

/// Places the pieces from above and those joining by `x`, restores the
/// order of the former and merges in the latter, charging each stage before
/// it runs, including a plain pass that must later be retried exactly.
fn arrange(
    (order, joining): (&mut Vec<Place>, &mut [Place]),
    x: impl Fn(u32) -> f64,
    work: &mut Meter,
) -> Result<()> {
    if !joining.is_empty() {
        work.charge(sort_work(joining.len()).saturating_add(order.len()))?;
    }
    for entry in order.iter_mut() {
        entry.0 = x(entry.1);
    }
    settle(order, work)?;
    if !joining.is_empty() {
        for entry in joining.iter_mut() {
            entry.0 = x(entry.1);
        }
        joining.sort_unstable_by(ordered);
        merge(order, joining);
    }
    Ok(())
}

/// An edge prepared for the cell kernel's plain interpolation, and how far
/// that can be from [`Line::x`]; the exact form where it does not apply.
#[derive(Debug, Clone, Copy)]
struct Lane {
    top: crate::Point,
    slope: f64,
    range: (f64, f64),
    error: f64,
    exact: bool,
}

impl Lane {
    fn new(line: Line) -> Self {
        let (top, bottom) = if line.a.y < line.b.y {
            (line.a, line.b)
        } else {
            (line.b, line.a)
        };
        let height = bottom.y - top.y;
        let exact = !line.plain();
        Self {
            top,
            slope: (bottom.x - top.x) / height,
            range: (top.x.min(bottom.x), top.x.max(bottom.x)),
            error: if exact {
                0.0
            } else {
                2.0 * line.interpolation_error().x
            },
            exact,
        }
    }

    /// The edge's x at a height within it. The edge is read only for the
    /// exact form, so it is passed by reference.
    fn x(&self, line: &Line, y: f64) -> f64 {
        if self.exact {
            line.x(y)
        } else {
            (self.top.x + (y - self.top.y) * self.slope)
                .max(self.range.0)
                .min(self.range.1)
        }
    }
}

/// Restores the order of `order` by moving entries down one at a time:
/// pieces that crossed between sub-slabs are one move apart per crossing.
/// Reserve the remaining work before starting, spend at most that many
/// moves, then return the unused reservation to the meter.
fn settle(order: &mut [Place], work: &mut Meter) -> Result<()> {
    // Before the first place not strictly above the one before it, the
    // entries are in order whatever their indices: no move to make there.
    let Some(first) = order
        .windows(2)
        .position(|w| w[0].0.partial_cmp(&w[1].0) != Some(std::cmp::Ordering::Less))
    else {
        return Ok(());
    };
    // Distinct places compare as floats; equal or unordered ones by their
    // total order and index.
    let before = |p: &Place, q: &Place| {
        if p.0 < q.0 {
            true
        } else if p.0 > q.0 {
            false
        } else {
            ordered(p, q).is_lt()
        }
    };
    let mut remaining = work.remaining();
    work.charge(remaining)?;
    for n in first + 1..order.len() {
        let mut j = n;
        while j > 0 && before(&order[j], &order[j - 1]) {
            if remaining == 0 {
                // The reservation exhausted the meter; refuse before the move.
                return work.charge(1);
            }
            remaining -= 1;
            order.swap(j - 1, j);
            j -= 1;
        }
    }
    work.refund(remaining);
    Ok(())
}

/// Merges the ordered `joining` into the ordered `order`, which has room
/// for them.
fn merge(order: &mut Vec<Place>, joining: &[Place]) {
    let (mut i, mut j) = (order.len(), joining.len());
    order.resize(i + j, (0.0, 0));
    for k in (0..order.len()).rev() {
        if j == 0 {
            break;
        }
        if i > 0 && ordered(&joining[j - 1], &order[i - 1]).is_lt() {
            order[k] = order[i - 1];
            i -= 1;
        } else {
            order[k] = joining[j - 1];
            j -= 1;
        }
    }
}

/// Appends the side of `line` over heights `from..to`: downward when
/// coverage starts at it (`side` +1), upward when it stops.
fn push_side(
    (out, bytes): (&mut Vec<Line>, &mut Meter),
    line: Line,
    side: i32,
    (from, to): (f64, f64),
) -> Result<()> {
    let (upper, lower) = if line.a.y < line.b.y {
        (line.a, line.b)
    } else {
        (line.b, line.a)
    };
    let from = greater(from, upper.y);
    let to = lesser(to, lower.y);
    if from >= to {
        return Ok(());
    }
    let a = crate::Point::new(line.x(from), from);
    let b = crate::Point::new(line.x(to), to);
    room(out, 1, bytes)?;
    out.push(if side > 0 {
        Line { a, b }
    } else {
        Line { a: b, b: a }
    });
    Ok(())
}

fn covered(winding: i64, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}

#[cfg(test)]
mod tests;
