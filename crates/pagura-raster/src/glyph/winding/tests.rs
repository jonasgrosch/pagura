use super::*;
use crate::{GlyphGeometry, PathBuilder};
use pagura_core::Limits;

fn geometry(build: impl FnOnce(&mut PathBuilder)) -> GlyphGeometry {
    let mut path = PathBuilder::new();
    build(&mut path);
    GlyphGeometry::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &Limits::default()).unwrap()
}

// The chain count of a certified glyph.
fn certified(build: impl FnOnce(&mut PathBuilder)) -> Option<usize> {
    let chains = MonotoneChains::new(geometry(build).contours())?;
    let count = chains.count();
    chains.certified().then_some(count)
}

fn polygon(path: &mut PathBuilder, points: &[(f64, f64)]) {
    path.move_to(Point::new(points[0].0, points[0].1));
    for &(x, y) in &points[1..] {
        path.line_to(Point::new(x, y)).unwrap();
    }
    path.close();
}

#[test]
fn simple_and_nested_contours_with_alternating_orientation_are_certified() {
    assert_eq!(
        certified(|p| {
            p.rect(0.0, 0.0, 4.0, 4.0);
        }),
        Some(2)
    );
    // A hole reverses the outer orientation.
    let ring = |p: &mut PathBuilder| {
        polygon(p, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        polygon(p, &[(3.0, 3.0), (3.0, 7.0), (7.0, 7.0), (7.0, 3.0)]);
    };
    assert_eq!(certified(ring), Some(4));
    // An island inside the hole has the outer orientation again.
    assert_eq!(
        certified(|p| {
            ring(p);
            polygon(p, &[(4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)]);
        }),
        Some(6)
    );
    // Contours meeting at a shared vertex, and a final open point contour.
    assert!(
        certified(|p| {
            polygon(p, &[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0)]);
            polygon(p, &[(2.0, 2.0), (4.0, 2.0), (4.0, 4.0)]);
            p.move_to(Point::new(9.0, 9.0));
        })
        .is_some()
    );
}

#[test]
fn overlaps_mixed_orientations_and_points_are_not_certified() {
    // Same orientation inside: winding 2.
    assert_eq!(
        certified(|p| {
            polygon(p, &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
            polygon(p, &[(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0)]);
        }),
        None
    );
    // Disjoint contours of opposite orientation: windings 1 and -1.
    assert_eq!(
        certified(|p| {
            polygon(p, &[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]);
            polygon(p, &[(5.0, 0.0), (5.0, 2.0), (7.0, 2.0), (7.0, 0.0)]);
        }),
        None
    );
    // Crossing edges of one contour.
    assert_eq!(
        certified(|p| polygon(p, &[(0.0, 0.0), (4.0, 4.0), (4.0, 0.0), (0.0, 4.0)])),
        None
    );
    // A shallow crossing between partly overlapping squares.
    assert_eq!(
        certified(|p| {
            p.rect(0.0, 0.0, 4.0, 4.0);
            p.rect(3.999_999, 1.0, 4.0, 1.0);
        }),
        None
    );
    // A geometric point contour keeps its floor-cell rule elsewhere.
    assert_eq!(
        certified(|p| {
            p.rect(0.0, 0.0, 4.0, 4.0);
            p.move_to(Point::new(8.0, 8.0));
            p.close();
        }),
        None
    );
}

#[test]
fn chains_split_at_horizontal_edges_and_turns() {
    // An L shape: two horizontal edges inside the contour's height.
    let shape = |p: &mut PathBuilder| {
        polygon(
            p,
            &[
                (0.0, 0.0),
                (6.0, 0.0),
                (6.0, 2.0),
                (2.0, 2.0),
                (2.0, 8.0),
                (0.0, 8.0),
            ],
        );
    };
    assert_eq!(certified(shape), Some(3));
    // A zigzag down one side keeps a single chain on the other.
    assert_eq!(
        certified(|p| polygon(
            p,
            &[
                (0.0, 0.0),
                (1.0, 2.0),
                (0.5, 4.0),
                (1.5, 6.0),
                (0.0, 8.0),
                (-3.0, 4.0)
            ]
        )),
        Some(2)
    );
}

// A comb whose teeth end at increasing heights: every slab below a tip keeps
// all longer teeth active, so the sweep visits about T^2 + T chains for T
// teeth and 4T edges.
fn comb(path: &mut PathBuilder, teeth: u32) {
    let pitch = 0.01;
    let x = |i: u32| f64::from(i) * pitch;
    let mut points = vec![(0.0, 0.0), (x(2 * teeth - 1), 0.0)];
    for i in (0..teeth).rev() {
        let tip = 2.0 + f64::from(i) * pitch;
        points.extend([(x(2 * i + 1), tip), (x(2 * i), tip)]);
        if i > 0 {
            points.extend([(x(2 * i), 1.0), (x(2 * i - 1), 1.0)]);
        }
    }
    polygon(path, &points);
}

#[test]
fn the_sweep_stops_at_its_work_cap() {
    // 64 teeth take about 4,200 steps, within 32 * 256 + 1024.
    assert_eq!(certified(|p| comb(p, 64)), Some(128));
    assert!(geometry(|p| comb(p, 64)).signed_area());
    // 1,000 teeth would take about 10^6, beyond 32 * 4,000 + 1024. The error
    // bound admits this narrow comb, so only the cap rejects it.
    let large = geometry(|p| comb(p, 1000));
    assert_eq!(large.vertices(), 4000);
    assert!(MonotoneChains::new(large.contours()).is_some_and(|c| c.count() == 2000));
    assert_eq!(certified(|p| comb(p, 1000)), None);
    assert!(!large.signed_area());
}
