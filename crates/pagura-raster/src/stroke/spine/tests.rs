//! The spine's convex sets against sampled points, and its budgets.

use super::*;
use crate::flatten::TOLERANCE;

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

    /// A value in `lo..hi`.
    fn within(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * f64::from(self.next()) / f64::from(u32::MAX >> 1)
    }
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let d = b - a;
    let l = d.dot(d);
    let t = if l > 0.0 {
        ((p - a).dot(d) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + d * t)).length()
}

/// Every cell a capsule or quad claims has its four corners inside it.
#[test]
fn capsule_cells_lie_inside() {
    let mut rng = Lcg(7);
    let mut cells = 0;
    for case in 0..2_000 {
        let a = Point::new(rng.within(0.0, 20.0), rng.within(0.0, 20.0));
        let b = if case % 5 == 0 {
            a
        } else {
            Point::new(rng.within(0.0, 20.0), rng.within(0.0, 20.0))
        };
        let radius = rng.within(0.5, 9.0);
        let round = case % 3 != 0;
        let length = (b - a).length();
        let capsule = Capsule {
            a,
            b,
            radius,
            round,
            top: a.y.min(b.y) - radius,
            bottom: a.y.max(b.y) + radius,
            unit: if length > 0.0 {
                (b - a) / length
            } else {
                Point::default()
            },
            length,
        };
        for row in -10..32 {
            let top = f64::from(row);
            let Some((first, last)) = capsule.cells((top, top + 1.0)) else {
                continue;
            };
            let mut c = first;
            while c <= last {
                for corner in [
                    (c, top),
                    (c + 1.0, top),
                    (c, top + 1.0),
                    (c + 1.0, top + 1.0),
                ] {
                    let p = Point::new(corner.0, corner.1);
                    let inside = if round {
                        distance_to_segment(p, a, b) <= radius + 1e-9
                    } else {
                        let d = b - a;
                        let l = d.length();
                        let (u, n) = (d / l, (d / l).perp());
                        ((p - a).dot(n)).abs() <= radius + 1e-9
                            && (p - a).dot(u) >= -1e-9
                            && (p - a).dot(u) <= l + 1e-9
                    };
                    assert!(inside, "case {case}: {p:?} {capsule:?}");
                }
                cells += 1;
                c += 1.0;
            }
        }
    }
    assert!(cells > 50_000, "{cells}");
}

/// A veto's extent in a row holds every point of its region there.
#[test]
fn veto_extents_hold_their_regions() {
    let mut rng = Lcg(11);
    for case in 0..2_000 {
        let centre = Point::new(rng.within(0.0, 10.0), rng.within(0.0, 10.0));
        let angle = rng.within(0.0, std::f64::consts::TAU);
        let normal = Point::new(angle.cos(), angle.sin());
        let radius = rng.within(0.5, 6.0);
        let offset = rng.within(-0.5, 1.0) * radius;
        let veto = Veto {
            centre,
            normal,
            offset,
            radius,
            top: f64::NEG_INFINITY,
            bottom: f64::INFINITY,
        };
        for row in -2..14 {
            let top = f64::from(row);
            let extent = veto.extent((top, top + 1.0));
            for _ in 0..64 {
                let p = Point::new(rng.within(-8.0, 18.0), rng.within(top, top + 1.0));
                let d = p - centre;
                if d.length() <= radius && d.dot(normal) >= offset {
                    let (lo, hi) = extent.unwrap_or_else(|| panic!("case {case}: {p:?}"));
                    assert!(lo <= p.x && p.x <= hi, "case {case}: {p:?} {lo} {hi}");
                }
            }
        }
    }
}

fn dot_run(centre: Point, around: f64, count: u32) -> Run {
    Run {
        points: (0..count)
            .map(|k| {
                let t = std::f64::consts::TAU * f64::from(k) / f64::from(count);
                Point::new(centre.x + around * t.cos(), centre.y + around * t.sin())
            })
            .collect(),
        closed: true,
        tangent: Point::new(1.0, 0.0),
    }
}

/// A spine the byte budget cannot admit is left out, its bytes returned; one
/// it admits stays charged as long as it lives.
#[test]
fn spines_the_byte_budget_cannot_admit_are_left_out() {
    let runs = vec![dot_run(Point::new(20.0, 20.0), 3.0, 60)];
    let pen = Pen {
        radius: 8.0,
        tolerance: TOLERANCE,
    };
    let stroke = Stroke {
        width: 16.0,
        join: LineJoin::Round,
        ..Stroke::default()
    };
    let limits = Limits::default();
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let spine = Spine::of(
        &runs,
        |_| false,
        (pen, Transform::IDENTITY),
        &stroke,
        &limits,
        &mut bytes,
    )
    .unwrap();
    assert_eq!(bytes.used(), spine.heap_bytes());
    let needed = spine.heap_bytes();
    for budget in [0, 64, needed / 2] {
        let mut small = Meter::new(budget, "bytes");
        let spine = Spine::of(
            &runs,
            |_| false,
            (pen, Transform::IDENTITY),
            &stroke,
            &limits,
            &mut small,
        );
        assert!(spine.is_none(), "{budget}");
        assert_eq!(small.used(), 0, "{budget}");
    }
    // Neither a pen too narrow for a cell nor a stretching map has one.
    let narrow = Pen {
        radius: 0.5,
        tolerance: TOLERANCE,
    };
    let none = Spine::of(
        &runs,
        |_| false,
        (narrow, Transform::IDENTITY),
        &stroke,
        &limits,
        &mut bytes,
    );
    assert!(none.is_none());
    let stretch = Transform::new(2.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    let none = Spine::of(
        &runs,
        |_| false,
        (pen, stretch),
        &stroke,
        &limits,
        &mut bytes,
    );
    assert!(none.is_none());
}

/// Proving a row's cells is refused before its work goes past the budget.
#[test]
fn cover_rows_refuse_within_their_work_budget() {
    let runs = vec![dot_run(Point::new(20.0, 20.0), 3.0, 60)];
    let pen = Pen {
        radius: 8.0,
        tolerance: TOLERANCE,
    };
    let stroke = Stroke {
        width: 16.0,
        join: LineJoin::Bevel,
        ..Stroke::default()
    };
    let limits = Limits::default();
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let spine = Spine::of(
        &runs,
        |_| false,
        (pen, Transform::IDENTITY),
        &stroke,
        &limits,
        &mut bytes,
    )
    .unwrap();
    let mut chunks = Vec::new();
    let mut needed = 0;
    for budget in 0.. {
        let mut cover = spine.cover(None);
        let mut work = Meter::new(budget, "work");
        chunks.clear();
        if cover
            .row(20.0, usize::MAX, &mut chunks, (&mut work, &mut bytes))
            .is_ok()
        {
            needed = budget;
            break;
        }
        assert!(work.used() <= budget);
    }
    assert!(needed > 0);
    assert_ne!(chunks.len(), 0);
}
