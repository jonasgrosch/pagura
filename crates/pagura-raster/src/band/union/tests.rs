//! The row resolver against known areas, against an integration of the
//! row slab by slab, and its charges.

use super::*;
use crate::Point;

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

fn key_columns(pieces: &[Piece]) -> Vec<Span> {
    let mut columns: Vec<_> = (0_u32..)
        .zip(pieces)
        .map(|(i, p)| {
            let (left, right) = p.columns();
            (left, right, i)
        })
        .collect();
    columns.reverse();
    index_columns(&mut columns, pieces);
    columns
}

#[test]
fn midpoint_filter_separates_overlapping_piece_ranges_and_reuses_its_center() {
    let input = pieces(&[
        (line(0.0, 0.0, 100.0, 1.0), 0),
        (line(2.0, 0.0, 102.0, 1.0), 0),
    ]);
    let columns = key_columns(&input);
    assert!(separated(columns[0], columns[1]).is_none());
    let mut a = Place::new(0, &input, 0.5);
    let mut b = Place::new(1, &input, 0.5);
    a.ready = false;
    b.ready = false;
    assert!(compare(&mut a, &mut b, &input, 0.5).is_lt());
    assert!(!a.ready && !b.ready && a.approximate && b.approximate);
    let centers = (a.x.to_bits(), b.x.to_bits());
    assert!(!coincident(&mut a, &mut b, &input, 0.5));
    assert_eq!((a.x.to_bits(), b.x.to_bits()), centers);
    assert_eq!(a.key(&input, 0.5).to_bits(), input[0].line.x(0.5).to_bits());
    assert!(a.ready && !a.approximate);
}

#[test]
fn midpoint_gap_keeps_rounded_radius_ties_on_the_exact_path() {
    let radius = 2.0_f64.powi(-43);
    let tiny = f64::from_bits(1);
    let input = pieces(&[
        (line(0.0, 0.0, 2.0 * radius, 1.0), 0),
        (line(tiny, 0.0, tiny, 1.0), 0),
    ]);
    let mut a = Place::new(0, &input, 0.5);
    let mut b = Place::new(1, &input, 0.5);
    a.ready = false;
    assert_eq!(a.radius(), radius);
    assert_eq!(b.radius(), 0.0);
    // The real gap is just below the radius, but subtraction rounds to
    // the radius. A non-strict test would accept an unproved separation.
    assert_eq!(radius - tiny, radius);
    assert!(filtered(&mut a, &mut b, &input, 0.5).is_none());
    assert!(a.approximate && !a.ready);
    assert!(compare(&mut a, &mut b, &input, 0.5).is_gt());
    assert_eq!(a.x.to_bits(), input[0].line.x(0.5).to_bits());
    assert!(a.ready && !a.approximate);
}

#[test]
fn midpoint_gap_declines_nonfinite_centers_and_preserves_signed_zero() {
    let input = pieces(&[
        (line(-0.0, 0.0, -0.0, 1.0), 0),
        (line(0.0, 0.0, 0.0, 1.0), 0),
    ]);
    let mut a = Place::new(0, &input, 0.5);
    let mut b = Place::new(1, &input, 0.5);
    assert!(filtered(&mut a, &mut b, &input, 0.5).is_none());
    assert!(compare(&mut a, &mut b, &input, 0.5).is_lt());
    assert!(coincident(&mut a, &mut b, &input, 0.5));
    for x in [f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
        for ready in [false, true] {
            let mut a = Place {
                x,
                ready,
                approximate: !ready,
                radius_code: 1,
                ..Place::default()
            };
            assert!(filtered(&mut a, &mut b, &input, 0.5).is_none());
        }
    }
}

#[test]
fn midpoint_filter_encloses_literal_keys_with_cancellation_and_underflow() {
    let n = 549_755_813_888.0;
    let tiny = f64::from_bits(1);
    for edge in [
        line(-n, -n, n, n + 1.0 / 8192.0),
        line(n, n + 1.0 / 8192.0, -n, -n),
        line(-1.0e12, -1.0e12, 1.0e12, 1.0e12),
        line(tiny, 0.0, 2.0 * tiny, f64::MIN_POSITIVE),
        line(-0.0, 0.0, 0.0, f64::MIN_POSITIVE),
        line(0.25, f64::MIN_POSITIVE, 0.5, 2.0 * f64::MIN_POSITIVE),
    ] {
        let input = [Piece {
            line: edge,
            lo: edge.a.y.min(edge.b.y),
            hi: edge.a.y.max(edge.b.y),
            direction: 1,
            group: 0,
        }];
        assert_ne!(key_radius(&input[0]), 0);
        for k in 0..=16 {
            let mid = input[0].lo + (input[0].hi - input[0].lo) * (f64::from(k) / 16.0);
            let mut place = Place::new(0, &input, mid);
            place.ready = false;
            assert!(place.ensure_center(&input, mid));
            let (lo, hi) = (
                (place.x - place.radius()).next_down(),
                (place.x + place.radius()).next_up(),
            );
            let exact = edge.x(mid);
            assert!(
                lo <= exact && exact <= hi,
                "{edge:?}: {lo} <= {exact} <= {hi}"
            );
        }
    }
}

#[test]
fn midpoint_radius_codes_bound_the_reviewed_allowance() {
    for power in 0..=39 {
        let edge = line(2.0_f64.powi(power), 0.0, 0.0, 1.0);
        let input = pieces(&[(edge, 0)]);
        let code = key_radius(&input[0]);
        assert!((1..=41).contains(&code));
        let radius = f64::from_bits((u64::from(code) + 979) << 52);
        let error = (256.0 * f64::EPSILON) * ((1.0 + edge.a.x.abs()) + edge.b.x.abs());
        assert!(error < radius && radius <= 2.0 * error);
    }
    let input = pieces(&[(line(0.0, 0.0, 0.0, 1.0), 0)]);
    assert_eq!(key_radius(&input[0]), 1);
}

#[test]
fn midpoint_filter_unsupported_domain_uses_the_original_key() {
    let tiny = f64::from_bits(1);
    for edge in [
        line(0.25, 0.0, 0.5, 2.0 * tiny),
        line(1.0e13, 0.0, 1.0e13 + 2.0, 1.0),
        line(f64::INFINITY, 0.0, 2.0, 1.0),
        line(f64::NAN, 0.0, 2.0, 1.0),
    ] {
        let input = [Piece {
            line: edge,
            lo: edge.a.y,
            hi: edge.b.y,
            direction: 1,
            group: 0,
        }];
        assert_eq!(key_radius(&input[0]), 0);
        let mid = input[0].lo.midpoint(input[0].hi);
        let mut place = Place::new(0, &input, mid);
        place.ready = false;
        assert!(!place.ensure_center(&input, mid));
        let actual = place.key(&input, mid);
        let expected = edge.x(mid);
        assert!(actual.to_bits() == expected.to_bits() || actual.is_nan() && expected.is_nan());
        assert!(place.ready && !place.approximate);
    }
}

#[test]
fn indexed_columns_reuses_clipped_ranges_for_long_diagonals() {
    let input = pieces(&[
        (line(0.0, 0.0, 1000.0, 1000.0), 0),
        (line(1002.0, 1000.0, 2.0, 0.0), 0),
    ]);
    let columns = key_columns(&input);
    assert_eq!((columns[0].2, columns[1].2), (0, 1));
    assert!(columns[0].1 < columns[1].0);
    let mut a = Place {
        radius_code: key_radius(&input[0]),
        ..Place::default()
    };
    let mut b = Place {
        piece: 1,
        radius_code: key_radius(&input[1]),
        ..Place::default()
    };
    assert!(compare(&mut a, &mut b, &input, 0.5).is_lt());
    assert!(!coincident(&mut a, &mut b, &input, 0.5));
    assert!(!a.ready && !b.ready);
    let mut lines = polygon(
        &[(0.0, 0.0), (2.0, 0.0), (1002.0, 1000.0), (1000.0, 1000.0)],
        0,
    );
    lines.extend(polygon(
        &[(8.0, 0.125), (9.0, 0.125), (9.0, 0.625), (8.0, 0.625)],
        0,
    ));
    compare_ordering(&pieces(&lines), &[FillRule::NonZero]);
}

#[test]
fn key_bounds_enclose_cancellation_and_subnormal_interpolation() {
    let n = 549_755_813_888.0;
    let tiny = f64::from_bits(1);
    let witnesses = [
        line(-n, -n, n, n + 1.0 / 8192.0),
        line(-1.0e12, 0.0, 1.0e12, 1.0),
        line(-2.0, 0.5, 2.0, 0.5 + f64::EPSILON),
        line(0.25, 0.0, 0.75, 3.0 * tiny),
        line(-tiny, 0.0, tiny, 1.0),
        line(-0.0, tiny, 0.0, 2.0 * tiny),
    ];
    for line in witnesses {
        let p = Piece::new(line, 0.0, 0).unwrap();
        let (left, right) = key_bounds(&p, p.columns());
        for step in 0..=64 {
            let t = f64::from(step) / 64.0;
            let y = (p.lo * (1.0 - t) + p.hi * t).clamp(p.lo, p.hi);
            let x = line.x(y);
            assert!(left <= x && x <= right, "{p:?}, y={y:?}, x={x:?}");
        }
    }
}

#[test]
fn unsupported_key_bounds_keep_the_original_fallback() {
    for (line, lo, hi) in [
        (line(2.0e12, 0.0, 3.0e12, 1.0), 0.0, 1.0),
        (line(1.0, 2.0e12, 2.0, 2.0e12 + 1.0), 2.0e12, 2.0e12 + 1.0),
        (line(1.0, 0.0, 2.0, 1.0), -1.0, 2.0),
    ] {
        let p = Piece {
            line,
            lo,
            hi,
            direction: 1,
            group: 0,
        };
        assert_eq!(key_bounds(&p, (1.25, 1.75)), (line.a.x, line.b.x));
    }
    for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let p = Piece {
            line: line(x, 0.0, 1.0, 1.0),
            lo: 0.0,
            hi: 1.0,
            direction: 1,
            group: 0,
        };
        let (left, right) = key_bounds(&p, (0.0, 1.0));
        let range = (left, right, 0);
        assert_eq!((left, right), (f64::NEG_INFINITY, f64::INFINITY));
        assert!(separated(range, (2.0, 3.0, 1)).is_none());
        assert!(separated((2.0, 3.0, 1), range).is_none());
    }
}

#[test]
fn column_index_cycles_do_not_displace_completed_bounds() {
    let pieces = pieces(&[
        (line(0.0, 0.0, 1000.0, 1000.0), 0),
        (line(2.0, 0.0, 1002.0, 1000.0), 0),
        (line(4.0, 0.0, 1004.0, 1000.0), 0),
        (line(6.0, 0.0, 1006.0, 1000.0), 0),
    ]);
    let expected = key_columns(&pieces);
    for permutation in [[1, 2, 3, 0], [2, 0, 3, 1], [0, 2, 1, 3]] {
        let mut columns: Vec<_> = permutation
            .into_iter()
            .map(|i| {
                let (left, right) = pieces[i as usize].columns();
                (left, right, i)
            })
            .collect();
        index_columns(&mut columns, &pieces);
        assert_eq!(columns, expected);
    }
    index_columns(&mut [], &[]);
    let p = &pieces[..1];
    let mut column = vec![(p[0].columns().0, p[0].columns().1, 0)];
    index_columns(&mut column, p);
    assert_eq!(column[0], expected[0]);
}

#[test]
fn row_axis_bounds_reject_apart_pairs_and_keep_real_crossings() {
    let input = pieces(&[
        (line(-10.0, 0.0, -9.0, 0.4), 0),
        (line(9.0, 0.2, 10.0, 0.6), 0),
        (line(-10.0, 0.8, 30.0, 0.81), 0),
        (line(-10.0, 0.85, 30.0, 0.86), 0),
        (line(20.0, 0.65, 21.0, 0.75), 0),
        (line(21.0, 0.65, 20.0, 0.75), 0),
    ]);
    let mut rows: Vec<_> = (0_u32..)
        .zip(&input)
        .map(|(i, p)| (p.lo, p.hi, i))
        .collect();
    let mut columns: Vec<_> = (0_u32..)
        .zip(&input)
        .map(|(i, p)| {
            let (left, right) = p.columns();
            (left, right, i)
        })
        .collect();
    let by_start = |a: &Span, b: &Span| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2));
    rows.sort_unstable_by(by_start);
    columns.sort_unstable_by(by_start);
    assert!(overlapping(&rows) < overlapping(&columns));
    let (_, resolver) = compare_ordering(&input, &[FillRule::NonZero]);
    assert!(resolver.heights.iter().any(|&y| y > 0.65 && y < 0.75));
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

/// Compare the complete side coordinates and resource charges with the
/// original midpoint ordering, including the sign of every zero.
fn compare_ordering(pieces: &[Piece], rules: &[FillRule]) -> (Vec<Line>, Resolver) {
    let run = |resolver: &mut Resolver| {
        let mut work = Meter::new(usize::MAX, "work");
        let mut bytes = Meter::new(usize::MAX, "bytes");
        let mut out = Vec::new();
        resolver
            .row(
                pieces,
                rules,
                0.0,
                (
                    &mut out,
                    Meters {
                        work: &mut work,
                        bytes: &mut bytes,
                    },
                ),
            )
            .unwrap();
        (out, work.used(), bytes.used())
    };
    let mut demand = Resolver::default();
    let mut midpoint = Resolver {
        eager_midpoint_keys: true,
        eager_joining_keys: true,
        original_height_sort: true,
        original_crossings: true,
        ..Resolver::default()
    };
    let (actual, work, bytes) = run(&mut demand);
    let (expected, old_work, old_bytes) = run(&mut midpoint);
    let bits = |lines: &[Line]| {
        lines
            .iter()
            .map(|l| {
                [
                    l.a.x.to_bits(),
                    l.a.y.to_bits(),
                    l.b.x.to_bits(),
                    l.b.y.to_bits(),
                ]
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&actual), bits(&expected));
    assert_eq!(work, old_work);
    assert_eq!(bytes, old_bytes);
    (actual, demand)
}

#[test]
fn demand_keys_keeps_coincident_ids_and_staggered_joins() {
    let mark = polygon(&[(-6.0, 0.0), (-3.0, 0.0), (-2.0, 1.0), (-5.0, 1.0)], 0);
    let mut lines = [mark.clone(), mark].concat();
    lines.extend(polygon(
        &[(-8.0, 0.0), (0.0, 0.0), (1.0, 1.0), (-7.0, 1.0)],
        1,
    ));
    lines.extend(polygon(
        &[(4.0, 0.125), (5.0, 0.125), (5.0, 0.625), (4.0, 0.625)],
        0,
    ));
    let (out, resolver) = compare_ordering(&pieces(&lines), &[FillRule::NonZero; 2]);
    assert_eq!(
        out,
        [line(-6.0, 0.0, -5.0, 1.0), line(-2.0, 1.0, -3.0, 0.0)]
    );
    // Separated singleton groups never needed their current midpoint key.
    assert!(resolver.order.iter().any(|entry| !entry.ready));
    assert_eq!(enclosed(&out), 3.0);
}

#[test]
fn demand_keys_preserves_chunk_carries_and_both_fill_rules() {
    let mut lines = Vec::new();
    for n in 0..128_u32 {
        let x = f64::from(n) * 4.0;
        let y = f64::from(n % 4) * 0.125;
        lines.extend(polygon(
            &[
                (x, y),
                (x + 1.0, y),
                (x + 1.5, y + 0.375),
                (x + 0.5, y + 0.375),
            ],
            0,
        ));
    }
    lines.extend(polygon(
        &[(-2.0, 0.0), (514.0, 0.0), (514.0, 1.0), (-2.0, 1.0)],
        1,
    ));
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let (out, resolver) = compare_ordering(&pieces(&lines), &[rule, FillRule::NonZero]);
        assert_ne!(resolver.cuts, [] as [f64; 0]);
        assert_eq!(enclosed(&out), 48.0);
    }
}

#[test]
fn rounding_ties_without_a_crossing_keep_midpoint_ordering() {
    let high = 1.0 + f64::EPSILON;
    let mut lines = vec![
        (line(high, 0.0, 1.0, 1.0), 0),
        (line(1.0, 1.0, 1.0, 0.0), 0),
    ];
    lines.extend(polygon(
        &[(4.0, 0.5), (5.0, 0.5), (5.0, 1.0), (4.0, 1.0)],
        0,
    ));
    let (_, resolver) = compare_ordering(&pieces(&lines), &[FillRule::NonZero]);
    assert_eq!(resolver.heights, [0.0, 0.5, 1.0]);
    // The first two edges never strictly cross. Their rounded keys do
    // coincide in the last slab, restoring the original piece-index tie.
    assert_eq!(
        resolver.order[..2]
            .iter()
            .map(|entry| (entry.x, entry.piece))
            .collect::<Vec<_>>(),
        [(1.0, 0), (1.0, 1)]
    );
    let columns = key_columns(&pieces(&lines));
    assert!(separated(columns[1], columns[0]).is_none());
}

#[test]
fn demand_keys_rejects_unproved_signed_zero_relations() {
    let mixed = line(-0.0, f64::from_bits(1), 0.0, f64::from_bits(2));
    let constant = line(-0.0, 0.0, -0.0, 1.0);
    let columns = key_columns(&pieces(&[(mixed, 0), (constant, 0)]));
    assert!(separated(columns[0], columns[1]).is_none());
    assert!(separated(columns[1], columns[0]).is_none());
    let (_, resolver) = compare_ordering(
        &pieces(&[(mixed, 0), (constant, 0), (line(2.0, 1.0, 2.0, 0.0), 0)]),
        &[FillRule::NonZero],
    );
    // The mixed line's subnormal slab rounds its midpoint to the lower
    // endpoint. It cannot be grouped with the constant negative zero by
    // an unproved numerical-equality shortcut.
    assert!(resolver.heights.contains(&f64::from_bits(2)));
}

#[test]
fn demand_keys_distinguish_order_from_numeric_zero_equality() {
    let pieces = pieces(&[
        (line(-0.0, 0.0, -0.0, 1.0), 0),
        (line(0.0, 1.0, 0.0, 0.0), 0),
    ]);
    let mut a = Place {
        piece: 0,
        ..Place::default()
    };
    let mut b = Place {
        piece: 1,
        ..Place::default()
    };
    assert!(compare(&mut a, &mut b, &pieces, 0.5).is_lt());
    assert!(a.ready && b.ready);
    assert_eq!(a.x.to_bits(), (-0.0_f64).to_bits());
    assert_eq!(b.x.to_bits(), 0.0_f64.to_bits());
    assert!(coincident(&mut a, &mut b, &pieces, 0.5));
}

#[test]
fn demand_keys_recompute_ambiguous_survivors_at_each_midpoint() {
    let pieces = pieces(&[
        (line(1.0 + f64::EPSILON, 0.0, 1.0, 1.0), 0),
        (line(1.0, 1.0, 1.0, 0.0), 0),
    ]);
    let mut a = Place {
        piece: 0,
        ..Place::default()
    };
    let mut b = Place {
        piece: 1,
        ..Place::default()
    };
    assert!(compare(&mut a, &mut b, &pieces, 0.25).is_gt());
    assert!(!coincident(&mut a, &mut b, &pieces, 0.25));
    (a.ready, b.ready) = (false, false);
    assert!(compare(&mut a, &mut b, &pieces, 0.75).is_lt());
    assert!(coincident(&mut a, &mut b, &pieces, 0.75));
}

#[test]
fn demand_keys_leave_proven_separation_unevaluated() {
    let pieces = pieces(&[
        (line(-4.0, 0.0, -3.0, 1.0), 0),
        (line(1.0, 1.0, 0.0, 0.0), 0),
    ]);
    let mut a = Place {
        piece: 0,
        radius_code: key_radius(&pieces[0]),
        ..Place::default()
    };
    let mut b = Place {
        piece: 1,
        radius_code: key_radius(&pieces[1]),
        ..Place::default()
    };
    assert!(compare(&mut a, &mut b, &pieces, 0.5).is_lt());
    assert!(!coincident(&mut a, &mut b, &pieces, 0.5));
    assert!(!a.ready && !b.ready);
}

#[test]
fn demand_keys_keeps_the_original_resource_acceptance_boundary() {
    let mut lines = polygon(&[(-4.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (-3.0, 1.0)], 0);
    lines.extend(polygon(
        &[(4.0, 0.125), (5.0, 0.125), (5.0, 0.625), (4.0, 0.625)],
        0,
    ));
    let pieces = pieces(&lines);
    let attempt = |eager_midpoint_keys, work_limit, byte_limit| {
        let mut work = Meter::new(work_limit, "work");
        let mut bytes = Meter::new(byte_limit, "bytes");
        let result = Resolver {
            eager_midpoint_keys,
            eager_joining_keys: eager_midpoint_keys,
            original_height_sort: eager_midpoint_keys,
            original_crossings: eager_midpoint_keys,
            ..Resolver::default()
        }
        .row(
            &pieces,
            &[FillRule::NonZero],
            0.0,
            (
                &mut Vec::new(),
                Meters {
                    work: &mut work,
                    bytes: &mut bytes,
                },
            ),
        );
        (result.is_ok(), work.used(), bytes.used())
    };
    let (passed, work, bytes) = attempt(true, usize::MAX, usize::MAX);
    assert!(passed);
    for work_limit in [work - 1, work, work + 1] {
        for byte_limit in [bytes - 1, bytes, bytes + 1] {
            assert_eq!(
                attempt(false, work_limit, byte_limit),
                attempt(true, work_limit, byte_limit)
            );
        }
    }
}

#[test]
fn height_merge_keeps_crossings_carry_and_both_fill_rules() {
    let mut lines = polygon(&[(-2.0, 0.0), (2.0, 1.0), (-2.0, 1.0), (2.0, 0.0)], 0);
    lines.extend(polygon(
        &[(-3.0, 0.0), (3.0, 0.0), (3.0, 1.0), (-3.0, 1.0)],
        1,
    ));
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let (out, resolver) = compare_ordering(&pieces(&lines), &[rule, FillRule::NonZero]);
        assert!(resolver.heights.contains(&0.5));
        assert_eq!(enclosed(&out), 2.0);
    }
}

#[test]
fn height_merge_preserves_sorted_bits_and_zero_representative() {
    let rows = [
        (-0.0, 0.5, 0),
        (0.0, 1.0, 1),
        (0.125, 0.75, 2),
        (0.5, 1.0, 3),
        (0.5, 0.875, 4),
        (0.75, 1.0, 5),
    ];
    let mut heights = vec![0.0, 1.0, 0.75, 0.5, 1.0, 0.75, 1.0, 0.875, -0.0];
    let mut expected = heights.clone();
    expected.extend(rows.iter().map(|row| row.0));
    expected.sort_unstable_by(f64::total_cmp);
    heights.reserve(rows.len());
    heights.sort_unstable_by(f64::total_cmp);
    merge_heights(&mut heights, &rows);
    let bits = |values: &[f64]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&heights), bits(&expected));
    heights.dedup();
    expected.dedup();
    assert_eq!(bits(&heights), bits(&expected));
    assert_eq!(heights[0].to_bits(), (-0.0_f64).to_bits());
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
            let actual = clip(piece, (0.0, 1.0));
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
