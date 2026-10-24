//! General singleton-key differentials against eager original joining.
use super::{Line, Meters, Piece, Place, Resolver, key_radius};
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
    let mut work = Meter::new(work_limit, "joining test work");
    let mut bytes = Meter::new(byte_limit, "joining test bytes");
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

#[test]
fn deferred_constant_key_preserves_negative_zero_when_first_read() {
    let input = pieces(&[(line((-0.0, 0.0), (-0.0, 1.0)), 0)]);
    let code = key_radius(&input[0]);
    assert_ne!(code, 0);
    let mut entry = Place::deferred(0, &input, code);
    assert!(entry.constant && !entry.ready && !entry.approximate);
    assert_eq!(entry.radius_code, code);
    assert!(entry.ensure_center(&input, 0.5));
    assert!(!entry.ready);
    assert_eq!(entry.key(&input, 0.5).to_bits(), (-0.0_f64).to_bits());
    assert!(entry.ready && !entry.approximate);
    assert_eq!(entry.key(&input, 0.75).to_bits(), (-0.0_f64).to_bits());
}

/// The general joining path has no whole-frontier machinery. The
/// reference evaluates every joining key before its original fee.
fn joining_differential(input: &[Piece]) -> Vec<Line> {
    let mut actual = Resolver::default();
    let mut original = Resolver {
        eager_joining_keys: true,
        ..Resolver::default()
    };
    let (mut out, mut reference) = (Vec::new(), Vec::new());
    let limits = (usize::MAX, usize::MAX);
    let a = observe(&mut actual, input, &[FillRule::NonZero], &mut out, limits);
    let b = observe(
        &mut original,
        input,
        &[FillRule::NonZero],
        &mut reference,
        limits,
    );
    assert!(b.error.is_none(), "{b:?}");
    assert_eq!(a, b);
    out
}

#[test]
fn general_singleton_keys_keep_multishift_order_and_unsupported_fallbacks() {
    for offset in [0.0, 1.0e13, -1.0e13] {
        let mut lines = chain(offset + 3.0, offset + 8.0, 0);
        for (left, right) in [(-2.0, 6.0), (0.0, 5.0), (1.0, 4.0)] {
            lines.extend(polygon(
                &[
                    (offset + left, 0.0),
                    (offset + left, 1.0),
                    (offset + right, 1.0),
                    (offset + right, 0.0),
                ],
                0,
            ));
        }
        // One entrant passes four survivors before reaching its rank.
        // The distant copies have code zero and require the exact key.
        let out = joining_differential(&pieces(&lines));
        assert_eq!(area(&out), 10.0);
        let expected = [
            line((offset - 2.0, 0.0), (offset - 2.0, 1.0)),
            line((offset + 8.0, 1.0), (offset + 8.0, 0.0)),
        ];
        assert_eq!(bits(&out), bits(&expected));
    }
}

#[test]
fn general_singleton_keys_preserve_rounded_ties_and_signed_zero_group_heads() {
    let points = [
        (1.0 + f64::EPSILON, 0.0),
        (1.0, 0.25),
        (1.0 + f64::EPSILON, 0.5),
        (1.0, 0.75),
        (1.0 + f64::EPSILON, 1.0),
    ];
    let mut lines: Vec<_> = points
        .windows(2)
        .map(|pair| (line(pair[0], pair[1]), 0))
        .collect();
    lines.push((line((3.0, 1.0), (3.0, 0.0)), 0));
    lines.extend(polygon(
        &[(1.0, 0.0), (1.0, 1.0), (2.0, 1.0), (2.0, 0.0)],
        0,
    ));
    let out = joining_differential(&pieces(&lines));
    assert_eq!(area(&out), 2.0);
    let expected: Vec<_> = lines[..5].iter().map(|&(edge, _)| edge).collect();
    assert_eq!(bits(&out), bits(&expected));

    let mut lines = Vec::new();
    for k in 0..4_u32 {
        let a = f64::from(k) * 0.25;
        let b = f64::from(k + 1) * 0.25;
        lines.push((line((-0.0, a), (-0.0, b)), 0));
    }
    lines.push((line((2.0, 1.0), (2.0, 0.0)), 0));
    lines.extend(polygon(
        &[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)],
        0,
    ));
    let out = joining_differential(&pieces(&lines));
    assert_eq!(area(&out), 2.0);
    let expected: Vec<_> = lines[..5].iter().map(|&(edge, _)| edge).collect();
    assert_eq!(bits(&out), bits(&expected));
}

#[test]
fn general_singleton_keys_keep_refusal_and_reused_scratch_history() {
    let input = pieces(&chain(0.0, 2.0, 0));
    let make = |eager_joining_keys| Resolver {
        eager_joining_keys,
        ..Resolver::default()
    };
    let mut old = make(true);
    let full = observe(
        &mut old,
        &input,
        &[FillRule::NonZero],
        &mut Vec::new(),
        (usize::MAX, usize::MAX),
    );
    assert!(full.error.is_none() && full.work > 1 && full.bytes > 1);
    for work in [0, 1, full.work - 1, full.work, full.work + 1] {
        for bytes in [0, 1, full.bytes - 1, full.bytes, full.bytes + 1] {
            let (mut actual, mut original) = (make(false), make(true));
            let (mut out, mut reference) = (Vec::new(), Vec::new());
            // Reuse the failed or successful owners before another row.
            for limits in [(work, bytes), (usize::MAX, usize::MAX)] {
                let a = observe(&mut actual, &input, &[FillRule::NonZero], &mut out, limits);
                let b = observe(
                    &mut original,
                    &input,
                    &[FillRule::NonZero],
                    &mut reference,
                    limits,
                );
                assert_eq!(a, b, "work={work}, bytes={bytes}, limits={limits:?}");
            }
        }
    }
}
