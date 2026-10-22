//! Approximate cells against the old approximate cells and clip product
//! (the Competitive rasterizer).

use super::*;
use crate::{ClipRegion, Coverage, Rasterizer, Stroke};
use std::sync::Arc;

/// Rows of `(y, x, levels)` from a coverage's origin.
type Spans = Vec<(usize, usize, Vec<u8>)>;

/// The old Competitive coverage of a fill over `area`, within `clips`.
fn old(
    area: &Area,
    path: &Path,
    transform: Transform,
    rule: FillRule,
    clips: &[(&Path, FillRule)],
) -> Vec<u8> {
    let r = area.region;
    let (w, h) = (r.width(), r.height());
    let limits = Limits::default();
    let mut whole = PathBuilder::new();
    whole.rect(
        f64::from(r.x0) - 1.0,
        f64::from(r.y0) - 1.0,
        f64::from(w) + 2.0,
        f64::from(h) + 2.0,
    );
    let whole = whole.finish();
    let mut region = ClipRegion::from_path(
        w,
        h,
        &whole,
        Transform::IDENTITY,
        FillRule::NonZero,
        limits,
        (r.x0, r.y0),
    )
    .unwrap();
    for &(p, rule) in clips {
        region
            .intersect(
                &ClipRegion::from_path(w, h, p, Transform::IDENTITY, rule, limits, (r.x0, r.y0))
                    .unwrap(),
            )
            .unwrap();
    }
    let mut raster = Rasterizer::with_approximate_clip_product(1.0 / 64.0)
        .unwrap()
        .with_geometric_compounds();
    raster.prepare_clip(&mut region).unwrap();
    let mut out = vec![0; (w * h) as usize];
    if let Some(coverage) = raster
        .clipped_coverage(&region, Arc::new(path.clone()), transform, rule, limits)
        .unwrap()
    {
        let (spans, origin): (Spans, (u32, u32)) = match &coverage {
            Coverage::Mask(mask) => (
                mask.spans().map(|(y, x, v)| (y, x, v.to_vec())).collect(),
                mask.origin(),
            ),
            Coverage::Rectangle(rect) => (
                rect.spans().map(|(y, x, v)| (y, x, v.to_vec())).collect(),
                rect.origin(),
            ),
        };
        for (y, x, values) in spans {
            let start = (origin.1 - r.y0) as usize * w as usize
                + y * w as usize
                + (origin.0 - r.x0) as usize
                + x;
            out[start..start + values.len()].copy_from_slice(&values);
        }
    }
    out
}

fn approximate<'a>(area: Area, limits: &'a Limits, clip: Option<&'a ClipShape>) -> Prep<'a> {
    Prep {
        precision: Precision::Approximate,
        ..options(area, limits, clip)
    }
}

/// The new approximate coverage, checked for invariance.
fn new(
    area: &Area,
    prepare: &dyn Fn(&Prep<'_>) -> Option<Shape>,
    clips: &[(&Path, FillRule)],
) -> Vec<u8> {
    let clip = |o: &ShapeOptions<'_>| ClipShape::new(clips, Transform::IDENTITY, o).unwrap();
    invariant_with(
        area,
        &|o| {
            prepare(&Prep {
                precision: Precision::Approximate,
                ..*o
            })
        },
        (!clips.is_empty()).then_some(&clip as &dyn Fn(&ShapeOptions<'_>) -> ClipShape),
        Precision::Approximate,
    )
}

#[test]
fn fills_strokes_and_glyph_polygons_match_the_old_cells() {
    let mut rng = Lcg(65);
    let area = area(0, 0, 40, 32, 16);
    let (mut differing, mut exact, mut strokes, mut pixels) = (0, 0, 0, 0);
    for case in 0..80 {
        let mut path = PathBuilder::new();
        for _ in 0..1 + case % 2 {
            path.move_to(Point::new(rng.grid(0.0, 40.0, 10), rng.grid(0.0, 32.0, 10)));
            for _ in 0..2 + case % 5 {
                if case % 3 == 0 {
                    let p = |rng: &mut Lcg| {
                        Point::new(rng.grid(0.0, 40.0, 10), rng.grid(0.0, 32.0, 10))
                    };
                    let (a, b, c) = (p(&mut rng), p(&mut rng), p(&mut rng));
                    path.cubic_to(a, b, c).unwrap();
                } else {
                    path.line_to(Point::new(rng.grid(0.0, 40.0, 10), rng.grid(0.0, 32.0, 10)))
                        .unwrap();
                }
            }
            path.close();
        }
        let mut path = path.finish();
        if case % 4 == 1 {
            let mut work = 0;
            path.classify_fill(FillRule::NonZero, &Limits::default(), &mut work)
                .unwrap();
            path.classify_fill(FillRule::EvenOdd, &Limits::default(), &mut work)
                .unwrap();
        }
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let new = new(
                &area,
                &|o| Shape::fill_in(&path, Transform::IDENTITY, rule, o).unwrap(),
                &[],
            );
            let old = old(&area, &path, Transform::IDENTITY, rule, &[]);
            let (count, largest) = compare(&new, &old);
            assert!(largest <= 1, "case {case} {rule:?}: {count} by {largest}");
            // Unclassified compound paths keep exact coverage, whose ties
            // the old snapped backend rounded down.
            let compound = case % 2 == 1 && case % 4 != 1;
            if compound {
                exact += count;
            } else {
                differing += count;
            }
            pixels += new.len();
        }
        // The same path stroked against the Competitive renderer's tiled
        // pieces or components: round caps and joins are now flattened at
        // the cells' tolerance and pieces not proven to tile take their
        // union's boundary, within the fast parameter's three levels.
        let stroke = Stroke {
            width: 0.4 + f64::from(case % 5),
            ..Stroke::default()
        };
        let new = new(
            &area,
            &|o| {
                Shape::stroke_in(
                    &path,
                    &stroke,
                    (Transform::IDENTITY, Transform::IDENTITY),
                    Adjustment::None,
                    o,
                )
                .unwrap()
            },
            &[],
        );
        let (outline, t) =
            crate::tiled_stroke_outline(&path, Transform::IDENTITY, &stroke, &Limits::default())
                .unwrap();
        let old = old(&area, &outline, t, FillRule::NonZero, &[]);
        let (count, largest) = compare(&new, &old);
        assert!(largest <= 3, "stroke {case}: {count} by {largest}");
        strokes += count;
        pixels += new.len();
    }
    eprintln!(
        "approximate fills: {differing} of {pixels} differ; exact compounds {exact}; strokes {strokes}"
    );
    assert_eq!(differing, 0);
}

#[test]
fn clips_multiply_or_integrate_as_the_old_product() {
    let mut rng = Lcg(91);
    let area = area(0, 0, 32, 28, 8);
    let (mut differing, mut pixels) = (0, 0);
    for case in 0..60 {
        let tri = |rng: &mut Lcg| {
            polygon(&[
                Point::new(rng.grid(0.0, 32.0, 10), rng.grid(0.0, 28.0, 10)),
                Point::new(rng.grid(0.0, 32.0, 10), rng.grid(0.0, 28.0, 10)),
                Point::new(rng.grid(0.0, 32.0, 10), rng.grid(0.0, 28.0, 10)),
            ])
        };
        let quad = |rng: &mut Lcg| {
            polygon(&[
                Point::new(rng.grid(0.0, 10.0, 10), rng.grid(0.0, 10.0, 10)),
                Point::new(rng.grid(20.0, 32.0, 10), rng.grid(0.0, 10.0, 10)),
                Point::new(rng.grid(20.0, 32.0, 10), rng.grid(18.0, 28.0, 10)),
                Point::new(rng.grid(0.0, 10.0, 10), rng.grid(18.0, 28.0, 10)),
            ])
        };
        let mut rect = PathBuilder::new();
        rect.rect(
            rng.grid(0.0, 10.0, 10),
            rng.grid(0.0, 10.0, 10),
            rng.grid(8.0, 22.0, 10),
            rng.grid(8.0, 18.0, 10),
        );
        let rect = rect.finish();
        let (mark_tri, mark_quad) = (tri(&mut rng), quad(&mut rng));
        let (clip_tri, clip_quad) = (tri(&mut rng), quad(&mut rng));
        let cases: [(&Path, Vec<(&Path, FillRule)>); 5] = [
            (&mark_tri, vec![(&rect, FillRule::NonZero)]),
            (&mark_tri, vec![(&clip_tri, FillRule::NonZero)]),
            (&mark_quad, vec![(&clip_quad, FillRule::NonZero)]),
            (
                &mark_quad,
                vec![(&rect, FillRule::NonZero), (&clip_tri, FillRule::EvenOdd)],
            ),
            (&rect, vec![(&clip_tri, FillRule::NonZero)]),
        ];
        for (kind, (mark, clips)) in cases.iter().enumerate() {
            let new = new(
                &area,
                &|o| Shape::fill_in(mark, Transform::IDENTITY, FillRule::NonZero, o).unwrap(),
                clips,
            );
            let old = old(&area, mark, Transform::IDENTITY, FillRule::NonZero, clips);
            let (count, largest) = compare(&new, &old);
            assert!(
                largest <= 1,
                "case {case} kind {kind}: {count} by {largest}"
            );
            differing += count;
            pixels += new.len();
        }
    }
    eprintln!("approximate clipped marks: {differing} of {pixels} differ");
}

#[test]
fn approximate_rows_fold_without_resolving_overlaps() {
    let limits = Limits::default();
    let area = area(0, 0, 8, 8, 4);
    // Two overlapping squares: nonzero folds the winding 2 to full cover.
    let mut path = PathBuilder::new();
    path.rect(1.0, 1.0, 4.0, 4.0);
    path.rect(3.0, 3.0, 4.0, 4.0);
    let path = path.finish();
    let shape = Shape::fill_in(
        &path,
        Transform::IDENTITY,
        FillRule::EvenOdd,
        &approximate(area, &limits, None),
    )
    .unwrap()
    .unwrap();
    let (coverage, work) = raster(&area, Some(&shape), None);
    assert!(work <= shape.work());
    assert_eq!(coverage[3 * 8 + 3], 0);
    assert_eq!(coverage[2 * 8 + 2], 255);
}
