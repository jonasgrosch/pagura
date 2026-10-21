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
    order: Vec<(f64, u32)>,
    windings: Vec<i64>,
}

/// The work units [`Resolver::coverage`] spends on `edges` at most.
pub(crate) fn work(edges: usize) -> usize {
    let k = edges.saturating_add(2);
    // Pairwise crossings, then per slab (at most 2k + k²/2 of them) a
    // sort of k edges and one pass.
    let slabs = k.saturating_mul(k).saturating_add(2 * k);
    k.saturating_mul(k)
        .saturating_add(slabs.saturating_mul(k.saturating_mul(usize::BITS as usize - k.leading_zeros() as usize + 1)))
}

impl Resolver {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.heights
            .capacity()
            .saturating_mul(size_of::<f64>())
            .saturating_add(self.order.capacity().saturating_mul(size_of::<(f64, u32)>()))
            .saturating_add(self.windings.capacity().saturating_mul(size_of::<i64>()))
    }

    /// The covered area of the cell within heights `span`, quantized,
    /// charging [`work`] to `meter` first.
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
        meter.charge(work(edges.len()))?;
        let area = self.area(edges, rules, span)?;
        Ok(fold(area, FillRule::NonZero))
    }

    fn area(&mut self, edges: &[LocalEdge], rules: &[FillRule], (bottom, top): (f64, f64)) -> Result<f64> {
        let allocation = |_| Error::limit("cell resolver allocation failed");
        self.heights.clear();
        self.heights
            .try_reserve(edges.len().saturating_mul(edges.len()).saturating_add(2 * edges.len() + 2))
            .map_err(allocation)?;
        self.heights.extend([bottom, top]);
        for e in edges {
            for y in [e.y0, e.y1] {
                if y > bottom && y < top {
                    self.heights.push(y);
                }
            }
        }
        for (i, a) in edges.iter().enumerate() {
            for b in &edges[i + 1..] {
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
                        self.heights.push(y);
                    }
                }
            }
        }
        self.heights.sort_unstable_by(f64::total_cmp);
        self.heights.dedup();
        self.windings.clear();
        self.windings.try_reserve(rules.len()).map_err(allocation)?;
        self.windings.resize(rules.len(), 0);
        self.order.clear();
        self.order.try_reserve(edges.len()).map_err(allocation)?;
        let mut area = 0.0;
        for pair in self.heights.windows(2) {
            let (lo, hi) = (pair[0], pair[1]);
            if hi <= lo || lo < bottom || hi > top {
                continue;
            }
            let mid = lo.midpoint(hi);
            self.order.clear();
            for (i, e) in (0_u32..).zip(edges) {
                if e.y0 <= lo && e.y1 >= hi {
                    self.order.push((e.x(mid).clamp(0.0, 1.0), i));
                }
            }
            self.order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
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

fn covered(winding: i64, rule: FillRule) -> bool {
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(x0: f64, y0: f64, x1: f64, y1: f64, winding: i32, group: u8) -> LocalEdge {
        LocalEdge {
            x0,
            y0,
            x1,
            y1,
            winding,
            group,
        }
    }

    fn area(edges: &[LocalEdge], rules: &[FillRule]) -> f64 {
        Resolver::default().area(edges, rules, (0.0, 1.0)).unwrap()
    }

    #[test]
    fn crossing_diagonals_cover_their_union_or_their_parity() {
        // Two triangles sharing the cell's left side, crossing at the middle.
        let edges = [
            edge(0.0, 0.0, 0.0, 1.0, 2, 0),
            edge(0.0, 0.0, 1.0, 1.0, -1, 0),
            edge(1.0, 0.0, 0.0, 1.0, -1, 0),
        ];
        let nonzero = area(&edges, &[FillRule::NonZero]);
        assert!((nonzero - 0.75).abs() < 1e-15, "{nonzero}");
        let even_odd = area(&edges, &[FillRule::EvenOdd]);
        assert!((even_odd - 0.5).abs() < 1e-15, "{even_odd}");
    }

    #[test]
    fn groups_intersect() {
        // The left half and the upper half: a quarter of the cell.
        let edges = [
            edge(0.0, 0.0, 0.0, 1.0, 1, 0),
            edge(0.5, 0.0, 0.5, 1.0, -1, 0),
            edge(0.0, 0.0, 0.0, 0.5, 1, 1),
        ];
        let both = area(&edges, &[FillRule::NonZero, FillRule::NonZero]);
        assert!((both - 0.25).abs() < 1e-15);
        let lower = Resolver::default()
            .area(&edges, &[FillRule::NonZero], (0.25, 1.0))
            .unwrap();
        assert!((lower - 0.375).abs() < 1e-15);
        let mut meter = Meter::new(usize::MAX, "budget");
        let value = Resolver::default()
            .coverage(&edges, &[FillRule::NonZero; 2], (0.0, 1.0), &mut meter)
            .unwrap();
        assert_eq!(value, 64);
        assert!(meter.used() >= work(3));
        let mut tiny = Meter::new(1, "budget");
        assert!(
            Resolver::default()
                .coverage(&edges, &[FillRule::NonZero; 2], (0.0, 1.0), &mut tiny)
                .is_err()
        );
    }
}
