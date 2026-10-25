//! Clipped marks and clips alone against the exact area of their
//! intersection.

use crate::support::{
    Clips, Lcg, area, compare, invariant, options, polygon, raster, rectangle, reference,
};
use pagura_core::Limits;
use pagura_raster::{Area, ClipShape, FillRule, Path, Point, Shape, Transform};
use pagura_reference::exact;

/// The differences of a mark's coverage within `clips`, or of the clips
/// alone, from the exact coverage of their intersection.
fn check(area: &Area, mark: Option<(&Path, FillRule)>, clips: Clips<'_>) -> (usize, u8) {
    let limits = Limits::default();
    let Some((path, rule)) = mark else {
        let clip = ClipShape::new(clips, Transform::IDENTITY, &options(*area, &limits)).unwrap();
        let (coverage, work) = raster(area, None, Some(&clip));
        assert!(work <= clip.work());
        let (first, rule) = clips[0];
        let first = exact::fill(first, Transform::IDENTITY, rule, &limits).unwrap();
        return compare(&coverage, &reference(area.region, first, &clips[1..]));
    };
    let new = invariant(
        area,
        &|options, clip| {
            Shape::fill_with_clip(path, Transform::IDENTITY, rule, clip, options).unwrap()
        },
        clips,
    );
    let mark = exact::fill(path, Transform::IDENTITY, rule, &limits).unwrap();
    compare(&new, &reference(area.region, mark, clips))
}

fn random_polygon(rng: &mut Lcg, count: usize, steps: u32) -> Path {
    let points: Vec<_> = (0..count)
        .map(|_| Point::new(rng.grid(-1.0, 25.0, steps), rng.grid(-1.0, 21.0, steps)))
        .collect();
    polygon(&points)
}

fn random_rectangle(rng: &mut Lcg, steps: u32) -> Path {
    rectangle(
        rng.grid(-1.0, 14.0, steps),
        rng.grid(-1.0, 12.0, steps),
        rng.grid(2.0, 14.0, steps),
        rng.grid(2.0, 12.0, steps),
    )
}

#[test]
fn marks_within_rectangles_and_paths_are_within_one_level() {
    let mut rng = Lcg(808);
    let area = area(0, 0, 24, 20, 16);
    let mut totals = [(0, 0); 5];
    for case in 0..40 {
        let steps = if case % 2 == 0 { 256 } else { 10 };
        let mark = random_polygon(&mut rng, 3 + case % 6, steps);
        let rect_a = random_rectangle(&mut rng, steps);
        let rect_b = random_rectangle(&mut rng, steps);
        let clip_a = random_polygon(&mut rng, 3 + case % 4, steps);
        let clip_b = random_polygon(&mut rng, 4, steps);
        let rect_mark = random_rectangle(&mut rng, steps);
        let cases: [(&Path, Vec<(&Path, FillRule)>); 5] = [
            (&mark, vec![(&rect_a, FillRule::NonZero)]),
            (
                &mark,
                vec![(&rect_a, FillRule::NonZero), (&rect_b, FillRule::EvenOdd)],
            ),
            (&mark, vec![(&clip_a, FillRule::NonZero)]),
            (
                &mark,
                vec![
                    (&rect_a, FillRule::NonZero),
                    (&clip_a, FillRule::EvenOdd),
                    (&clip_b, FillRule::NonZero),
                ],
            ),
            (&rect_mark, vec![(&clip_a, FillRule::NonZero)]),
        ];
        for (kind, (mark, clips)) in cases.iter().enumerate() {
            for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                let (count, largest) = check(&area, Some((mark, rule)), clips);
                assert!(
                    largest <= 1,
                    "case {case} kind {kind} {rule:?}: {count} by {largest}"
                );
                totals[kind].0 += count;
                totals[kind].1 += 480;
            }
        }
    }
    eprintln!("clipped marks (rect, two rects, path, rect and paths, rect mark): {totals:?}");
}

#[test]
fn clips_alone_cover_their_intersection() {
    let mut rng = Lcg(77);
    let area = area(0, 0, 24, 20, 8);
    for case in 0..30 {
        let steps = if case % 2 == 0 { 256 } else { 10 };
        let rect = random_rectangle(&mut rng, steps);
        let a = random_polygon(&mut rng, 3 + case % 5, steps);
        let b = random_polygon(&mut rng, 5, steps);
        for clips in [
            vec![(&rect, FillRule::NonZero)],
            vec![(&a, FillRule::EvenOdd)],
            vec![(&a, FillRule::NonZero), (&b, FillRule::NonZero)],
            vec![(&rect, FillRule::NonZero), (&a, FillRule::NonZero)],
        ] {
            let (count, largest) = check(&area, None, &clips);
            assert!(largest <= 1, "case {case}: {count} by {largest}");
        }
    }
}

#[test]
fn a_containing_clip_keeps_the_exact_mark() {
    let area = area(0, 0, 16, 16, 4);
    let mark = polygon(&[
        Point::new(5.2, 5.1),
        Point::new(9.7, 6.3),
        Point::new(6.1, 9.9),
    ]);
    let around = polygon(&[
        Point::new(0.5, 0.5),
        Point::new(15.5, 1.0),
        Point::new(8.0, 15.5),
    ]);
    let (count, _) = check(
        &area,
        Some((&mark, FillRule::NonZero)),
        &[(&around, FillRule::NonZero)],
    );
    assert_eq!(count, 0);
}

/// A regular `count`-gon around `(x, y)`, turned by `angle` and stretched by
/// `scale` along x: a convex clipping path with sides at many slopes.
fn regular(count: u32, (x, y): (f64, f64), radius: f64, angle: f64, scale: f64) -> Path {
    let step = std::f64::consts::TAU / f64::from(count);
    let points: Vec<_> = (0..count)
        .map(|i| {
            let a = angle + f64::from(i) * step;
            Point::new(x + scale * radius * a.cos(), y + radius * a.sin())
        })
        .collect();
    polygon(&points)
}

#[test]
fn marks_within_convex_paths_are_within_one_level() {
    let mut rng = Lcg(4242);
    let area = area(0, 0, 24, 20, 16);
    let mut totals = [(0, 0); 5];
    for case in 0..40 {
        let steps = if case % 2 == 0 { 256 } else { 10 };
        let mark = random_polygon(&mut rng, 3 + case % 6, steps);
        let centre = (rng.grid(6.0, 18.0, steps), rng.grid(6.0, 14.0, steps));
        let round = regular(
            u32::try_from(6 + case % 50).unwrap(),
            centre,
            rng.grid(2.0, 9.0, steps),
            rng.grid(0.0, 6.3, 1000),
            rng.grid(0.4, 2.5, steps),
        );
        // A turned square, and a hexagon with a horizontal top and bottom.
        let turned = regular(4, centre, rng.grid(3.0, 9.0, steps), 0.4, 1.0);
        let flat = regular(6, centre, rng.grid(3.0, 9.0, steps), 0.0, 1.0);
        let triangle = random_polygon(&mut rng, 3, steps);
        let rect = random_rectangle(&mut rng, steps);
        let small = regular(5, centre, 1.5, 0.2, 1.0);
        let cases: [(&Path, Vec<(&Path, FillRule)>); 5] = [
            (&mark, vec![(&round, FillRule::NonZero)]),
            (
                &mark,
                vec![(&turned, FillRule::EvenOdd), (&triangle, FillRule::NonZero)],
            ),
            (
                &mark,
                vec![(&rect, FillRule::NonZero), (&flat, FillRule::NonZero)],
            ),
            // A mark inside the path, and one around it.
            (&small, vec![(&round, FillRule::NonZero)]),
            (&round, vec![(&small, FillRule::NonZero)]),
        ];
        for (kind, (mark, clips)) in cases.iter().enumerate() {
            for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                let (count, largest) = check(&area, Some((mark, rule)), clips);
                assert!(
                    largest <= 1,
                    "case {case} kind {kind} {rule:?}: {count} by {largest}"
                );
                totals[kind].0 += count;
                totals[kind].1 += 480;
            }
        }
    }
    eprintln!("marks in convex paths (round, two, rect and flat, inside, around): {totals:?}");
}

/// A convex octagon on a grid of `1 / steps`: a box with its corners cut.
fn octagon(rng: &mut Lcg, steps: u32) -> Vec<Point> {
    let (x0, y0) = (rng.grid(-1.0, 10.0, steps), rng.grid(-1.0, 8.0, steps));
    let (x1, y1) = (
        x0 + rng.grid(5.0, 14.0, steps),
        y0 + rng.grid(5.0, 12.0, steps),
    );
    let mut cut = || rng.grid(0.5, 3.0, steps).max(1.0 / f64::from(steps));
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
fn marks_sharing_points_and_sides_with_convex_paths_are_within_one_level() {
    let mut rng = Lcg(9090);
    let area = area(0, 0, 24, 20, 16);
    let mut total = (0, 0);
    for case in 0..60 {
        let steps = [1, 2, 4, 10][case % 4];
        let corners = octagon(&mut rng, steps);
        let clip = polygon(&corners);
        // Marks on the same grid, partly through the clip's own points.
        let mark: Vec<_> = (0..3 + case % 6)
            .map(|i| {
                let pick = rng.next() as usize;
                if i % 2 == 0 {
                    Point::new(corners[pick % 8].x, corners[(pick / 8) % 8].y)
                } else {
                    Point::new(rng.grid(-1.0, 25.0, steps), rng.grid(-1.0, 21.0, steps))
                }
            })
            .collect();
        let mark = polygon(&mark);
        let (xs, ys): (Vec<f64>, Vec<f64>) = corners.iter().map(|p| (p.x, p.y)).unzip();
        let span = |v: &[f64]| {
            (
                v.iter().copied().fold(f64::INFINITY, f64::min),
                v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            )
        };
        let ((left, right), (top, bottom)) = (span(&xs), span(&ys));
        // A rectangle on the clip's bounding sides, as a filled frame side
        // within its mitred clip.
        let frame = rectangle(left, top, right - left, bottom - top);
        let rect = random_rectangle(&mut rng, steps);
        for mark in [&mark, &frame, &rect] {
            for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                let (count, largest) =
                    check(&area, Some((mark, rule)), &[(&clip, FillRule::NonZero)]);
                assert!(largest <= 1, "case {case} {rule:?}: {count} by {largest}");
                total.0 += count;
                total.1 += 480;
            }
        }
    }
    eprintln!("marks sharing points with convex paths: {total:?}");
}

#[test]
fn a_frame_side_within_its_mitred_clip_keeps_the_exact_coverage() {
    // A filled frame side clipped to its mitred trapezoid: the rectangle's
    // left side runs along the trapezoid's vertical side.
    let area = area(0, 0, 16, 24, 8);
    let trapezoid = polygon(&[
        Point::new(9.25, 1.5),
        Point::new(9.25, 21.75),
        Point::new(6.3, 18.8),
        Point::new(6.3, 4.45),
    ]);
    let side = rectangle(6.3, 1.5, 2.95, 20.25);
    let (count, largest) = check(
        &area,
        Some((&side, FillRule::NonZero)),
        &[(&trapezoid, FillRule::NonZero)],
    );
    assert_eq!((count, largest), (0, 0));
}

#[test]
fn a_box_under_a_nearly_flat_convex_side_keeps_the_exact_coverage() {
    let area = area(490, 0, 520, 8, 8);
    let square = polygon(&[
        Point::new(0.0, 0.0),
        Point::new(1000.0, 1.0),
        Point::new(999.0, 1001.0),
        Point::new(-1.0, 1000.0),
    ]);
    for (x, y) in [(500.0, -5.0), (500.25, -0.3), (503.7, 0.1)] {
        let mark = rectangle(x, y, 10.0, 6.0);
        let (count, largest) = check(
            &area,
            Some((&mark, FillRule::NonZero)),
            &[(&square, FillRule::NonZero)],
        );
        assert_eq!((count, largest), (0, 0), "{x} {y}");
    }
}
