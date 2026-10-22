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
