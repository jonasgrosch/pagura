//! Fills against the exact covered area.

use crate::support::{Lcg, area, compare, invariant, polygon, reference};
use pagura_core::Limits;
use pagura_raster::{Area, FillRule, Path, PathBuilder, Point, Shape, Transform};
use pagura_reference::exact;

/// The band coverage, checked for invariance, and its differences from the
/// exact coverage.
fn check_fill(area: &Area, path: &Path, transform: Transform, rule: FillRule) -> (usize, u8) {
    let new = invariant(
        area,
        &|options, clip| Shape::fill_with_clip(path, transform, rule, clip, options).unwrap(),
        &[],
    );
    let mark = exact::fill(path, transform, rule, &Limits::default()).unwrap();
    compare(&new, &reference(area.region, mark, &[]))
}

fn random_polygon(rng: &mut Lcg, count: usize, extent: f64, steps: u32) -> Vec<Point> {
    (0..count)
        .map(|_| Point::new(rng.grid(-0.5, extent, steps), rng.grid(-0.5, extent, steps)))
        .collect()
}

#[test]
fn random_self_intersecting_polygons_are_exact() {
    let mut rng = Lcg(192_873);
    let area = area(0, 0, 8, 8, 3);
    let mut differences = 0;
    for case in 0..300 {
        let points = random_polygon(&mut rng, 3 + case % 9, 8.0, 256);
        let path = polygon(&points);
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let (count, largest) = check_fill(&area, &path, Transform::IDENTITY, rule);
            assert!(
                largest <= 1,
                "case {case} {rule:?}: {count} differ by up to {largest}"
            );
            differences += count;
        }
    }
    eprintln!(
        "dyadic polygons: {differences} differing pixels of {}",
        600 * 64
    );
    assert_eq!(differences, 0);
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "page sizes are small and positive"
)]
fn decimal_polygons_under_page_transforms_are_within_one_level() {
    let mut rng = Lcg(7);
    let mut differences = 0;
    let mut pixels = 0;
    for case in 0..120 {
        let scale = [1.0, 150.0 / 72.0, 300.0 / 72.0][case % 3];
        let transform = Transform::new(scale, 0.0, 0.0, -scale, 3.0, 40.0 * scale);
        let area = area(0, 0, (30.0 * scale) as u32, (40.0 * scale) as u32, 16);
        let points = random_polygon(&mut rng, 3 + case % 7, 30.0, 10);
        let path = polygon(&points);
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let (count, largest) = check_fill(&area, &path, transform, rule);
            assert!(
                largest <= 1,
                "case {case} {rule:?}: {count} differ by up to {largest}"
            );
            differences += count;
            pixels += crate::support::pixels(area.region);
        }
    }
    eprintln!("decimal polygons: {differences} differing pixels of {pixels}");
}

#[test]
fn curves_and_compound_paths_are_within_one_level() {
    let mut rng = Lcg(31_337);
    let area = area(0, 0, 40, 30, 16);
    let mut differences = 0;
    let mut pixels = 0;
    for case in 0..60 {
        let mut path = PathBuilder::new();
        for _ in 0..=(case % 3) {
            path.move_to(Point::new(
                rng.grid(0.0, 40.0, 100),
                rng.grid(0.0, 30.0, 100),
            ));
            for _ in 0..=(case % 4) {
                let p = |rng: &mut Lcg| {
                    Point::new(rng.grid(-2.0, 42.0, 100), rng.grid(-2.0, 32.0, 100))
                };
                let (a, b, c) = (p(&mut rng), p(&mut rng), p(&mut rng));
                path.cubic_to(a, b, c).unwrap();
            }
            path.close();
        }
        let path = path.finish();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let (count, largest) = check_fill(&area, &path, Transform::IDENTITY, rule);
            assert!(
                largest <= 1,
                "case {case} {rule:?}: {count} differ by up to {largest}"
            );
            differences += count;
            pixels += 1200;
        }
    }
    eprintln!("curved paths: {differences} differing pixels of {pixels}");
}

/// Hundreds of edges meeting in a cell without crossing, as a fan of
/// overlapping slivers through one point gives (and the shared sides of
/// unions, or repeated points filling their floor cells): the resolver
/// integrates the cell in the few slabs they make, within the default
/// budget.
#[test]
fn crowded_cells_resolve_within_the_default_budget() {
    let area = area(0, 0, 4, 4, 2);
    let apex = Point::new(1.5, 1.5);
    let mut path = PathBuilder::new();
    for i in 0..300 {
        let at = |turn: f64| {
            let angle = std::f64::consts::TAU * turn;
            Point::new(apex.x + 2.5 * angle.cos(), apex.y + 2.5 * angle.sin())
        };
        let turn = f64::from(i) / 300.0;
        path.move_to(apex);
        path.line_to(at(turn)).unwrap();
        path.line_to(at(turn + 0.008)).unwrap();
        path.close();
    }
    let path = path.finish();
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let (count, largest) = check_fill(&area, &path, Transform::IDENTITY, rule);
        assert!(largest <= 1, "{rule:?}: {count} differ by up to {largest}");
    }
}

#[test]
fn rectangles_are_exact() {
    let mut rng = Lcg(99);
    let area = area(0, 0, 24, 20, 3);
    for _ in 0..200 {
        let x = rng.grid(-3.0, 24.0, 10);
        let y = rng.grid(-3.0, 20.0, 10);
        let mut path = PathBuilder::new();
        path.rect(x, y, rng.grid(0.1, 12.0, 20), rng.grid(0.1, 12.0, 20));
        let (count, _) = check_fill(
            &area,
            &path.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
        );
        assert_eq!(count, 0);
    }
}

#[test]
fn tiny_huge_and_distant_geometry_is_within_one_level() {
    let area = area(10, 20, 50, 60, 16);
    let cases: Vec<Vec<Point>> = vec![
        // Sub-pixel slivers and a tiny triangle.
        vec![
            Point::new(12.1, 22.1),
            Point::new(12.15, 22.1),
            Point::new(12.12, 22.17),
        ],
        vec![
            Point::new(30.0, 25.0),
            Point::new(30.000_001, 45.0),
            Point::new(30.0, 45.0),
        ],
        // A huge triangle crossing the whole area.
        vec![
            Point::new(-1e6, -1e6),
            Point::new(1e6, 30.5),
            Point::new(-1e6, 1e6),
        ],
        // A thin diagonal band far beyond both sides.
        vec![
            Point::new(-1e9, -1e9),
            Point::new(1e9, 1e9 + 0.7),
            Point::new(1e9, 1e9 + 3.1),
            Point::new(-1e9, -1e9 + 2.4),
        ],
    ];
    for points in cases {
        let path = polygon(&points);
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let (count, largest) = check_fill(&area, &path, Transform::IDENTITY, rule);
            assert!(
                largest <= 1,
                "{points:?} {rule:?}: {count} differ by up to {largest}"
            );
        }
    }
}

#[test]
fn overlapping_subpaths_resolve_their_union_and_parity() {
    let area = area(0, 0, 32, 32, 16);
    let mut rng = Lcg(4242);
    let mut differences = 0;
    for _ in 0..40 {
        let mut path = PathBuilder::new();
        for _ in 0..6 {
            let (x, y) = (rng.grid(0.0, 28.0, 64), rng.grid(0.0, 28.0, 64));
            let (w, h) = (rng.grid(0.5, 8.0, 64), rng.grid(0.5, 8.0, 64));
            path.move_to(Point::new(x, y));
            path.line_to(Point::new(x + w, y + h * 0.25)).unwrap();
            path.line_to(Point::new(x + w * 0.75, y + h)).unwrap();
            path.close();
        }
        let path = path.finish();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let (count, largest) = check_fill(&area, &path, Transform::IDENTITY, rule);
            assert!(largest <= 1, "{rule:?}: {count} differ by up to {largest}");
            differences += count;
        }
    }
    eprintln!("overlapping subpaths: {differences} differing pixels");
}

#[test]
fn points_and_empty_regions_follow_the_point_and_hairline_rules() {
    let area = area(0, 0, 16, 16, 4);
    // A geometric point encloses its floor cell (8.5.3.3.1).
    let mut path = PathBuilder::new();
    path.move_to(Point::new(3.5, 4.25));
    path.close();
    path.rect(8.0, 8.0, 2.0, 2.0);
    let (count, _) = check_fill(
        &area,
        &path.finish(),
        Transform::IDENTITY,
        FillRule::NonZero,
    );
    assert_eq!(count, 0);
    // A certified empty region paints its boundary as hairlines (10.7.4).
    let mut path = PathBuilder::new();
    path.move_to(Point::new(2.0, 2.0));
    path.line_to(Point::new(12.0, 9.0)).unwrap();
    path.close();
    let mut path = path.finish();
    let mut work = 0;
    path.classify_fill(FillRule::NonZero, &Limits::default(), &mut work)
        .unwrap();
    let (count, largest) = check_fill(&area, &path, Transform::IDENTITY, FillRule::NonZero);
    assert!(largest <= 1, "{count} differ by up to {largest}");
}

#[test]
fn degenerate_overlaps_on_pixel_boundaries_are_within_one_level() {
    // Vertices on pixel corners and edges, coincident and retraced edges,
    // contours within one pixel and touching contours.
    let area = area(0, 0, 12, 12, 4);
    let mut rng = Lcg(2718);
    let mut differences = 0;
    for case in 0..400 {
        let mut path = PathBuilder::new();
        for _ in 0..=(case % 4) {
            let steps = [1, 2, 4][case % 3];
            path.move_to(Point::new(
                rng.grid(0.0, 12.0, steps),
                rng.grid(0.0, 12.0, steps),
            ));
            for _ in 0..2 + case % 5 {
                let reach = if case % 7 == 0 { 1.0 } else { 6.0 };
                let p = Point::new(
                    rng.grid(0.0, 12.0, steps)
                        .min(rng.grid(0.0, reach, steps) + 3.0),
                    rng.grid(0.0, 12.0, steps),
                );
                path.line_to(p).unwrap();
            }
            path.close();
        }
        let path = path.finish();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let (count, largest) = check_fill(&area, &path, Transform::IDENTITY, rule);
            assert!(
                largest <= 1,
                "case {case} {rule:?}: {count} differ by up to {largest}: {:?}",
                path.segments()
            );
            differences += count;
        }
    }
    eprintln!("degenerate overlaps: {differences} differing pixels");
}
