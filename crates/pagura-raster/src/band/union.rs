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

/// The pieces a row takes in one sweep: a row of more is cut into chunks.
const CHUNK: usize = 64;

/// A piece's current midpoint key, evaluated only when a comparison needs it.
/// Joining pieces are ready before their sort; survivors are invalidated in
/// the existing retain visit. Bit-constant verticals keep their exact key.
/// Both flags occupy the old tuple's padding.
#[derive(Debug, Clone, Copy, Default)]
struct Place {
    x: f64,
    piece: u32,
    ready: bool,
    constant: bool,
}

const _: () = assert!(size_of::<Place>() == size_of::<(f64, u32)>());
const _: () = assert!(align_of::<Place>() == align_of::<(f64, u32)>());

impl Place {
    fn new(piece: u32, pieces: &[Piece], mid: f64) -> Self {
        let constant =
            pieces[piece as usize].line.a.x.to_bits() == pieces[piece as usize].line.b.x.to_bits();
        Self {
            x: pieces[piece as usize].line.x(mid),
            piece,
            ready: true,
            constant,
        }
    }

    fn key(&mut self, pieces: &[Piece], mid: f64) -> f64 {
        if !self.ready {
            self.x = pieces[self.piece as usize].line.x(mid);
            self.ready = true;
        }
        self.x
    }
}

fn ordered(p: &Place, q: &Place) -> std::cmp::Ordering {
    p.x.total_cmp(&q.x).then(p.piece.cmp(&q.piece))
}

/// A strict gap between precomputed enclosures proves order and
/// non-equality. Touching or overlapping ranges use the original keys.
fn separated(p: Span, q: Span) -> Option<std::cmp::Ordering> {
    if p.1 < q.0 {
        Some(std::cmp::Ordering::Less)
    } else if q.1 < p.0 {
        Some(std::cmp::Ordering::Greater)
    } else {
        None
    }
}

fn compare(
    p: &mut Place,
    q: &mut Place,
    pieces: &[Piece],
    columns: &[Span],
    mid: f64,
) -> std::cmp::Ordering {
    if !(p.ready && q.ready)
        && let Some(order) = separated(columns[p.piece as usize], columns[q.piece as usize])
    {
        return order;
    }
    p.key(pieces, mid)
        .total_cmp(&q.key(pieces, mid))
        .then(p.piece.cmp(&q.piece))
}

fn coincident(p: &mut Place, q: &mut Place, pieces: &[Piece], columns: &[Span], mid: f64) -> bool {
    if !(p.ready && q.ready)
        && separated(columns[p.piece as usize], columns[q.piece as usize]).is_some()
    {
        return false;
    }
    // Numeric equality deliberately groups the two signs of zero together.
    p.key(pieces, mid) == q.key(pieces, mid)
}

/// A piece's extent along one axis, `(start, end, piece)`.
type Span = (f64, f64, u32);

/// Encloses every literal midpoint key over this piece's heights, reusing
/// the endpoint keys already evaluated for crossing enumeration.
fn key_bounds(piece: &Piece, (left, right): (f64, f64)) -> (f64, f64) {
    let Line { a, b } = piece.line;
    if ![a.x, b.x].into_iter().all(f64::is_finite) {
        return (f64::NEG_INFINITY, f64::INFINITY);
    }
    let full = (a.x.min(b.x), a.x.max(b.x));
    let (top, bottom) = (a.y.min(b.y), a.y.max(b.y));
    if ![a.x, a.y, b.x, b.y]
        .into_iter()
        .all(|value| value.abs() <= 1.0e12)
        || !(top < bottom && top <= piece.lo && piece.lo < piece.hi && piece.hi <= bottom)
    {
        return full;
    }
    // For this guarded domain the literal normalized interpolation is
    // within 384*2^-53*M of the exact affine line, M=1+|a.x|+|b.x|.
    // This new comparison-only allowance exceeds 500*2^-53*M, including
    // its own rounding. One allowance transports an endpoint and one an
    // interior key. Computed interpolation need not be monotone.
    let error = (256.0 * f64::EPSILON) * ((1.0 + a.x.abs()) + b.x.abs());
    let radius = 2.0 * error;
    (
        (left - radius).next_down().max(full.0),
        (right + radius).next_up().min(full.1),
    )
}

/// Reuses the finished crossing table as ID-indexed key enclosures. Each
/// swap fixes a new ID, so there are at most N-1 swaps and N fixed bodies.
/// No already fixed slot can be displaced by another unique ID.
fn index_columns(columns: &mut [Span], pieces: &[Piece]) {
    debug_assert_eq!(columns.len(), pieces.len());
    for i in 0..columns.len() {
        while columns[i].2 as usize != i {
            let target = columns[i].2 as usize;
            columns.swap(i, target);
        }
        let bounds = key_bounds(&pieces[i], (columns[i].0, columns[i].1));
        (columns[i].0, columns[i].1) = bounds;
    }
}

/// Work units for sorting `count` values and passing over them once.
fn sort_work(count: usize) -> usize {
    count.saturating_mul(count.max(1).ilog2() as usize + 2)
}

/// Reusable storage of the resolver.
#[derive(Debug, Default)]
pub(crate) struct Resolver {
    #[cfg(test)]
    eager_midpoint_keys: bool,
    #[cfg(test)]
    original_height_sort: bool,
    #[cfg(test)]
    original_crossings: bool,
    heights: Vec<f64>,
    /// Column spans by start during crossing enumeration, then enclosing
    /// column spans indexed by piece ID for comparisons.
    columns: Vec<Span>,
    /// Height spans by start: the sweep takes pieces by their upper end.
    rows: Vec<Span>,
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
    spans: Vec<Span>,
    lasts: Vec<f64>,
    cuts: Vec<f64>,
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
        self.spans.clear();
        self.lasts.clear();
        for (i, p) in (0_u32..).zip(pieces) {
            let (left, right) = p.columns();
            let (first, last) = (left.floor(), right.floor());
            self.spans.push((first, last, i));
            self.lasts.push(last);
        }
        self.spans
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        self.lasts.sort_unstable_by(f64::total_cmp);
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
                let piece = pieces[self.spans[n as usize].2 as usize];
                local.extend(clip(piece, (left, right)));
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
        let breaks = self.carry.len().saturating_sub(1);
        room(&mut self.heights, 2 * count + 2 + breaks, bytes)?;
        self.heights.clear();
        self.heights.extend([top, top + 1.0]);
        self.heights.extend_from_slice(&self.carry[1..]);
        // Start heights will already be ordered in `rows`. Keep them
        // implicit until the final merge instead of sorting them twice.
        let starts = count;
        #[cfg(test)]
        let starts = if self.original_height_sort { 0 } else { starts };
        for p in pieces {
            #[cfg(test)]
            if self.original_height_sort {
                self.heights.push(p.lo);
            }
            self.heights.push(p.hi);
        }
        self.crossings(pieces, total, starts, (work, bytes))?;
        work.charge(sort_work(self.heights.len() + starts))?;
        self.heights.sort_unstable_by(f64::total_cmp);
        if starts != 0 {
            merge_heights(&mut self.heights, &self.rows);
        }
        self.heights.dedup();
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
        let (mut joined, mut segment) = (0, 0);
        for k in 1..self.heights.len() {
            let (a, b) = (self.heights[k - 1], self.heights[k]);
            if b > a {
                while self.carry.get(segment + 1).is_some_and(|&y| y <= a) {
                    segment += 1;
                }
                joined = self.place(pieces, (a, b), joined, work)?;
                work.charge(
                    self.order
                        .len()
                        .saturating_add(self.open.len())
                        .saturating_add(1),
                )?;
                self.slab(pieces, rules, segment, a.midpoint(b));
                if carry {
                    self.carry_on(a, bytes)?;
                }
                self.join(pieces, a, (out, bytes))?;
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

    /// Adds the heights where two pieces cross strictly inside both. Only
    /// pairs overlapping in both columns and heights can cross; they are
    /// enumerated along the axis where fewer pairs overlap, so that pieces
    /// side by side across the row, or short ones one below another, cost
    /// about their sort.
    fn crossings(
        &mut self,
        pieces: &[Piece],
        total: u32,
        starts: usize,
        (work, bytes): (&mut Meter, &mut Meter),
    ) -> Result<()> {
        self.columns.clear();
        self.rows.clear();
        for (i, p) in (0..total).zip(pieces) {
            let (left, right) = p.columns();
            self.columns.push((left, right, i));
            self.rows.push((p.lo, p.hi, i));
        }
        let by_start = |a: &Span, b: &Span| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2));
        self.columns.sort_unstable_by(by_start);
        self.rows.sort_unstable_by(by_start);
        let (across, down) = (overlapping(&self.columns), overlapping(&self.rows));
        let by_rows = down < across;
        work.charge(across.min(down))?;
        // Columns become free after their enumeration, or immediately if
        // rows enumerate the pairs. Reuse them exactly once. The original
        // four sort packets cover columns sort + initialization + bounds,
        // rows sort + cycle visits + swaps, and each overlap count's
        // binary queries + fold: 4*N*(floor(log2(N))+2) abstract units.
        if by_rows {
            index_columns(&mut self.columns, pieces);
        }
        let filter = by_rows;
        #[cfg(test)]
        let filter = filter && !self.original_crossings;
        let spans = if by_rows { &self.rows } else { &self.columns };
        for (n, &(_, end, i)) in spans.iter().enumerate() {
            let p = pieces[i as usize];
            for &(start, _, j) in &spans[n + 1..] {
                if start > end {
                    break;
                }
                let q = pieces[j as usize];
                let lo = p.lo.max(q.lo);
                let hi = p.hi.min(q.hi);
                if lo >= hi {
                    continue;
                }
                // Both overlap endpoints belong to both pieces. Strict
                // separation over their whole intervals makes both old
                // endpoint differences have the same weak sign; neither
                // a strict crossing nor a height insertion is possible.
                if filter && separated(self.columns[i as usize], self.columns[j as usize]).is_some()
                {
                    continue;
                }
                let d0 = p.line.x(lo) - q.line.x(lo);
                let d1 = p.line.x(hi) - q.line.x(hi);
                if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
                    let y = lo + (hi - lo) * (d0 / (d0 - d1));
                    if y > lo && y < hi {
                        // Implicit starts still own their original slots:
                        // reserve exactly the old full-list requirement.
                        room(&mut self.heights, starts + 1, bytes)?;
                        self.heights.push(y);
                    }
                }
            }
        }
        if !by_rows {
            index_columns(&mut self.columns, pieces);
        }
        Ok(())
    }

    /// Orders the pieces spanning sub-slab `a..b`: those from above placed
    /// anew and dropped where they ended, their order restored (they swap
    /// only where they crossed, at `a`), and those starting at `a` merged
    /// in. Returns how many pieces, by upper end, have joined.
    fn place(
        &mut self,
        pieces: &[Piece],
        (a, b): (f64, f64),
        mut joined: usize,
        work: &mut Meter,
    ) -> Result<usize> {
        let mid = a.midpoint(b);
        work.charge(self.order.len())?;
        self.order.retain_mut(|entry| {
            entry.ready = entry.constant;
            #[cfg(test)]
            if self.eager_midpoint_keys {
                entry.ready = false;
                entry.key(pieces, mid);
            }
            pieces[entry.piece as usize].hi >= b
        });
        settle(&mut self.order, pieces, &self.columns, mid, work)?;
        self.joining.clear();
        while let Some(&(lo, hi, i)) = self.rows.get(joined)
            && lo <= a
        {
            if hi >= b {
                self.joining.push(Place::new(i, pieces, mid));
            }
            joined += 1;
        }
        if !self.joining.is_empty() {
            work.charge(sort_work(self.joining.len()).saturating_add(self.order.len()))?;
            self.joining.sort_unstable_by(ordered);
            merge(&mut self.order, &self.joining, pieces, &self.columns, mid);
        }
        Ok(joined)
    }

    /// The sides of the current sub-slab in `sides`: the pieces across it
    /// in order, coincident ones together, and where coverage starts or
    /// stops, from the windings carried in `segment`. Leaves the windings
    /// right of the pieces.
    fn slab(&mut self, pieces: &[Piece], rules: &[FillRule], segment: usize, mid: f64) {
        let groups = self.windings.len();
        let from = segment.saturating_mul(groups);
        match self.carried.get(from..from.saturating_add(groups)) {
            Some(carried) => self.windings.copy_from_slice(carried),
            None => self.windings.fill(0),
        }
        self.sides.clear();
        let mut inside = self
            .windings
            .iter()
            .zip(rules)
            .all(|(&w, &rule)| covered(w, rule));
        let mut n = 0;
        while let Some(first) = self.order.get(n).map(|entry| entry.piece) {
            let start = n;
            // Finite validated pieces always equal themselves. Consume the
            // first winding without forcing a separated singleton's key.
            loop {
                let p = pieces[self.order[n].piece as usize];
                if let Some(w) = self.windings.get_mut(usize::from(p.group)) {
                    *w += i64::from(p.direction);
                }
                n += 1;
                if n == self.order.len() {
                    break;
                }
                let (before, after) = self.order.split_at_mut(n);
                if !coincident(
                    &mut before[start],
                    &mut after[0],
                    pieces,
                    &self.columns,
                    mid,
                ) {
                    break;
                }
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
}

/// `piece` within columns `left..right`: its heights where its x is there,
/// cut where it crosses `left` or `right`, at the same heights for the
/// chunks on either side.
fn clip(piece: Piece, (left, right): (f64, f64)) -> Option<Piece> {
    // A vertical edge has no inverse crossing. Chunks own their left side
    // and exclude their right, independently of the contour's direction.
    if piece.line.a.x == piece.line.b.x {
        return (left <= piece.line.a.x && piece.line.a.x < right).then_some(piece);
    }
    let (start, end) = (piece.line.x(piece.lo), piece.line.x(piece.hi));
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

/// The pairs of `spans`, ordered by start, that overlap: for each, the
/// later ones starting at or before its end.
fn overlapping(spans: &[Span]) -> usize {
    spans
        .iter()
        .enumerate()
        .map(|(n, &(_, end, _))| spans[n + 1..].partition_point(|s| s.0 <= end))
        .fold(0, usize::saturating_add)
}

/// Merges the already ordered row starts into the other sorted heights.
/// Every implicit start retained its slot in the original height capacity;
/// crossing insertion reserves against that full length too.
fn merge_heights(heights: &mut Vec<f64>, rows: &[Span]) {
    let (mut i, mut j) = (heights.len(), rows.len());
    debug_assert!(heights.capacity() >= i + j);
    heights.resize(i + j, 0.0);
    for k in (0..heights.len()).rev() {
        if j == 0 {
            break;
        }
        if i > 0 && rows[j - 1].0.total_cmp(&heights[i - 1]).is_lt() {
            heights[k] = heights[i - 1];
            i -= 1;
        } else {
            heights[k] = rows[j - 1].0;
            j -= 1;
        }
    }
}

/// Restores the order of `order` by moving entries down one at a time,
/// charging each move: pieces that crossed at the height between two
/// sub-slabs are one move apart per crossing.
fn settle(
    order: &mut [Place],
    pieces: &[Piece],
    columns: &[Span],
    mid: f64,
    work: &mut Meter,
) -> Result<()> {
    for n in 1..order.len() {
        let mut j = n;
        while j > 0 && {
            let (before, after) = order.split_at_mut(j);
            compare(&mut after[0], &mut before[j - 1], pieces, columns, mid).is_lt()
        } {
            work.charge(1)?;
            order.swap(j - 1, j);
            j -= 1;
        }
    }
    Ok(())
}

/// Merges the ordered `joining` into the ordered `order`, which has room
/// for them.
fn merge(order: &mut Vec<Place>, joining: &[Place], pieces: &[Piece], columns: &[Span], mid: f64) {
    let (mut i, mut j) = (order.len(), joining.len());
    order.resize(i + j, Place::default());
    for k in (0..order.len()).rev() {
        if j == 0 {
            break;
        }
        let mut next = joining[j - 1];
        if i > 0 && compare(&mut next, &mut order[i - 1], pieces, columns, mid).is_lt() {
            order[k] = order[i - 1];
            i -= 1;
        } else {
            order[k] = next;
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
    let from = from.max(upper.y);
    let to = to.min(lower.y);
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
