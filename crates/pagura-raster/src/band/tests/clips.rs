//! Clipped marks against the old exact backends (`ClipRegion`, `Mask`).

use super::*;
use crate::ClipRegion;

type Paths<'a> = &'a [(&'a Path, FillRule)];

/// The old coverage of `path` clipped by `clips`, or of the clips alone.
fn old_clipped(area: &Area, mark: Option<(&Path, FillRule)>, clips: Paths<'_>) -> Vec<u8> {
    let r = area.region;
    let limits = Limits::default();
    let (w, h) = (r.width(), r.height());
    let regions: Vec<ClipRegion> = clips
        .iter()
        .map(|&(p, rule)| {
            ClipRegion::from_path(w, h, p, Transform::IDENTITY, rule, limits, (r.x0, r.y0)).unwrap()
        })
        .collect();
    let mut out = vec![0; (w * h) as usize];
    match mark {
        Some((path, rule)) => {
            let mut region = regions[0].clone();
            for other in &regions[1..] {
                region.intersect(other).unwrap();
            }
            if let Some(mask) = region.clipped_path(path, Transform::IDENTITY, rule, limits).unwrap() {
                let (x0, y0) = mask.origin();
                for (y, row) in mask.coverage().chunks(mask.width() as usize).enumerate() {
                    let start = (y0 - r.y0) as usize * w as usize + y * w as usize + (x0 - r.x0) as usize;
                    out[start..start + row.len()].copy_from_slice(row);
                }
            }
        }
        None => {
            let mut mask = Mask::from_path_with_limits(
                w,
                h,
                clips[0].0,
                Transform::IDENTITY,
                clips[0].1,
                limits,
                (r.x0, r.y0),
            )
            .unwrap();
            for &(p, rule) in &clips[1..] {
                let other =
                    Mask::from_path_with_limits(w, h, p, Transform::IDENTITY, rule, limits, (r.x0, r.y0))
                        .unwrap();
                mask.intersect(&other).unwrap();
            }
            out.copy_from_slice(mask.coverage());
        }
    }
    out
}

fn check(area: &Area, mark: Option<(&Path, FillRule)>, clips: Paths<'_>) -> (usize, u8) {
    let clip = |options: &ShapeOptions<'_>| ClipShape::new(clips, Transform::IDENTITY, options).unwrap();
    let new = match mark {
        Some((path, rule)) => invariant(
            area,
            &|options| Shape::fill_in(path, Transform::IDENTITY, rule, options).unwrap(),
            Some(&clip),
        ),
        None => {
            let limits = Limits::default();
            let clip_shape = clip(&public(*area, &limits, Precision::Exact));
            let (coverage, work) = raster(area, None, Some(&clip_shape));
            assert!(work <= clip_shape.work());
            coverage
        }
    };
    let old = old_clipped(area, mark, clips);
    compare(&new, &old)
}

fn random_polygon(rng: &mut Lcg, count: usize, steps: u32) -> Path {
    let points: Vec<_> = (0..count)
        .map(|_| Point::new(rng.grid(-1.0, 25.0, steps), rng.grid(-1.0, 21.0, steps)))
        .collect();
    polygon(&points)
}

fn rectangle(rng: &mut Lcg, steps: u32) -> Path {
    let mut path = PathBuilder::new();
    path.rect(
        rng.grid(-1.0, 14.0, steps),
        rng.grid(-1.0, 12.0, steps),
        rng.grid(2.0, 14.0, steps),
        rng.grid(2.0, 12.0, steps),
    );
    path.finish()
}

#[test]
fn marks_within_rectangles_and_paths_match_the_old_backends() {
    let mut rng = Lcg(808);
    let area = area(0, 0, 24, 20, 16);
    let mut totals = [(0, 0); 5];
    for case in 0..40 {
        let steps = if case % 2 == 0 { 256 } else { 10 };
        let mark = random_polygon(&mut rng, 3 + case % 6, steps);
        let rect_a = rectangle(&mut rng, steps);
        let rect_b = rectangle(&mut rng, steps);
        let clip_a = random_polygon(&mut rng, 3 + case % 4, steps);
        let clip_b = random_polygon(&mut rng, 4, steps);
        let rect_mark = rectangle(&mut rng, steps);
        let cases: [(Path, Vec<(&Path, FillRule)>); 5] = [
            (mark.clone(), vec![(&rect_a, FillRule::NonZero)]),
            (
                mark.clone(),
                vec![(&rect_a, FillRule::NonZero), (&rect_b, FillRule::EvenOdd)],
            ),
            (mark.clone(), vec![(&clip_a, FillRule::NonZero)]),
            (
                mark.clone(),
                vec![(&rect_a, FillRule::NonZero), (&clip_a, FillRule::EvenOdd), (&clip_b, FillRule::NonZero)],
            ),
            (rect_mark, vec![(&clip_a, FillRule::NonZero)]),
        ];
        for (kind, (mark, clips)) in cases.iter().enumerate() {
            for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                let (count, largest) = check(&area, Some((mark, rule)), clips);
                assert!(largest <= 1, "case {case} kind {kind} {rule:?}: {count} by {largest}");
                totals[kind].0 += count;
                totals[kind].1 += 480;
            }
        }
    }
    eprintln!("clipped marks (rect, two rects, path, rect and paths, rect mark): {totals:?}");
}

#[test]
fn clips_alone_cover_what_their_masks_do() {
    let mut rng = Lcg(77);
    let area = area(0, 0, 24, 20, 8);
    for case in 0..30 {
        let steps = if case % 2 == 0 { 256 } else { 10 };
        let rect = rectangle(&mut rng, steps);
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
fn containing_and_disjoint_clips() {
    let area = area(0, 0, 16, 16, 4);
    let mark = polygon(&[Point::new(5.2, 5.1), Point::new(9.7, 6.3), Point::new(6.1, 9.9)]);
    let around = polygon(&[Point::new(0.5, 0.5), Point::new(15.5, 1.0), Point::new(8.0, 15.5)]);
    let (count, _) = check(&area, Some((&mark, FillRule::NonZero)), &[(&around, FillRule::NonZero)]);
    assert_eq!(count, 0);
    let mut away = PathBuilder::new();
    away.rect(12.0, 12.0, 3.0, 3.0);
    let away = away.finish();
    let limits = Limits::default();
    let clip = ClipShape::new(&[(&away, FillRule::NonZero)], Transform::IDENTITY, &public(area, &limits, Precision::Exact))
        .unwrap();
    let shape = Shape::fill_in(
        &mark,
        Transform::IDENTITY,
        FillRule::NonZero,
        &options(area, &limits, Some(&clip)),
    )
    .unwrap();
    assert!(shape.is_none());
}
