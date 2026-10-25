use super::*;
use crate::band::tests::Lcg;

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

/// The winding number of closed `lines` around `p`, by the signed sides
/// crossing the horizontal ray to its right.
fn winding(lines: impl Iterator<Item = Line>, p: Point) -> i32 {
    lines
        .filter(|l| (l.a.y <= p.y) != (l.b.y <= p.y) && l.x(p.y) > p.x)
        .map(|l| if l.a.y < l.b.y { 1 } else { -1 })
        .sum()
}

/// The distance from `p` to the segment `l`.
fn distance(l: Line, p: Point) -> f64 {
    let d = l.b - l.a;
    let t = ((p - l.a).dot(d) / d.dot(d)).clamp(0.0, 1.0);
    (l.a + d * t - p).length()
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
        let x = |chain: &[Point]| chain_x(chain, segment(chain, middle), middle);
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

#[test]
fn lines_inside_keep_their_end_points_and_outside_parts_follow_the_chains() {
    let chains = convex(&square()).unwrap();
    let mut out = Vec::new();
    let inside = Line {
        a: Point::new(4.5, 6.5),
        b: Point::new(1.5, 2.5),
    };
    chains.clip(inside, &mut out);
    assert_eq!(out, [inside]);
    // From left of the square to its inside: a part on the left side, then
    // the rest of the line from where it enters.
    out.clear();
    chains.clip(
        Line {
            a: Point::new(-1.0, 3.0),
            b: Point::new(3.0, 5.0),
        },
        &mut out,
    );
    assert_eq!(
        out,
        [
            Line {
                a: Point::new(1.0, 3.0),
                b: Point::new(1.0, 4.0),
            },
            Line {
                a: Point::new(1.0, 4.0),
                b: Point::new(3.0, 5.0),
            },
        ]
    );
    // Upward across the top and right of the square: the part above it
    // leaves, the part beside it lies on the right side.
    out.clear();
    chains.clip(
        Line {
            a: Point::new(7.0, 6.0),
            b: Point::new(7.0, 0.0),
        },
        &mut out,
    );
    assert_eq!(
        out,
        [Line {
            a: Point::new(5.0, 6.0),
            b: Point::new(5.0, 2.0),
        }]
    );
}

#[test]
fn clipped_marks_wind_as_before_inside_and_not_at_all_outside() {
    let mut rng = Lcg(17);
    let mut checked = 0;
    for case in 0..300 {
        let count = 3 + rng.next() % 60;
        let centre = Point::new(rng.grid(8.0, 12.0, 64), rng.grid(8.0, 12.0, 64));
        // Every third path has sides horizontal up to rounding.
        let angle = if case % 3 == 0 {
            std::f64::consts::PI / f64::from(count)
        } else {
            rng.grid(0.0, 6.3, 1000)
        };
        let clip = regular(
            count,
            centre,
            rng.grid(2.0, 6.0, 64),
            angle,
            rng.grid(0.3, 3.0, 64),
        );
        let Some(chains) = convex(&clip) else {
            continue;
        };
        let clip_lines = polygon(&clip).sides().collect::<Vec<_>>();
        let corners = 3 + case % 12;
        let mark: Vec<_> = (0..corners)
            .map(|_| Point::new(rng.grid(0.0, 20.0, 256), rng.grid(0.0, 20.0, 256)))
            .collect();
        let mark_lines = polygon(&mark).sides().collect::<Vec<_>>();
        let mut clipped = Vec::new();
        for &line in &mark_lines {
            let before = clipped.len();
            chains.clip(line, &mut clipped);
            assert!(clipped.len() - before <= chains.bound(line));
        }
        assert!(clipped.iter().all(|l| !l.is_horizontal()));
        for _ in 0..200 {
            let p = Point::new(
                rng.grid(0.0, 20.0, 1 << 20) + 1e-7,
                rng.grid(0.0, 20.0, 1 << 20) + 3e-7,
            );
            let near = |lines: &[Line]| lines.iter().any(|&l| distance(l, p) < 1e-6);
            if near(&mark_lines) || near(&clip_lines) {
                continue;
            }
            let inside = winding(clip_lines.iter().copied(), p) != 0;
            let expected = if inside {
                winding(mark_lines.iter().copied(), p)
            } else {
                0
            };
            assert_eq!(
                winding(clipped.iter().copied(), p),
                expected,
                "case {case} at {p:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 40_000, "{checked}");
}

/// A convex octagon on the grid of `rng`: a box with its four corners cut by
/// positive steps, so that marks on the same grid share its points and sides.
fn octagon(rng: &mut Lcg, steps: u32) -> Vec<Point> {
    let (x0, y0) = (rng.grid(0.0, 8.0, steps), rng.grid(0.0, 8.0, steps));
    let (x1, y1) = (
        x0 + rng.grid(6.0, 12.0, steps),
        y0 + rng.grid(6.0, 12.0, steps),
    );
    let mut cut = || rng.grid(1.0, 3.0, steps);
    let c = [cut(), cut(), cut(), cut(), cut(), cut(), cut(), cut()];
    vec![
        Point::new(x0 + c[0], y0),
        Point::new(x1 - c[1], y0),
        Point::new(x1, y0 + c[2]),
        Point::new(x1, y1 - c[3]),
        Point::new(x1 - c[4], y1),
        Point::new(x0 + c[5], y1),
        Point::new(x0, y1 - c[6]),
        Point::new(x0, y0 + c[7]),
    ]
}

#[test]
fn an_edge_along_a_chain_side_follows_the_chain_through_its_points() {
    // A frame side clipped to its mitred trapezoid: the filled rectangle's
    // left side lies on the trapezoid's vertical side.
    let trapezoid = [
        Point::new(9.0, 1.0),
        Point::new(9.0, 21.0),
        Point::new(6.0, 18.0),
        Point::new(6.0, 4.0),
    ];
    let chains = convex(&trapezoid).unwrap();
    let mut out = Vec::new();
    for line in polygon(&[
        Point::new(6.0, 1.0),
        Point::new(9.0, 1.0),
        Point::new(9.0, 21.0),
        Point::new(6.0, 21.0),
    ])
    .sides()
    {
        chains.clip(line, &mut out);
    }
    let area: f64 = out
        .iter()
        .map(|l| (l.a.x + l.b.x) * (l.b.y - l.a.y) / 2.0)
        .sum();
    // The trapezoid's area, 3 by 20 less the two mitred corners.
    assert_eq!(area.abs(), 3.0 * 20.0 - 2.0 * 4.5);
    for y in [2.0, 3.5, 10.0, 19.5] {
        let p = Point::new(8.9, y);
        assert_eq!(winding(out.iter().copied(), p).abs(), 1, "{y}");
    }
    assert_eq!(winding(out.iter().copied(), Point::new(6.1, 2.0)), 0);
}

#[test]
fn marks_sharing_points_and_sides_with_the_clip_wind_correctly() {
    let mut rng = Lcg(29);
    let mut checked = 0;
    for case in 0..400 {
        let steps = [1, 2, 4][case % 3];
        let clip = octagon(&mut rng, steps);
        let chains = convex(&clip).unwrap();
        let clip_lines = polygon(&clip).sides().collect::<Vec<_>>();
        // Marks on the same coarse grid, some made of the clip's own
        // coordinates.
        let xs: Vec<f64> = clip.iter().map(|p| p.x).collect();
        let ys: Vec<f64> = clip.iter().map(|p| p.y).collect();
        let corners = 3 + case % 7;
        let mark: Vec<_> = (0..corners)
            .map(|i| {
                let pick = rng.next() as usize;
                if (case + i) % 2 == 0 {
                    Point::new(xs[pick % 8], ys[(pick / 8) % 8])
                } else {
                    Point::new(rng.grid(0.0, 20.0, steps), rng.grid(0.0, 20.0, steps))
                }
            })
            .collect();
        let mark_lines = polygon(&mark).sides().collect::<Vec<_>>();
        let mut clipped = Vec::new();
        for &line in &mark_lines {
            chains.clip(line, &mut clipped);
        }
        for _ in 0..100 {
            let p = Point::new(
                rng.grid(0.0, 20.0, 1 << 16) + 1.3e-6,
                rng.grid(0.0, 20.0, 1 << 16) + 0.7e-6,
            );
            let near = |lines: &[Line]| lines.iter().any(|&l| distance(l, p) < 1e-7);
            if near(&mark_lines) || near(&clip_lines) {
                continue;
            }
            let inside = winding(clip_lines.iter().copied(), p) != 0;
            let expected = if inside {
                winding(mark_lines.iter().copied(), p)
            } else {
                0
            };
            assert_eq!(
                winding(clipped.iter().copied(), p),
                expected,
                "case {case} at {p:?}: mark {mark:?} clip {clip:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 30_000, "{checked}");
}

#[test]
fn a_crossing_on_a_nearly_flat_side_moves_area_only_within_its_slab() {
    // A hexagon whose top and bottom sides rise by a few rounding units: one
    // unit of height moves the top side by more than a pixel, so a crossing
    // there is placed only to its slab. Each piece stays on its own line.
    let hexagon = [
        Point::new(17.9, 11.2),
        Point::new(15.0, 16.222_947_341_949_74),
        Point::new(9.200_000_000_000_001, 16.222_947_341_949_745),
        Point::new(6.3, 11.2),
        Point::new(9.199_999_999_999_998, 6.177_052_658_050_257),
        Point::new(14.999_999_999_999_996, 6.177_052_658_050_253),
    ];
    let mark = [
        Point::new(5.5, -0.3),
        Point::new(15.7, 13.2),
        Point::new(18.2, 5.7),
        Point::new(7.1, 4.3),
        Point::new(12.9, 0.0),
        Point::new(5.5, 15.9),
    ];
    let chains = convex(&hexagon).unwrap();
    let mut clipped = Vec::new();
    for line in polygon(&mark).sides() {
        chains.clip(line, &mut clipped);
    }
    let clip_lines = polygon(&hexagon).sides().collect::<Vec<_>>();
    let mark_lines = polygon(&mark).sides().collect::<Vec<_>>();
    for i in 0..200 {
        for j in 0..200 {
            let p = Point::new(
                5.0 + 14.0 * (f64::from(i) + 0.37) / 200.0,
                5.0 + 12.0 * (f64::from(j) + 0.61) / 200.0,
            );
            let inside = winding(clip_lines.iter().copied(), p) != 0;
            let expected = if inside {
                winding(mark_lines.iter().copied(), p)
            } else {
                0
            };
            assert_eq!(winding(clipped.iter().copied(), p), expected, "{p:?}");
        }
    }
}
