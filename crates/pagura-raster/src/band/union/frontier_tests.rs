//! Differential shields for complete-frontier reuse. The oracle keeps the
//! original event layout and full place/slab/join path, including its errors.

use super::{CHUNK, Line, Meters, Piece, Resolver};
use crate::{FillRule, Point};
use pagura_core::{ErrorKind, Meter};

fn line(a: (f64, f64), b: (f64, f64)) -> Line {
    Line {
        a: Point::new(a.0, a.1),
        b: Point::new(b.0, b.1),
    }
}

fn pieces(lines: &[(Line, u8)]) -> Vec<Piece> {
    lines
        .iter()
        .filter_map(|&(edge, group)| Piece::new(edge, 0.0, group))
        .collect()
}

fn polygon(points: &[(f64, f64)], group: u8) -> Vec<(Line, u8)> {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(&a, &b)| (line(a, b), group))
        .collect()
}

/// A changing left side and one uninterrupted right side. Each interior
/// height has exactly one original edge ending and another starting.
fn chain(left: f64, right: f64, group: u8) -> Vec<(Line, u8)> {
    let points = [
        (left, 0.0),
        (left + 0.5, 0.25),
        (left, 0.5),
        (left + 0.5, 0.75),
        (left, 1.0),
    ];
    let mut result: Vec<_> = points
        .windows(2)
        .map(|pair| (line(pair[0], pair[1]), group))
        .collect();
    result.push((line((right, 1.0), (right, 0.0)), group));
    result
}

fn bits(lines: &[Line]) -> Vec<[u64; 4]> {
    lines
        .iter()
        .map(|edge| {
            [
                edge.a.x.to_bits(),
                edge.a.y.to_bits(),
                edge.b.x.to_bits(),
                edge.b.y.to_bits(),
            ]
        })
        .collect()
}

fn area(lines: &[Line]) -> f64 {
    lines
        .iter()
        .map(|edge| (edge.b.y - edge.a.y) * -(edge.a.x + edge.b.x) * 0.5)
        .sum()
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    error: Option<(ErrorKind, &'static str)>,
    sides: Vec<[u64; 4]>,
    work: usize,
    bytes: usize,
    capacities: [usize; 20],
    completed_heights: Option<Vec<u64>>,
}

fn observe(
    resolver: &mut Resolver,
    input: &[Piece],
    rules: &[FillRule],
    out: &mut Vec<Line>,
    (work_limit, byte_limit): (usize, usize),
) -> Observation {
    out.clear();
    let mut work = Meter::new(work_limit, "frontier test work");
    let mut bytes = Meter::new(byte_limit, "frontier test bytes");
    let error = resolver
        .row(
            input,
            rules,
            0.0,
            (
                &mut *out,
                Meters {
                    work: &mut work,
                    bytes: &mut bytes,
                },
            ),
        )
        .err()
        .map(|error| (error.kind, error.message));
    Observation {
        completed_heights: error.is_none().then(|| {
            resolver
                .heights
                .iter()
                .map(|height| height.to_bits())
                .collect()
        }),
        error,
        sides: bits(out),
        work: work.used(),
        bytes: bytes.used(),
        capacities: [
            resolver.heights.capacity(),
            resolver.columns.capacity(),
            resolver.rows.capacity(),
            resolver.order.capacity(),
            resolver.joining.capacity(),
            resolver.windings.capacity(),
            resolver.open.capacity(),
            resolver.sides.capacity(),
            resolver.next.capacity(),
            resolver.slots.capacity(),
            resolver.carry.capacity(),
            resolver.carried.capacity(),
            resolver.after.capacity(),
            resolver.carried_after.capacity(),
            resolver.spans.capacity(),
            resolver.lasts.capacity(),
            resolver.cuts.capacity(),
            resolver.live.capacity(),
            resolver.local.capacity(),
            out.capacity(),
        ],
    }
}

fn differential(input: &[Piece], rules: &[FillRule]) -> (Vec<Line>, Resolver) {
    let mut actual = Resolver::default();
    let mut original = Resolver {
        original_frontier: true,
        ..Resolver::default()
    };
    let (mut out, mut reference) = (Vec::new(), Vec::new());
    let observed = observe(
        &mut actual,
        input,
        rules,
        &mut out,
        (usize::MAX, usize::MAX),
    );
    let expected = observe(
        &mut original,
        input,
        rules,
        &mut reference,
        (usize::MAX, usize::MAX),
    );
    assert!(expected.error.is_none(), "{expected:?}");
    assert_eq!(observed, expected);
    (out, actual)
}

#[test]
fn selected_handoffs_keep_each_original_kink_and_side_order() {
    let lines = chain(0.0, 2.0, 0);
    let input = pieces(&lines);
    let (out, resolver) = differential(&input, &[FillRule::NonZero]);
    let expected: Vec<_> = lines.iter().map(|&(edge, _)| edge).collect();
    assert_eq!(bits(&out), bits(&expected));
    assert_eq!(area(&out), 1.75);
    assert!(resolver.frontier_successes >= 3);
    assert!(resolver.frontier_replacements >= 3);
}

#[test]
fn unselected_handoffs_keep_coincident_multiplicity_and_open_sides() {
    let rectangle = polygon(&[(0.0, 0.0), (4.0, 0.0), (4.0, 1.0), (0.0, 1.0)], 0);
    let mut lines = [rectangle.clone(), rectangle].concat();
    lines.extend(chain(1.0, 3.0, 0));
    let (out, resolver) = differential(&pieces(&lines), &[FillRule::NonZero]);
    assert_eq!(out.len(), 2);
    assert_eq!(area(&out), 4.0);
    assert!(out.iter().all(|edge| edge.a.x == edge.b.x));
    assert!(resolver.frontier_successes >= 3);
}

#[test]
fn handoffs_preserve_group_clips_opposite_directions_and_both_rules() {
    let mark = chain(-1.0, 4.0, 0);
    let mut lines = [mark.clone(), mark.clone(), mark].concat();
    lines.extend(polygon(
        &[(0.0, 0.125), (2.0, 0.125), (2.0, 0.875), (0.0, 0.875)],
        1,
    ));
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let (out, _) = differential(&pieces(&lines), &[rule, FillRule::NonZero]);
        assert_eq!(area(&out), 1.5);
    }
    lines.extend(polygon(
        &[(8.0, 0.0), (8.0, 1.0), (9.0, 1.0), (9.0, 0.0)],
        0,
    ));
    let _ = differential(&pieces(&lines), &[FillRule::NonZero, FillRule::EvenOdd]);
}

#[test]
fn uncertified_first_and_last_relations_fall_back_at_a_future_apex() {
    for offset in [-6.0, 4.0] {
        let mut lines = chain(offset, offset + 2.0, 0);
        // Both sides touch at their future endpoint. Their midpoint order
        // is strict, but a whole-overlap certificate must not admit it.
        lines.extend(polygon(&[(0.0, 0.0), (2.0, 0.0), (1.0, 1.0)], 0));
        let (out, resolver) = differential(&pieces(&lines), &[FillRule::NonZero]);
        assert_eq!(area(&out), 2.75);
        assert_eq!(resolver.frontier_successes, 0);
    }
}

#[test]
fn rounded_ulp_ties_and_signed_zero_do_not_inherit_stale_order() {
    let mut lines = chain(4.0, 6.0, 0);
    lines.extend([
        (line((1.0 + f64::EPSILON, 0.0), (1.0, 1.0)), 0),
        (line((1.0, 1.0), (1.0, 0.0)), 0),
        (line((-0.0, 0.0), (-0.0, 1.0)), 0),
        (line((0.0, 1.0), (0.0, 0.0)), 0),
    ]);
    let mut input = pieces(&lines);
    for piece in &mut input {
        if piece.lo == 0.0 {
            piece.lo = -0.0;
        }
    }
    let (_, resolver) = differential(&input, &[FillRule::NonZero]);
    assert_eq!(resolver.heights[0].to_bits(), (-0.0_f64).to_bits());
    assert_eq!(resolver.frontier_successes, 0);
}

fn chunked_input() -> Vec<Piece> {
    let mut lines = Vec::new();
    for n in 0..48_u32 {
        let x = 3.0 * f64::from(n);
        for k in 0..4_u32 {
            let y = f64::from(k) * 0.25;
            lines.push((line((x, y), (x, y + 0.25)), 0));
        }
        lines.push((line((x + 0.5, 1.0), (x + 0.5, 0.0)), 0));
    }
    lines.extend(polygon(
        &[(-1.0, 0.125), (145.0, 0.125), (145.0, 0.875), (-1.0, 0.875)],
        1,
    ));
    pieces(&lines)
}

#[test]
fn handoffs_reset_at_chunks_and_carried_winding_changes() {
    let input = chunked_input();
    assert!(input.len() > CHUNK);
    let (out, resolver) = differential(&input, &[FillRule::NonZero; 2]);
    assert!(!resolver.cuts.is_empty());
    assert_eq!(area(&out), 18.0);
    assert!(resolver.frontier_successes > 0);
}

#[test]
fn empty_singleton_and_unsupported_domains_keep_the_old_route() {
    assert!(differential(&[], &[FillRule::NonZero]).0.is_empty());
    let input = pieces(&chain(0.0, 2.0, 0)[..4]);
    let (out, resolver) = differential(&input, &[FillRule::NonZero]);
    assert_eq!(out.len(), 4);
    assert!(resolver.frontier_successes >= 3);
    for x in [1.0e13, -1.0e13] {
        let _ = differential(&pieces(&chain(x, x + 2.0, 0)), &[FillRule::NonZero]);
    }
}

#[test]
fn large_piece_ids_use_the_full_path_without_new_refusal() {
    let count = 65_535_u32;
    let input: Vec<_> = (0..count)
        .map(|n| {
            let a = f64::from(n) / f64::from(count);
            let b = f64::from(n + 1) / f64::from(count);
            Piece::new(line((0.25, a), (0.25, b)), 0.0, 0).unwrap()
        })
        .collect();
    let (out, resolver) = differential(&input, &[FillRule::NonZero]);
    assert_eq!(out.len(), input.len());
    assert_eq!(resolver.frontier_successes, 0);
}

#[test]
fn exact_work_and_byte_boundaries_preserve_partial_output_and_capacity() {
    let input = pieces(&chain(0.0, 2.0, 0));
    let rules = [FillRule::NonZero];
    let full = observe(
        &mut Resolver::default(),
        &input,
        &rules,
        &mut Vec::new(),
        (usize::MAX, usize::MAX),
    );
    assert!(full.error.is_none() && full.work > 1 && full.bytes > 1);
    for work_limit in [0, 1, full.work - 1, full.work, full.work + 1] {
        for byte_limit in [0, 1, full.bytes - 1, full.bytes, full.bytes + 1] {
            let mut actual = Resolver::default();
            let mut original = Resolver {
                original_frontier: true,
                ..Resolver::default()
            };
            let limits = (work_limit, byte_limit);
            let a = observe(&mut actual, &input, &rules, &mut Vec::new(), limits);
            let b = observe(&mut original, &input, &rules, &mut Vec::new(), limits);
            assert_eq!(a, b, "work={work_limit}, bytes={byte_limit}");
            if let Some((kind, _)) = a.error {
                assert_eq!(kind, ErrorKind::Limit);
            }
        }
    }
}

#[test]
fn failed_and_reused_scratch_keeps_history_and_existing_oracle_switches() {
    let small = pieces(&chain(0.0, 2.0, 0));
    let large = chunked_input();
    let rules = [FillRule::NonZero; 2];
    for original_helpers in [false, true] {
        let make = |original_frontier| Resolver {
            original_frontier,
            eager_midpoint_keys: original_helpers,
            original_height_sort: original_helpers,
            original_crossings: original_helpers,
            ..Resolver::default()
        };
        let (mut actual, mut original) = (make(false), make(true));
        let (mut out, mut reference) = (Vec::new(), Vec::new());
        for (input, limits) in [
            (small.as_slice(), (usize::MAX, usize::MAX)),
            (large.as_slice(), (usize::MAX, 1)),
            (large.as_slice(), (usize::MAX, usize::MAX)),
            (small.as_slice(), (1, usize::MAX)),
            (&[][..], (usize::MAX, usize::MAX)),
            (small.as_slice(), (usize::MAX, usize::MAX)),
        ] {
            let a = observe(&mut actual, input, &rules, &mut out, limits);
            let b = observe(&mut original, input, &rules, &mut reference, limits);
            assert_eq!(a, b, "helpers={original_helpers}, limits={limits:?}");
        }
    }
}

#[test]
fn asymmetric_open_owners_keep_the_original_swap_and_future_growth_history() {
    let input = pieces(&chain(0.0, 2.0, 0));
    // Setup can grow `open` and then refuse `next`, so a reused resolver
    // need not give these owners equal capacities. Four full joins swap
    // them four times; skipping three joins must not reverse their owners.
    for (open, next) in [(64, 16), (16, 64)] {
        let make = |original_frontier| Resolver {
            original_frontier,
            open: Vec::with_capacity(open),
            next: Vec::with_capacity(next),
            ..Resolver::default()
        };
        let (mut actual, mut original) = (make(false), make(true));
        let a = observe(
            &mut actual,
            &input,
            &[FillRule::NonZero],
            &mut Vec::new(),
            (usize::MAX, usize::MAX),
        );
        let b = observe(
            &mut original,
            &input,
            &[FillRule::NonZero],
            &mut Vec::new(),
            (usize::MAX, usize::MAX),
        );
        assert_eq!(a, b);
        assert_eq!(actual.frontier_successes, 0);
    }
}

#[test]
fn refused_implicit_heights_keep_the_next_rows_reservation_history() {
    let input = pieces(&chain(0.0, 2.0, 0));
    let rules = [FillRule::NonZero];
    let full = observe(
        &mut Resolver {
            original_frontier: true,
            ..Resolver::default()
        },
        &input,
        &rules,
        &mut Vec::new(),
        (usize::MAX, usize::MAX),
    );
    // Calibrate from the old route rather than duplicate its fee formula.
    // Stop after its endpoint initialization but before general placement.
    let (limit, mut original, old_error) = (0..full.work)
        .find_map(|limit| {
            let mut resolver = Resolver {
                original_frontier: true,
                ..Resolver::default()
            };
            let observation = observe(
                &mut resolver,
                &input,
                &rules,
                &mut Vec::new(),
                (limit, usize::MAX),
            );
            (observation.error.is_some()
                && resolver.rows.len() == input.len()
                && resolver.order.is_empty())
            .then_some((limit, resolver, observation))
        })
        .expect("the old crossing stage has a charged refusal boundary");
    assert_eq!(original.heights.len(), input.len() + 2);
    let mut actual = Resolver::default();
    let observed = observe(
        &mut actual,
        &input,
        &rules,
        &mut Vec::new(),
        (limit, usize::MAX),
    );
    assert_eq!(observed, old_error);

    // This row crosses the doubling boundary of the retained height
    // allocation. A shorter physical length left by an error would alter
    // its next reservation, although the first error's capacities matched.
    let mut followup: Vec<_> = (0..8_u32)
        .map(|n| {
            let a = f64::from(n) * 0.125;
            (line((0.25, a), (0.25, a + 0.125)), 0)
        })
        .collect();
    followup.push((line((2.0, 1.0), (2.0, 0.0)), 0));
    let followup = pieces(&followup);
    let limits = (usize::MAX, usize::MAX);
    let a = observe(&mut actual, &followup, &rules, &mut Vec::new(), limits);
    let b = observe(&mut original, &followup, &rules, &mut Vec::new(), limits);
    assert!(b.error.is_none());
    assert_eq!(a, b);
}
