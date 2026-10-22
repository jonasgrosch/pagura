//! The resolver against known areas and against the slab-by-slab
//! integration its sweep replaces, and its charges.

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
    let mut meter = Meter::new(usize::MAX, "budget");
    Resolver::default()
        .area(edges, rules, (0.0, 1.0), &mut meter)
        .unwrap()
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
    let mut meter = Meter::new(usize::MAX, "budget");
    let lower = Resolver::default()
        .area(&edges, &[FillRule::NonZero], (0.25, 1.0), &mut meter)
        .unwrap();
    assert!((lower - 0.375).abs() < 1e-15);
    let mut meter = Meter::new(usize::MAX, "budget");
    let value = Resolver::default()
        .coverage(&edges, &[FillRule::NonZero; 2], (0.0, 1.0), &mut meter)
        .unwrap();
    assert_eq!(value, 64);
    assert!(meter.used() >= 3 * 3);
    let mut tiny = Meter::new(1, "budget");
    assert!(
        Resolver::default()
            .coverage(&edges, &[FillRule::NonZero; 2], (0.0, 1.0), &mut tiny)
            .is_err()
    );
}

/// Coincident edges, as the floor cells of many points in one pixel
/// give, cross nowhere: one slab, and a charge quadratic in the edges
/// rather than the cubic bound of every pair crossing.
#[test]
fn coincident_edges_cost_one_slab() {
    let mut edges = Vec::new();
    for _ in 0..300 {
        edges.push(edge(0.0, 0.0, 0.0, 1.0, 1, 0));
        edges.push(edge(1.0, 0.0, 1.0, 1.0, -1, 0));
    }
    let limits = pagura_core::Limits::default();
    let mut meter = Meter::new(limits.raster_work, "budget");
    let value = Resolver::default()
        .coverage(&edges, &[FillRule::NonZero], (0.0, 1.0), &mut meter)
        .unwrap();
    assert_eq!(value, u8::MAX);
    assert!(meter.used() <= 2 * 600 * 600, "{}", meter.used());
}

/// The integration the sweep replaces: every pair tested for a crossing,
/// and per slab every edge tested and those spanning it sorted.
fn slab_by_slab(edges: &[LocalEdge], rules: &[FillRule], (bottom, top): (f64, f64)) -> f64 {
    let mut heights = vec![bottom, top];
    for e in edges {
        heights.extend([e.y0, e.y1].into_iter().filter(|&y| y > bottom && y < top));
    }
    for (i, a) in edges.iter().enumerate() {
        for b in &edges[i + 1..] {
            let (lo, hi) = (a.y0.max(b.y0), a.y1.min(b.y1));
            if lo >= hi {
                continue;
            }
            let (d0, d1) = (a.x(lo) - b.x(lo), a.x(hi) - b.x(hi));
            if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
                let y = lo + (hi - lo) * (d0 / (d0 - d1));
                if y > lo && y < hi && y > bottom && y < top {
                    heights.push(y);
                }
            }
        }
    }
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    let mut area = 0.0;
    for pair in heights.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        let mid = lo.midpoint(hi);
        let mut order: Vec<(f64, usize)> = (0..edges.len())
            .filter(|&i| edges[i].y0 <= lo && edges[i].y1 >= hi)
            .map(|i| (edges[i].x(mid).clamp(0.0, 1.0), i))
            .collect();
        order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut windings = vec![0_i64; rules.len()];
        let (mut width, mut previous, mut inside) = (0.0, 0.0, false);
        for (x, i) in order {
            if inside {
                width += x - previous;
            }
            windings[usize::from(edges[i].group)] += i64::from(edges[i].winding);
            inside = windings.iter().zip(rules).all(|(&w, &r)| covered(w, r));
            previous = x;
        }
        if inside {
            width += 1.0 - previous;
        }
        area += width * (hi - lo);
    }
    area
}

/// A deterministic generator for test geometry.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u32::try_from(self.0 >> 33).unwrap()
    }

    /// One of `count` values from `lo` on a grid of `1 / steps`.
    fn grid(&mut self, lo: f64, count: u32, steps: u32) -> f64 {
        lo + f64::from(self.next() % count) / f64::from(steps)
    }
}

/// Random edges on a coarse grid, so that ends, crossings and whole edges
/// coincide often; some reach outside the cell's heights.
fn random_edges(rng: &mut Lcg, count: usize, groups: u32) -> Vec<LocalEdge> {
    (0..count)
        .map(|_| {
            let y0 = rng.grid(-0.25, 20, 16);
            let y1 = (y0 + 1.0 / 16.0 + rng.grid(0.0, 20, 16)).min(1.25);
            let winding = [1, -1, 2, -1][rng.next() as usize % 4];
            edge(
                rng.grid(0.0, 9, 8),
                y0,
                rng.grid(0.0, 9, 8),
                y1,
                winding,
                u8::try_from(rng.next() % groups).unwrap(),
            )
        })
        .collect()
}

/// The sweep integrates exactly as slab by slab: the same heights, and per
/// slab the same edges in the same order but among ties, which add no
/// width between them.
#[test]
fn the_sweep_integrates_as_slab_by_slab() {
    let mut rng = Lcg(5_501);
    let mut resolver = Resolver::default();
    for case in 0..2_000_usize {
        let groups = u32::try_from(1 + case % 3).unwrap();
        let mut edges = random_edges(&mut rng, 2 + case % 40, groups);
        // Repeated edges, as the shared sides of a union give.
        let repeats: Vec<LocalEdge> = edges.iter().step_by(3).copied().collect();
        edges.extend(repeats);
        let rules: Vec<FillRule> = (0..groups)
            .map(|g| [FillRule::NonZero, FillRule::EvenOdd][(case + g as usize) % 2])
            .collect();
        let span = [(0.0, 1.0), (0.25, 0.75), (0.0, 0.5)][case % 3];
        let mut meter = Meter::new(usize::MAX, "budget");
        let swept = resolver.area(&edges, &rules, span, &mut meter).unwrap();
        let reference = slab_by_slab(&edges, &rules, span);
        assert_eq!(swept.to_bits(), reference.to_bits(), "case {case}");
    }
}

/// A chain of many short edges: each spans one slab, so the sweep costs
/// about the sort of its edges rather than every edge per slab.
#[test]
fn short_edges_cost_the_slabs_they_span() {
    let count = 4_000_u32;
    let mut edges: Vec<LocalEdge> = (0..count)
        .map(|i| {
            let x = |i: u32| if i.is_multiple_of(2) { 0.25 } else { 0.75 };
            let y = |i: u32| f64::from(i) / f64::from(count);
            edge(x(i), y(i), x(i + 1), y(i + 1), 1, 0)
        })
        .collect();
    edges.push(edge(1.0, 0.0, 1.0, 1.0, -1, 0));
    let mut meter = Meter::new(pagura_core::Limits::default().raster_work, "budget");
    let swept = Resolver::default()
        .area(&edges, &[FillRule::NonZero], (0.0, 1.0), &mut meter)
        .unwrap();
    assert_eq!(
        swept.to_bits(),
        slab_by_slab(&edges, &[FillRule::NonZero], (0.0, 1.0)).to_bits()
    );
    assert!((swept - 0.5).abs() < 1e-12, "{swept}");
    // Mostly the sorts of the edges and of their ends; slab by slab
    // charged every edge per slab, count² units.
    let edges = edges.len();
    assert!(meter.used() < 100 * edges, "{}", meter.used());
}

/// A cell whose crossings need more work than the budget is refused, with
/// no more work done than charged.
#[test]
fn crowded_crossings_are_refused_within_the_budget() {
    let count = 2_000_u32;
    let edges: Vec<LocalEdge> = (0..count)
        .map(|i| {
            let t = f64::from(i) / f64::from(count);
            edge(t, 0.0, 1.0 - t, 1.0, 1, 0)
        })
        .collect();
    let mut meter = Meter::new(1_000_000, "budget");
    let result = Resolver::default().coverage(&edges, &[FillRule::NonZero], (0.0, 1.0), &mut meter);
    assert!(result.is_err());
    assert!(meter.used() <= 1_000_000);
}
