//! The exact local resolver: the covered area of one cell whose windings
//! the signed area alone does not determine.
//!
//! The cell's edges are given in cell coordinates (`[0, 1]²`), each with
//! the winding it adds to the region right of it and its group. A point is
//! covered when it is inside every group under that group's fill rule (a
//! mark and the paths of its clip, 8.5.4). Between consecutive heights of
//! end points and crossings no two edges cross, so the widths between
//! neighbours are linear there and the midpoint rule integrates them
//! exactly.

use super::row::fold;
use crate::FillRule;
use pagura_core::{Error, Meter, Result};

/// One edge in cell coordinates, `y0 < y1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LocalEdge {
    pub(crate) x0: f64,
    pub(crate) y0: f64,
    pub(crate) x1: f64,
    pub(crate) y1: f64,
    /// The winding added to the region right of the edge.
    pub(crate) winding: i32,
    pub(crate) group: u8,
}

impl LocalEdge {
    fn x(self, y: f64) -> f64 {
        if y <= self.y0 {
            self.x0
        } else if y >= self.y1 {
            self.x1
        } else {
            self.x0 + (y - self.y0) / (self.y1 - self.y0) * (self.x1 - self.x0)
        }
    }
}

/// Reusable resolver storage.
#[derive(Debug, Default)]
pub(crate) struct Resolver {
    heights: Vec<f64>,
    /// The edges by their lower end.
    rising: Vec<u32>,
    /// The edges spanning the current slab by their places across it.
    order: Vec<Place>,
    /// The edges joining at the current slab.
    joining: Vec<Place>,
    windings: Vec<i64>,
}

/// An edge's place across a slab, at the slab's middle height, and its
/// index.
type Place = (f64, u32);

/// Work units for sorting `count` values and passing over them once.
fn sort_work(count: usize) -> usize {
    count.saturating_mul(count.max(1).ilog2() as usize + 2)
}

impl Resolver {
    /// The covered area of the cell within heights `span`, quantized.
    ///
    /// The edges are swept by height, those spanning the current slab kept
    /// in order across it. Each stage is charged to `meter` before it runs,
    /// with what it does on these edges: the sort of the edges by their
    /// lower ends, the crossing tests of the pairs whose heights overlap,
    /// the sort of the heights found, and per slab a unit per edge spanning
    /// it, one per pair that crossed below it and a sort of those joining.
    /// Edges that meet without crossing (the floor cells of points, the
    /// shared sides of a union) add no slab, and an edge costs the slabs it
    /// spans, not every slab of the cell.
    ///
    /// # Errors
    ///
    /// Returns the meter's `Limit` error, or one for failed allocations.
    pub(crate) fn coverage(
        &mut self,
        edges: &[LocalEdge],
        rules: &[FillRule],
        span: (f64, f64),
        meter: &mut Meter,
    ) -> Result<u8> {
        let area = self.area(edges, rules, span, meter)?;
        Ok(fold(area, FillRule::NonZero))
    }

    fn area(
        &mut self,
        edges: &[LocalEdge],
        rules: &[FillRule],
        span: (f64, f64),
        meter: &mut Meter,
    ) -> Result<f64> {
        let count =
            u32::try_from(edges.len()).map_err(|_| Error::limit("cell edges exceed u32"))?;
        // The sort, and a search for the overlapping pairs per edge.
        meter.charge_product(sort_work(edges.len()), 2)?;
        self.rising.clear();
        self.rising
            .try_reserve(edges.len())
            .map_err(|_| Error::limit("cell resolver allocation failed"))?;
        self.rising.extend(0..count);
        self.rising
            .sort_unstable_by(|&a, &b| edges[a as usize].y0.total_cmp(&edges[b as usize].y0));
        self.heights(edges, span, meter)?;
        meter.charge(sort_work(self.heights.len()))?;
        self.heights.sort_unstable_by(f64::total_cmp);
        self.heights.dedup();
        self.integrate(edges, rules, span, meter)
    }

    /// Collects the heights within `span` where an edge ends or two cross,
    /// and the span's own ends.
    fn heights(
        &mut self,
        edges: &[LocalEdge],
        (bottom, top): (f64, f64),
        meter: &mut Meter,
    ) -> Result<()> {
        let allocation = |_| Error::limit("cell resolver allocation failed");
        self.heights.clear();
        self.heights
            .try_reserve(edges.len().saturating_mul(2).saturating_add(2))
            .map_err(allocation)?;
        self.heights.extend([bottom, top]);
        for e in edges {
            for y in [e.y0, e.y1] {
                if y > bottom && y < top {
                    self.heights.push(y);
                }
            }
        }
        for (n, &i) in self.rising.iter().enumerate() {
            let a = edges[i as usize];
            // Later edges start no lower; those starting at or above `a`'s
            // upper end share no height range with it.
            let later = &self.rising[n + 1..];
            let overlapping = later.partition_point(|&j| edges[j as usize].y0 < a.y1);
            meter.charge(overlapping)?;
            for &j in &later[..overlapping] {
                let b = edges[j as usize];
                let lo = a.y0.max(b.y0);
                let hi = a.y1.min(b.y1);
                if lo >= hi {
                    continue;
                }
                let d0 = a.x(lo) - b.x(lo);
                let d1 = a.x(hi) - b.x(hi);
                if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
                    let y = lo + (hi - lo) * (d0 / (d0 - d1));
                    if y > lo && y < hi && y > bottom && y < top {
                        self.heights.try_reserve(1).map_err(allocation)?;
                        self.heights.push(y);
                    }
                }
            }
        }
        Ok(())
    }

    /// Integrates the covered width over the slabs between the sorted
    /// heights, keeping the edges that span the current slab: an edge joins
    /// once the slab's lower height reaches its lower end and leaves for
    /// good once the slab's upper height passes its upper end.
    fn integrate(
        &mut self,
        edges: &[LocalEdge],
        rules: &[FillRule],
        (bottom, top): (f64, f64),
        meter: &mut Meter,
    ) -> Result<f64> {
        let allocation = |_| Error::limit("cell resolver allocation failed");
        self.windings.clear();
        self.windings.try_reserve(rules.len()).map_err(allocation)?;
        self.windings.resize(rules.len(), 0);
        self.order.clear();
        self.joining.clear();
        self.order
            .try_reserve(edges.len())
            .and_then(|()| self.joining.try_reserve(edges.len()))
            .map_err(allocation)?;
        // Every edge joins once at most.
        meter.charge(edges.len())?;
        let mut joined = 0;
        let mut area = 0.0;
        for pair in self.heights.windows(2) {
            let (lo, hi) = (pair[0], pair[1]);
            if hi <= lo || lo < bottom || hi > top {
                continue;
            }
            let mid = lo.midpoint(hi);
            let place = |i: u32| (edges[i as usize].x(mid).clamp(0.0, 1.0), i);
            // A unit per edge spanning the slab below, placed anew and
            // dropped when it ended, then checked and swept in order: the
            // others keep their order but where they crossed.
            meter.charge(self.order.len())?;
            self.order.retain_mut(|entry| {
                *entry = place(entry.1);
                edges[entry.1 as usize].y1 >= hi
            });
            settle(&mut self.order, meter)?;
            self.joining.clear();
            while let Some(&i) = self.rising.get(joined)
                && edges[i as usize].y0 <= lo
            {
                if edges[i as usize].y1 >= hi {
                    self.joining.push(place(i));
                }
                joined += 1;
            }
            if !self.joining.is_empty() {
                meter.charge(sort_work(self.joining.len()).saturating_add(self.order.len()))?;
                self.joining.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
                merge(&mut self.order, &self.joining);
            }
            self.windings.fill(0);
            let mut width = 0.0;
            let mut previous = 0.0;
            let mut inside = false;
            for &(x, i) in &self.order {
                if inside {
                    width += x - previous;
                }
                let e = edges[i as usize];
                if let Some(w) = self.windings.get_mut(usize::from(e.group)) {
                    *w += i64::from(e.winding);
                }
                inside = self
                    .windings
                    .iter()
                    .zip(rules)
                    .all(|(&w, &rule)| covered(w, rule));
                previous = x;
            }
            if inside {
                width += 1.0 - previous;
            }
            area += width * (hi - lo);
        }
        Ok(area)
    }
}

/// Restores the order of `order` by moving entries down one at a time,
/// charging each move: edges that crossed at the height between two slabs
/// are one move apart per crossing. Ties may end in any order: they add no
/// width between them, and the windings after them are their sum.
fn settle(order: &mut [Place], meter: &mut Meter) -> Result<()> {
    for n in 1..order.len() {
        let mut j = n;
        while j > 0 && order[j - 1].0.total_cmp(&order[j].0).is_gt() {
            meter.charge(1)?;
            order.swap(j - 1, j);
            j -= 1;
        }
    }
    Ok(())
}

/// Merges the sorted `joining` into the sorted `order`, from the back;
/// `order` has room for them.
fn merge(order: &mut Vec<Place>, joining: &[Place]) {
    let mut kept = order.len();
    order.extend_from_slice(joining);
    let mut taken = joining.len();
    while taken > 0 {
        let next = joining[taken - 1];
        let write = kept + taken - 1;
        if kept > 0 && order[kept - 1].0.total_cmp(&next.0).is_gt() {
            order[write] = order[kept - 1];
            kept -= 1;
        } else {
            order[write] = next;
            taken -= 1;
        }
    }
}

fn covered(winding: i64, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}

#[cfg(test)]
mod tests;
