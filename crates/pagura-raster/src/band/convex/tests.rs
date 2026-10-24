use super::*;
use crate::band::edges::Line;

fn polygon(points: &[Point]) -> Polygon {
    Polygon {
        points: points.to_vec(),
        starts: vec![0],
        points_cells: false,
    }
}

fn convex(points: &[Point]) -> Option<Convex> {
    Convex::new(&polygon(points), &mut crate::testing::bytes()).unwrap()
}

fn square() -> Vec<Point> {
    vec![
        Point::new(1.0, 2.0),
        Point::new(5.0, 2.0),
        Point::new(5.0, 7.0),
        Point::new(1.0, 7.0),
    ]
}

/// A regular `count`-gon around `centre`, turned by `angle` and stretched
/// by `scale` along x: convex, with sides at every slope.
fn regular(count: u32, centre: Point, radius: f64, angle: f64, scale: f64) -> Vec<Point> {
    let step = std::f64::consts::TAU / f64::from(count);
    (0..count)
        .map(|i| {
            let a = angle + f64::from(i) * step;
            Point::new(
                centre.x + scale * radius * a.cos(),
                centre.y + radius * a.sin(),
            )
        })
        .collect()
}

#[test]
fn chains_run_down_both_sides_in_either_orientation() {
    let mut points = square();
    for _ in 0..2 {
        let chains = convex(&points).unwrap();
        assert_eq!(
            chains.left,
            [Point::new(1.0, 2.0), Point::new(1.0, 7.0)],
            "{points:?}"
        );
        assert_eq!(chains.right, [Point::new(5.0, 2.0), Point::new(5.0, 7.0)]);
        assert_eq!((chains.top, chains.bottom), (2.0, 7.0));
        points.reverse();
    }
    // A pointed top and bottom: both chains start and end there.
    let diamond = [
        Point::new(3.0, 0.0),
        Point::new(6.0, 4.0),
        Point::new(3.0, 9.0),
        Point::new(0.0, 4.0),
    ];
    let chains = convex(&diamond).unwrap();
    assert_eq!(chains.left, [diamond[0], diamond[3], diamond[2]]);
    assert_eq!(chains.right, [diamond[0], diamond[1], diamond[2]]);
    for count in [3, 4, 7, 40, 161] {
        let points = regular(count, Point::new(10.0, 10.0), 6.0, 0.3, 1.7);
        let chains = convex(&points).unwrap();
        assert_eq!(
            chains.left.len() + chains.right.len(),
            points.len() + 2,
            "{count}"
        );
        for chain in [&chains.left, &chains.right] {
            assert!(chain.windows(2).all(|w| w[0].y < w[1].y));
        }
        let middle = chains.top.midpoint(chains.bottom);
        let x = |chain: &[Point]| {
            let k = segment(chain, middle);
            Line {
                a: chain[k - 1],
                b: chain[k],
            }
            .x(middle)
        };
        assert!(x(&chains.left) < x(&chains.right));
    }
}

#[test]
fn only_one_certified_convex_contour_has_chains() {
    let mut repeated = square();
    repeated.insert(1, repeated[0]);
    repeated.push(repeated[0]);
    assert!(convex(&repeated).is_some());
    let mut concave = square();
    concave.insert(2, Point::new(4.0, 4.0));
    assert!(convex(&concave).is_none());
    // A point on a side is not certified as a turn.
    let mut subdivided = square();
    subdivided.insert(1, Point::new(3.0, 2.0));
    assert!(convex(&subdivided).is_none());
    let twice: Vec<_> = regular(5, Point::new(5.0, 5.0), 3.0, 0.0, 1.0)
        .into_iter()
        .cycle()
        .step_by(2)
        .take(5)
        .collect();
    assert!(convex(&twice).is_none());
    assert!(convex(&square()[..2]).is_none());
    let mut two = polygon(&[square(), square()].concat());
    two.starts.push(4);
    assert!(
        Convex::new(&two, &mut crate::testing::bytes())
            .unwrap()
            .is_none()
    );
}

#[test]
fn only_certified_interior_points_are_contained() {
    let chains = convex(&square()).unwrap();
    assert!(chains.contains(Point::new(3.0, 4.0)));
    assert!(chains.contains(Point::new(1.0 + 1e-9, 7.0 - 1e-9)));
    for p in [
        Point::new(1.0, 4.0),
        Point::new(3.0, 2.0),
        Point::new(5.0, 7.0),
        Point::new(0.5, 4.0),
        Point::new(3.0, 7.5),
    ] {
        assert!(!chains.contains(p), "{p:?}");
    }
    let diamond = convex(&[
        Point::new(3.0, 0.0),
        Point::new(6.0, 4.0),
        Point::new(3.0, 9.0),
        Point::new(0.0, 4.0),
    ])
    .unwrap();
    assert!(diamond.contains(Point::new(4.4, 2.0)));
    assert!(!diamond.contains(Point::new(4.6, 2.0)));
    assert!(!diamond.contains(Point::new(4.5, 2.0)));
}
