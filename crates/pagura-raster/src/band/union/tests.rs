//! The row resolver against known areas, against an integration of the
//! row slab by slab, and its charges.

use super::*;
use crate::Point;

#[test]
fn ordering_refuses_before_spending_unavailable_work() {
    let original = [(3.0, 0), (2.0, 1), (1.0, 2)];
    for budget in 0..=3 {
        let mut order = original;
        let mut work = Meter::new(budget, "ordering work");
        let result = settle(&mut order, &mut work);
        assert_eq!(result.is_ok(), budget == 3);
        assert_eq!(work.used(), budget);
        let remaining = order
            .iter()
            .enumerate()
            .map(|(i, a)| {
                order[i + 1..]
                    .iter()
                    .filter(|b| ordered(a, b).is_gt())
                    .count()
            })
            .sum::<usize>();
        assert_eq!(remaining, 3 - budget);
    }
    // Sorting already ordered values returns every reserved unit.
    let mut order = [(1.0, 0), (2.0, 1)];
    let mut work = Meter::new(usize::MAX, "ordering work");
    work.charge(7).unwrap();
    settle(&mut order, &mut work).unwrap();
    assert_eq!(work.used(), 7);

    let mut order = vec![(3.0, 0)];
    let mut joining = [(2.0, 1), (1.0, 2)];
    let before = (order.clone(), joining);
    let mut tiny = Meter::new(sort_work(joining.len()) + order.len() - 1, "joining work");
    let result = arrange((&mut order, &mut joining), f64::from, &mut tiny);
    assert!(result.is_err());
    assert_eq!((order, joining), before);
}

#[test]
fn settling_matches_the_total_order_of_places_and_indices() {
    // Sorted prefixes, equal places, signed zeros and NaN, as plain and
    // exact places give them.
    let values = [-1.0, -0.0, 0.0, 0.5, 0.5, 1.0, f64::NAN];
    let mut rng = Lcg(31);
    for _ in 0..2_000 {
        let len = 1 + rng.next() as usize % 12;
        let mut order: Vec<Place> = (0..len)
            .map(|i| {
                (
                    values[rng.next() as usize % values.len()],
                    u32::try_from(i).unwrap(),
                )
            })
            .collect();
        if rng.next().is_multiple_of(2) {
            // Most sub-slabs find their order sorted but for one move.
            order.sort_unstable_by(ordered);
            let k = rng.next() as usize % len;
            order[k].0 = values[rng.next() as usize % values.len()];
        }
        let mut expected = order.clone();
        expected.sort_unstable_by(ordered);
        let mut work = Meter::new(usize::MAX, "ordering work");
        settle(&mut order, &mut work).unwrap();
        assert_eq!(
            order
                .iter()
                .map(|o| (o.0.to_bits(), o.1))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|o| (o.0.to_bits(), o.1))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn close_plain_places_charge_their_exact_run() {
    // Coincident edges are placed exactly after the plain pass; a third
    // edge apart from them keeps its plain place.
    let pieces = pieces(&[
        (line(0.5, 0.0, 0.5, 1.0), 0),
        (line(0.5, 0.0, 0.5, 1.0), 0),
        (line(0.0, 0.0, 1.0, 1.0), 0),
    ]);
    let lanes: Vec<_> = pieces.iter().map(|p| Lane::new(p.line)).collect();
    let mut resolver = Resolver {
        rows: vec![(0.0, 1.0, 0), (0.0, 1.0, 1), (0.0, 1.0, 2)],
        lane_error: lanes.iter().fold(0.0, |e, lane| e.max(lane.error)),
        lanes,
        ..Resolver::default()
    };
    let mut work = Meter::new(usize::MAX, "placement work");
    resolver.place(&pieces, (0.0, 0.5), 0, &mut work).unwrap();
    assert_eq!(work.used(), sort_work(pieces.len()) + 2);
    assert_eq!(resolver.order, [(0.25, 2), (0.5, 0), (0.5, 1)]);
}

#[test]
fn integer_keys_sort_as_the_total_order() {
    let specials = [
        f64::NAN,
        -f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0,
        -0.0,
        f64::MIN_POSITIVE / 4.0,
        -f64::MIN_POSITIVE,
        1.0,
        -1.0,
        0.5,
    ];
    let mut rng = Lcg(97);
    let (mut keys, mut span_keys, mut sorted) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..1_000 {
        let len = rng.next() as usize % 40;
        let mut values: Vec<f64> = (0..len)
            .map(|_| match rng.next() % 3 {
                0 => specials[rng.next() as usize % specials.len()],
                _ => rng.grid(-2.0, 33, 8),
            })
            .collect();
        let mut expected = values.clone();
        expected.sort_unstable_by(f64::total_cmp);
        sort_values(&mut values, &mut keys);
        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&values), bits(&expected));
        let mut spans: Vec<Span> = (0..u32::try_from(len).unwrap())
            .map(|i| (expected[(i as usize * 7) % len.max(1)], f64::from(i), i))
            .collect();
        let mut by_start = spans.clone();
        by_start.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        sort_spans(&mut spans, (&mut span_keys, &mut sorted));
        let fields = |v: &[Span]| {
            v.iter()
                .map(|s| (s.0.to_bits(), s.1.to_bits(), s.2))
                .collect::<Vec<_>>()
        };
        assert_eq!(fields(&spans), fields(&by_start));
    }
}

fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Line {
    Line {
        a: Point::new(x0, y0),
        b: Point::new(x1, y1),
    }
}

/// The pieces of `lines` in row `0..1`, each with its group.
fn pieces(lines: &[(Line, u8)]) -> Vec<Piece> {
    lines
        .iter()
        .filter_map(|&(l, group)| Piece::new(l, 0.0, group))
        .collect()
}

/// The sides of row `0..1` and the work they took.
fn sides(pieces: &[Piece], rules: &[FillRule], work: &mut Meter) -> Result<Vec<Line>> {
    let mut out = Vec::new();
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let meters = Meters {
        work,
        bytes: &mut bytes,
    };
    Resolver::default().row(pieces, rules, 0.0, (&mut out, meters))?;
    Ok(out)
}

/// The area the sides enclose: the left ones run down, the right ones up.
fn enclosed(sides: &[Line]) -> f64 {
    sides
        .iter()
        .map(|l| (l.b.y - l.a.y) * -(l.a.x + l.b.x) / 2.0)
        .sum()
}

fn area(lines: &[(Line, u8)], rules: &[FillRule]) -> f64 {
    let mut work = Meter::new(usize::MAX, "work");
    enclosed(&sides(&pieces(lines), rules, &mut work).unwrap())
}

/// The closed polygon through `points`, in `group`.
fn polygon(points: &[(f64, f64)], group: u8) -> Vec<(Line, u8)> {
    let next = points.iter().cycle().skip(1);
    points
        .iter()
        .zip(next)
        .map(|(&(x0, y0), &(x1, y1))| (line(x0, y0, x1, y1), group))
        .collect()
}

#[test]
fn chunk_boundary_verticals_keep_half_open_ownership_when_reversed() {
    for (x, kept) in [(-0.5, false), (0.0, true), (0.5, true), (1.0, false)] {
        for (a, b) in [(0.0, 1.0), (1.0, 0.0)] {
            let piece = Piece::new(line(x, a, x, b), 0.0, 0).unwrap();
            let actual = clip(piece, piece.ends(), (0.0, 1.0), (true, false));
            assert_eq!(actual.is_some(), kept, "x{x}, direction{a}->{b}");
            if let Some(clipped) = actual {
                assert_eq!((clipped.lo, clipped.hi), (0.0, 1.0));
                assert_eq!(clipped.direction, piece.direction);
            }
        }
    }
}

#[test]
fn crossing_triangles_cover_their_union_or_their_parity() {
    // Left of the rising and of the falling diagonal, crossing at the
    // middle.
    let mut lines = polygon(&[(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)], 0);
    lines.extend(polygon(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], 0));
    let nonzero = area(&lines, &[FillRule::NonZero]);
    assert!((nonzero - 0.75).abs() < 1e-15, "{nonzero}");
    let even_odd = area(&lines, &[FillRule::EvenOdd]);
    assert!((even_odd - 0.5).abs() < 1e-15, "{even_odd}");
}

#[test]
fn groups_intersect() {
    // The left half and the upper half: a quarter of the row.
    let mut lines = polygon(&[(0.0, 0.0), (0.5, 0.0), (0.5, 1.0), (0.0, 1.0)], 0);
    lines.extend(polygon(
        &[(0.0, 0.0), (1.0, 0.0), (1.0, 0.5), (0.0, 0.5)],
        1,
    ));
    let both = area(&lines, &[FillRule::NonZero; 2]);
    assert!((both - 0.25).abs() < 1e-15, "{both}");
    let mut tiny = Meter::new(1, "work");
    assert!(sides(&pieces(&lines), &[FillRule::NonZero; 2], &mut tiny).is_err());
}

/// The sides come out as pieces of the edges, joined across sub-slabs
/// while the same edge stays the same side: a rectangle of winding two
/// over a crossing gives its two sides whole.
#[test]
fn sides_are_joined_across_sub_slabs() {
    let rectangle = polygon(&[(0.0, 0.0), (4.0, 0.0), (4.0, 1.0), (0.0, 1.0)], 0);
    let mut lines = [rectangle.clone(), rectangle].concat();
    lines.extend(polygon(
        &[(1.0, 0.0), (3.0, 1.0), (1.0, 1.0), (3.0, 0.0)],
        0,
    ));
    let mut work = Meter::new(usize::MAX, "work");
    let out = sides(&pieces(&lines), &[FillRule::NonZero], &mut work).unwrap();
    assert_eq!(out.len(), 2, "{out:?}");
    assert!((enclosed(&out) - 4.0).abs() < 1e-15);
}

/// Coincident pieces, as the shared sides of a union or the floor cells of
/// many points in one pixel give, cross nowhere: one sub-slab, and work
/// about their pairs rather than every pair per sub-slab.
#[test]
fn coincident_pieces_cost_one_slab() {
    let mut lines = Vec::new();
    for _ in 0..300 {
        lines.extend(polygon(
            &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
            0,
        ));
    }
    let mut work = Meter::new(pagura_core::Limits::default().raster_work, "work");
    let out = sides(&pieces(&lines), &[FillRule::NonZero], &mut work).unwrap();
    assert_eq!(out.len(), 2);
    assert!((enclosed(&out) - 1.0).abs() < 1e-15);
    assert!(work.used() <= 600 * 600, "{}", work.used());
}

/// A chain of many short pieces one below another: each spans one
/// sub-slab and overlaps only its neighbours in height, so the row costs
/// about the sorts of its pieces, though all share their columns.
#[test]
fn short_pieces_cost_the_slabs_they_span() {
    let count = 4_000_u32;
    let x = |i: u32| if i.is_multiple_of(2) { 0.25 } else { 0.75 };
    let y = |i: u32| f64::from(i) / f64::from(count);
    let mut lines: Vec<(Line, u8)> = (0..count)
        .map(|i| (line(x(i), y(i), x(i + 1), y(i + 1)), 0))
        .collect();
    lines.push((line(1.0, 1.0, 1.0, 0.0), 0));
    let mut work = Meter::new(pagura_core::Limits::default().raster_work, "work");
    let out = sides(&pieces(&lines), &[FillRule::NonZero], &mut work).unwrap();
    assert!((enclosed(&out) - 0.5).abs() < 1e-12, "{}", enclosed(&out));
    // The sorts of the pieces and their heights, and a few units per
    // sub-slab; across every sub-slab, every piece would be count² units.
    let edges = lines.len();
    assert!(work.used() < 200 * edges, "{}", work.used());
}

/// A row whose crossings need more work than the budget is refused, with
/// no more work done than charged.
#[test]
fn crowded_crossings_are_refused_within_the_budget() {
    let count = 2_000_u32;
    let lines: Vec<(Line, u8)> = (0..count)
        .map(|i| {
            let t = f64::from(i) / f64::from(count);
            (line(t, 0.0, 1.0 - t, 1.0), 0)
        })
        .collect();
    let mut work = Meter::new(1_000_000, "work");
    assert!(sides(&pieces(&lines), &[FillRule::NonZero], &mut work).is_err());
    assert!(work.used() <= 1_000_000);
}

/// The scratch is charged to the byte meter, and a small one refuses it.
#[test]
fn scratch_is_charged_to_the_byte_meter() {
    let lines = polygon(&[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0), (1.0, 0.5)], 0);
    let pieces = pieces(&lines);
    let mut work = Meter::new(usize::MAX, "work");
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let mut out = Vec::new();
    let meters = Meters {
        work: &mut work,
        bytes: &mut bytes,
    };
    let mut resolver = Resolver::default();
    resolver
        .row(&pieces, &[FillRule::NonZero], 0.0, (&mut out, meters))
        .unwrap();
    let used = bytes.used();
    assert!(used >= pieces.len() * size_of::<Span>() * 2, "{used}");
    let mut small = Meter::new(used / 2, "bytes");
    let meters = Meters {
        work: &mut work,
        bytes: &mut small,
    };
    let result = Resolver::default().row(
        &pieces,
        &[FillRule::NonZero],
        0.0,
        (&mut Vec::new(), meters),
    );
    assert!(result.is_err());
    assert!(small.used() <= used / 2);
}

/// The covered area of row `0..1`, integrated slab by slab: every pair
/// tested for a crossing, and per slab every piece placed at its middle.
fn slab_by_slab(pieces: &[Piece], rules: &[FillRule]) -> f64 {
    let mut heights = vec![0.0, 1.0];
    for p in pieces {
        heights.extend([p.lo, p.hi]);
    }
    for (i, p) in pieces.iter().enumerate() {
        for q in &pieces[i + 1..] {
            let (lo, hi) = (p.lo.max(q.lo), p.hi.min(q.hi));
            if lo >= hi {
                continue;
            }
            let (d0, d1) = (p.line.x(lo) - q.line.x(lo), p.line.x(hi) - q.line.x(hi));
            if (d0 < 0.0 && d1 > 0.0) || (d0 > 0.0 && d1 < 0.0) {
                heights.push(lo + (hi - lo) * (d0 / (d0 - d1)));
            }
        }
    }
    heights.sort_unstable_by(f64::total_cmp);
    heights.dedup();
    let mut area = 0.0;
    for pair in heights.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        let mid = lo.midpoint(hi);
        let mut order: Vec<(f64, &Piece)> = pieces
            .iter()
            .filter(|p| p.lo <= lo && p.hi >= hi)
            .map(|p| (p.line.x(mid), p))
            .collect();
        order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut windings = vec![0_i64; rules.len()];
        let (mut width, mut previous, mut inside) = (0.0, 0.0, false);
        for (x, p) in order {
            if inside {
                width += x - previous;
            }
            windings[usize::from(p.group)] += i64::from(p.direction);
            inside = windings.iter().zip(rules).all(|(&w, &r)| covered(w, r));
            previous = x;
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

/// Closed random polygons on a coarse grid, so that ends, crossings and
/// whole edges coincide often; some reach above or below the row.
fn random_lines(rng: &mut Lcg, polygons: usize, groups: u32) -> Vec<(Line, u8)> {
    let mut lines = Vec::new();
    for _ in 0..polygons {
        let group = u8::try_from(rng.next() % groups).unwrap();
        let corners = 3 + rng.next() as usize % 4;
        let points: Vec<(f64, f64)> = (0..corners)
            .map(|_| (rng.grid(0.0, 17, 8), rng.grid(-0.25, 12, 8)))
            .collect();
        lines.extend(polygon(&points, group));
        // Repeated polygons, as the shared sides of a union give.
        if rng.next().is_multiple_of(4) {
            lines.extend(polygon(&points, group));
        }
    }
    lines
}

/// The sweep encloses the area an integration slab by slab finds.
#[test]
fn the_sweep_encloses_the_area_slab_by_slab() {
    let mut rng = Lcg(5_501);
    for case in 0..2_000_usize {
        let groups = u32::try_from(1 + case % 3).unwrap();
        let lines = random_lines(&mut rng, 1 + case % 6, groups);
        let rules: Vec<FillRule> = (0..groups)
            .map(|g| [FillRule::NonZero, FillRule::EvenOdd][(case + g as usize) % 2])
            .collect();
        let pieces = pieces(&lines);
        let mut work = Meter::new(usize::MAX, "work");
        let swept = enclosed(&sides(&pieces, &rules, &mut work).unwrap());
        let reference = slab_by_slab(&pieces, &rules);
        assert!(
            (swept - reference).abs() < 1e-12,
            "case {case}: {swept} {reference}"
        );
    }
}

/// Rows of more than a chunk of pieces, over many columns, with long
/// pieces across the cuts: the chunks enclose the area of the whole row.
#[test]
fn chunks_enclose_the_area_of_the_row() {
    let mut rng = Lcg(77);
    let mut cut = 0;
    for case in 0..300_usize {
        let groups = u32::try_from(1 + case % 2).unwrap();
        let mut lines = Vec::new();
        for _ in 0..20 + case % 40 {
            let group = u8::try_from(rng.next() % groups).unwrap();
            let x = rng.grid(0.0, 96, 8);
            let corners = 3 + rng.next() as usize % 3;
            let points: Vec<(f64, f64)> = (0..corners)
                .map(|_| (x + rng.grid(-0.5, 17, 8), rng.grid(-0.25, 12, 8)))
                .collect();
            lines.extend(polygon(&points, group));
        }
        // A long band across every cut.
        lines.extend(polygon(
            &[(-1.0, 0.3), (13.0, 0.5), (13.0, 0.7), (-1.0, 0.6)],
            0,
        ));
        let rules: Vec<FillRule> = (0..groups)
            .map(|g| [FillRule::NonZero, FillRule::EvenOdd][(case + g as usize) % 2])
            .collect();
        let pieces = pieces(&lines);
        let mut work = Meter::new(usize::MAX, "work");
        let mut bytes = Meter::new(usize::MAX, "bytes");
        let mut out = Vec::new();
        let mut resolver = Resolver::default();
        let meters = Meters {
            work: &mut work,
            bytes: &mut bytes,
        };
        resolver
            .row(&pieces, &rules, 0.0, (&mut out, meters))
            .unwrap();
        cut += usize::from(!resolver.cuts.is_empty() && pieces.len() > CHUNK);
        let swept = enclosed(&out);
        let reference = slab_by_slab(&pieces, &rules);
        assert!(
            (swept - reference).abs() < 1e-11,
            "case {case}: {swept} {reference}"
        );
    }
    assert!(cut > 200, "{cut}");
}

/// Hundreds of small overlapping shapes along a row, as a fine pattern at
/// a low resolution gives: each chunk sweeps its own ends and crossings,
/// so the row costs a fraction of one sweep, which takes its every height
/// times every piece across it.
#[test]
fn dense_rows_cost_their_chunks() {
    let mut rng = Lcg(3);
    let mut lines = Vec::new();
    for i in 0..600_u32 {
        let x = f64::from(i) / 10.0;
        let points: Vec<(f64, f64)> = (0..4)
            .map(|_| (x + rng.grid(0.0, 17, 8), rng.grid(-0.25, 12, 8)))
            .collect();
        lines.extend(polygon(&points, 0));
    }
    let pieces = pieces(&lines);
    let rules = [FillRule::NonZero];
    let mut chunked = Meter::new(pagura_core::Limits::default().raster_work, "work");
    let out = sides(&pieces, &rules, &mut chunked).unwrap();
    let mut whole = Meter::new(usize::MAX, "work");
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let mut once = Vec::new();
    let mut resolver = Resolver {
        carry: vec![0.0],
        carried: vec![0],
        ..Resolver::default()
    };
    resolver
        .sweep(
            &pieces,
            (&rules, 0.0),
            (&mut once, &mut whole, &mut bytes),
            false,
        )
        .unwrap();
    assert!((enclosed(&out) - enclosed(&once)).abs() < 1e-9);
    assert!(
        chunked.used() * 4 < whole.used(),
        "{} chunked, {} at once",
        chunked.used(),
        whole.used()
    );
}

/// The quadrilaterals a pen of radius `r` sweeps along the closed polyline
/// of `count` segments around a circle of radius `around` at `centre`, as a
/// stroker emits them where its segments are shorter than its pen.
fn fan(centre: (f64, f64), around: f64, (count, r): (u32, f64), turn: f64) -> Vec<(Line, u8)> {
    let at = |k: u32| {
        let t = turn + std::f64::consts::TAU * f64::from(k) / f64::from(count);
        (centre.0 + around * t.cos(), centre.1 + around * t.sin())
    };
    let mut lines = Vec::new();
    for k in 0..count {
        let (p, q) = (at(k), at(k + 1));
        let length = (q.0 - p.0).hypot(q.1 - p.1);
        let normal = ((p.1 - q.1) / length * r, (q.0 - p.0) / length * r);
        lines.extend(polygon(
            &[
                (p.0 + normal.0, p.1 + normal.1),
                (p.0 - normal.0, p.1 - normal.1),
                (q.0 - normal.0, q.1 - normal.1),
                (q.0 + normal.0, q.1 + normal.1),
            ],
            0,
        ));
    }
    lines
}

/// The sides of `pieces` in row `0..1` with the swaps inside the covered
/// set, or with every sub-slab placed and joined, the work charged, and how
/// many sub-slabs the swaps took.
fn swept(pieces: &[Piece], rules: &[FillRule], full: bool) -> (Vec<Line>, usize, usize) {
    let mut out = Vec::new();
    let mut work = Meter::new(usize::MAX, "work");
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let mut resolver = Resolver {
        full,
        ..Resolver::default()
    };
    let meters = Meters {
        work: &mut work,
        bytes: &mut bytes,
    };
    resolver
        .row(pieces, rules, 0.0, (&mut out, meters))
        .unwrap();
    (out, work.used(), resolver.swapped)
}

#[test]
fn doubtful_pairs_expire_once_and_large_live_sets_decline() {
    let mut resolver = Resolver {
        heights: vec![0.0, 0.25, 1.0],
        lowest: f64::INFINITY,
        ..Resolver::default()
    };
    for end in (2..1026).rev() {
        resolver.reach(end, (0, 1));
    }
    // A thousand live pairs need no repeated scan or additional work.
    let mut work = Meter::new(0, "doubtful pairs");
    for _ in 0..10 {
        assert_eq!(
            resolver
                .take_swaps(&[], FillRule::NonZero, 1, (0, 0, 0), &mut work)
                .unwrap(),
            None
        );
        assert_eq!(resolver.reaching.len(), 1024);
    }
    for height in 2..1027 {
        resolver.expire(height);
        assert_eq!(resolver.reaching.len(), 1026 - height);
        assert!(resolver.reaching.iter().all(|p| p.0 >= height));
        for at in 1..resolver.reaching.len() {
            assert!(resolver.reaching[(at - 1) / 2].0 <= resolver.reaching[at].0);
        }
    }
}

/// Fans of quadrilaterals crossing many times over inside their union, alone,
/// twice over, with collinear edges from one vertex, beside and over random
/// polygons and across chunks, under either rule: the swaps leave every
/// side as the full sweep emits it, bit for bit and in the same order, and
/// charge no more.
#[test]
fn swaps_inside_the_union_keep_every_side() {
    let mut rng = Lcg(2_026);
    let mut swapped = 0;
    for case in 0..400_usize {
        let mut lines = Vec::new();
        for _ in 0..=case % 4 {
            let centre = (rng.grid(0.0, 64, 8), rng.grid(-0.5, 17, 8));
            let count = 8 + rng.next() % 40;
            let r = rng.grid(0.25, 12, 8);
            let around = r * f64::from(1 + rng.next() % 8) / 64.0;
            lines.extend(fan(centre, around, (count, r), rng.grid(0.0, 64, 8)));
        }
        // Every edge twice, as strokes drawn twice or shared sides give.
        if case % 7 == 1 {
            lines.extend_from_within(..);
        }
        // Collinear edges from one vertex through the fans, whose places
        // differ only by rounding.
        if case % 5 == 2 {
            for _ in 0..24 {
                let (x, y) = (rng.grid(0.0, 64, 8) / 3.0, rng.grid(-0.5, 17, 8) / 3.0);
                let (dx, dy) = (rng.grid(-1.0, 17, 8) / 3.0, rng.grid(0.125, 8, 8) / 7.0);
                let t = f64::from(1 + rng.next() % 7) / 9.0;
                lines.extend(polygon(
                    &[(x, y), (x + dx, y + dy), (x + t * dx, y + t * dy)],
                    0,
                ));
            }
        }
        if case % 3 == 0 {
            lines.extend(random_lines(&mut rng, 1 + case % 8, 1));
        }
        let rule = [FillRule::NonZero, FillRule::EvenOdd][usize::from(case % 5 == 4)];
        let pieces = pieces(&lines);
        let ((fast, fast_work, taken), (full, full_work, _)) = (
            swept(&pieces, &[rule], false),
            swept(&pieces, &[rule], true),
        );
        let bits = |sides: &[Line]| -> Vec<[u64; 4]> {
            sides
                .iter()
                .map(|l| [l.a.x, l.a.y, l.b.x, l.b.y].map(f64::to_bits))
                .collect()
        };
        assert_eq!(bits(&fast), bits(&full), "case {case}");
        assert!(
            fast_work <= full_work,
            "case {case}: {fast_work} {full_work}"
        );
        swapped += taken;
    }
    assert!(swapped > 100_000, "{swapped}");
}

/// The sides of row `top..top + 1` under the nonzero rule, with the column
/// ranges `proven` passed over, and the work they took.
fn proven_sides(pieces: &[Piece], top: f64, proven: &[(f64, f64)]) -> (Vec<Line>, usize) {
    let mut out = Vec::new();
    let mut work = Meter::new(usize::MAX, "work");
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let meters = Meters {
        work: &mut work,
        bytes: &mut bytes,
    };
    let runs: Vec<(f64, f64, Run)> = proven
        .iter()
        .map(|&(left, right)| (left, right, Run::Proven))
        .collect();
    Resolver::default()
        .row_proven(pieces, &[FillRule::NonZero], top, &runs, (&mut out, meters))
        .unwrap();
    (out, work.used())
}

/// The covered area of cell `[column, column + 1]` of row `top..top + 1`
/// that `sides` bound: each side covers what lies right of it within the
/// cell, a left one running down, a right one up.
fn cell_area(sides: &[Line], column: f64) -> f64 {
    let mut area = 0.0;
    for s in sides {
        let (sign, upper, lower) = if s.a.y < s.b.y {
            (1.0, s.a, s.b)
        } else {
            (-1.0, s.b, s.a)
        };
        if upper.y == lower.y {
            continue;
        }
        // Width of the cell right of the side, at height `y`.
        let x = |y: f64| upper.x + (lower.x - upper.x) * (y - upper.y) / (lower.y - upper.y);
        let width = |y: f64| (column + 1.0 - x(y)).clamp(0.0, 1.0);
        let mut heights = vec![upper.y, lower.y];
        for edge in [column, column + 1.0] {
            if (upper.x - edge) * (lower.x - edge) < 0.0 {
                heights
                    .push(upper.y + (edge - upper.x) * (lower.y - upper.y) / (lower.x - upper.x));
            }
        }
        heights.sort_by(f64::total_cmp);
        for h in heights.windows(2) {
            area += sign * (h[1] - h[0]) * (width(h[0]) + width(h[1])) / 2.0;
        }
    }
    area
}

/// Asserts that `sides` and `full` cover each cell of row `top..top + 1`
/// alike, but for rounding.
fn same_cells(sides: &[Line], full: &[Line], top: f64, case: &str) {
    let (lo, hi) = full
        .iter()
        .chain(sides)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), l| {
            (lo.min(l.a.x.min(l.b.x)), hi.max(l.a.x.max(l.b.x)))
        });
    let mut column = lo.floor() - 1.0;
    while column <= hi + 1.0 {
        let (a, b) = (cell_area(sides, column), cell_area(full, column));
        assert!(
            (a - b).abs() <= 1e-9,
            "{case} top {top} column {column}: {a} {b}"
        );
        column += 1.0;
    }
}

/// Whether the cells from column `first` to `last` of row `top..top + 1`
/// are covered throughout, as the full sweep's `sides` of `pieces` show:
/// no side among them, and covered between every two heights a side ends
/// at.
fn covered_cells(pieces: &[Piece], sides: &[Line], top: f64, (first, last): (f64, f64)) -> bool {
    let (left, right) = (first, last + 1.0);
    if sides
        .iter()
        .any(|s| s.a.y != s.b.y && s.a.x.max(s.b.x) > left && s.a.x.min(s.b.x) < right)
    {
        return false;
    }
    let mut heights: Vec<f64> = [top, top + 1.0]
        .into_iter()
        .chain(sides.iter().flat_map(|s| [s.a.y, s.b.y]))
        .chain(pieces.iter().flat_map(|p| [p.lo, p.hi]))
        .collect();
    heights.sort_by(f64::total_cmp);
    heights.dedup();
    let x = f64::midpoint(left, right);
    heights.windows(2).all(|h| {
        let y = f64::midpoint(h[0], h[1]);
        let winding: i64 = pieces
            .iter()
            .filter(|p| p.lo <= y && y < p.hi && p.line.x(y) < x)
            .map(|p| i64::from(p.direction))
            .sum();
        winding != 0
    })
}

/// The ranges to pass over in row `top..top + 1` of `pieces`: the runs of
/// cells the full sweep leaves covered.
fn oracle_ranges(pieces: &[Piece], full: &[Line], top: f64) -> Vec<(f64, f64)> {
    let (lo, hi) = pieces
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            let (a, b) = p.ends();
            (lo.min(a.min(b)), hi.max(a.max(b)))
        });
    let mut ranges = Vec::new();
    let mut run: Option<(f64, f64)> = None;
    let mut c = lo.floor() - 1.0;
    while c <= hi.ceil() + 1.0 {
        if covered_cells(pieces, full, top, (c, c)) {
            run = Some(run.map_or((c, c), |(first, _)| (first, c)));
        } else if let Some((first, last)) = run.take() {
            ranges.push((first, last + 1.0));
        }
        c += 1.0;
    }
    if let Some((first, last)) = run {
        ranges.push((first, last + 1.0));
    }
    ranges
}

/// Fans, doubled fans and random polygons with the ranges a full sweep
/// leaves covered passed over: the sides are the full sweep's, bit for bit,
/// and the passes cost less than the sweeps they replace.
#[test]
fn proven_chunks_keep_every_side() {
    let mut rng = Lcg(2_030);
    let (mut passed, mut fast_work, mut full_work) = (0, 0, 0);
    for case in 0..300_usize {
        let mut lines = Vec::new();
        for _ in 0..=case % 3 {
            let centre = (rng.grid(0.0, 64, 8), rng.grid(-0.5, 17, 8));
            let count = 8 + rng.next() % 40;
            let r = rng.grid(4.0, 96, 4);
            let around = r * f64::from(1 + rng.next() % 8) / 64.0;
            lines.extend(fan(centre, around, (count, r), rng.grid(0.0, 64, 8)));
        }
        if case % 7 == 1 {
            lines.extend_from_within(..);
        }
        if case % 3 == 0 {
            lines.extend(random_lines(&mut rng, 1 + case % 8, 1));
        }
        let pieces = pieces(&lines);
        let (full, work) = proven_sides(&pieces, 0.0, &[]);
        let mut ranges = oracle_ranges(&pieces, &full, 0.0);
        // Some rows pass over only some of their ranges.
        if case % 4 == 3 {
            ranges.retain(|_| rng.next().is_multiple_of(2));
        }
        let (sides, used) = proven_sides(&pieces, 0.0, &ranges);
        same_cells(&sides, &full, 0.0, &format!("case {case} {ranges:?}"));
        passed += ranges.len();
        if !ranges.is_empty() {
            fast_work += used;
            full_work += work;
        }
    }
    assert!(passed > 100, "{passed}");
    assert!(fast_work < full_work, "{fast_work} {full_work}");
}

/// Case `case` of strokes of short segments around a wobbly circle, whole
/// or in part: its path, stroke and conformal map.
fn fan_stroke(
    rng: &mut Lcg,
    case: usize,
) -> (crate::Path, crate::stroke::Stroke, crate::Transform) {
    use crate::stroke::{LineCap, LineJoin, Stroke};
    use crate::{PathBuilder, Transform};
    let closed = case.is_multiple_of(2);
    let count = 6 + rng.next() % 50;
    let around = rng.grid(0.25, 64, 4);
    let span = if closed { 1.0 } else { rng.grid(0.25, 8, 4) };
    let centre = (rng.grid(4.0, 64, 4), rng.grid(4.0, 64, 4));
    let wobble = rng.grid(0.0, 4, 8);
    let mut builder = PathBuilder::new();
    for k in 0..count {
        let t = std::f64::consts::TAU * span * f64::from(k) / f64::from(count);
        let r = around * (1.0 + wobble * (3.0 * t).sin());
        let p = Point::new(centre.0 + r * t.cos(), centre.1 + r * t.sin());
        if k == 0 {
            builder.move_to(p);
        } else {
            builder.line_to(p).unwrap();
        }
        // Reversals, which a miter or bevel does not join.
        if case % 9 == 4 && k == count / 2 {
            builder.line_to(Point::new(p.x + 3.0, p.y)).unwrap();
            builder.line_to(p).unwrap();
        }
    }
    if closed {
        builder.close();
    }
    let stroke = Stroke {
        width: [2.0, 3.0, 6.0, 12.0, 24.0, 42.0][case % 6],
        cap: [LineCap::Butt, LineCap::Round, LineCap::ProjectingSquare][case / 6 % 3],
        join: [LineJoin::Round, LineJoin::Miter, LineJoin::Bevel][case / 18 % 3],
        miter_limit: [1.0, 1.5, 4.0, 10.0][rng.next() as usize % 4],
        dash_array: if case % 5 == 3 {
            vec![5.0, 2.0]
        } else {
            Vec::new()
        },
        dash_phase: 0.0,
    };
    let angle = rng.grid(0.0, 64, 10);
    let scale = rng.grid(0.5, 4, 2);
    let (cos, sin) = (scale * angle.cos(), scale * angle.sin());
    let transform = if case % 4 == 1 {
        Transform::new(cos, sin, sin, -cos, 100.0, 100.0)
    } else {
        Transform::new(cos, sin, -sin, cos, 0.0, 0.0)
    };
    (builder.finish(), stroke, transform)
}

/// Strokes of short segments under round, mitred and bevelled joins and
/// every cap, open and closed, dashed or not, under conformal maps: every
/// cell their spine proves the full sweep leaves covered, and passing over
/// those cells leaves every side as it was.
#[test]
fn spines_prove_only_covered_cells_and_keep_every_side() {
    use crate::stroke::{Tiling, polygon_pieces};
    let mut rng = Lcg(2_031);
    let limits = pagura_core::Limits::default();
    let (mut strokes, mut proven) = (0, 0.0);
    for case in 0..240_usize {
        let (path, stroke, transform) = fan_stroke(&mut rng, case);
        let mut bytes = Meter::new(usize::MAX, "bytes");
        let (polygon, tiling, spine) = polygon_pieces(
            &path,
            transform,
            &stroke,
            &limits,
            (Tiling::Unproven, crate::flatten::TOLERANCE),
            &mut bytes,
        )
        .unwrap();
        let Some(spine) = spine else {
            continue;
        };
        assert_eq!(tiling, Tiling::Unproven);
        strokes += 1;
        let lines = crate::band::edges::lines(&polygon, &mut bytes).unwrap();
        let (lo, hi) = lines
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), l| {
                (lo.min(l.a.y.min(l.b.y)), hi.max(l.a.y.max(l.b.y)))
            });
        let mut cover = spine.cover(None);
        let mut work = Meter::new(usize::MAX, "work");
        let mut top = lo.floor();
        while top < hi {
            let pieces: Vec<Piece> = lines
                .iter()
                .filter_map(|&l| Piece::new(l, top, 0))
                .collect();
            let mut ranges = Vec::new();
            cover
                .row(top, usize::MAX, &mut ranges, (&mut work, &mut bytes))
                .unwrap();
            if !ranges.is_empty() {
                let (full, _) = proven_sides(&pieces, top, &[]);
                for &(left, right) in &ranges {
                    assert!(
                        covered_cells(&pieces, &full, top, (left, right - 1.0)),
                        "case {case} row {top}: {left}..{right} {stroke:?}"
                    );
                    proven += right - left;
                }
                let (sides, _) = proven_sides(&pieces, top, &ranges);
                same_cells(&sides, &full, top, &format!("case {case} row {top}"));
            }
            top += 1.0;
        }
    }
    assert!(strokes > 100, "{strokes}");
    assert!(proven > 10_000.0, "{proven}");
}
