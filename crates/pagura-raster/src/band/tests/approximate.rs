//! Approximate coverage (the `approximate_coverage` fast parameter, not
//! within the one-level contract): the same bytes and charges at every band
//! height and on sub-regions, simple polygons and strokes within a few
//! levels of exact, overlaps folded without resolution, and nothing painted
//! where the mark or a clipping path covers nothing.

use super::*;
use crate::Stroke;

fn approximate<'a>(area: Area, limits: &'a Limits, clip: Option<&'a ClipShape>) -> Prep<'a> {
    Prep {
        precision: Precision::Approximate,
        ..options(area, limits, clip)
    }
}

/// The coverage of the shape `prepare` gives at `precision` within `clips`,
/// checked for invariance.
fn coverage(
    area: &Area,
    prepare: &dyn Fn(&Prep<'_>) -> Option<Shape>,
    clips: &[(&Path, FillRule)],
    precision: Precision,
) -> Vec<u8> {
    let clip = |o: &ShapeOptions<'_>| ClipShape::new(clips, Transform::IDENTITY, o).unwrap();
    invariant_with(
        area,
        &|o| prepare(&Prep { precision, ..*o }),
        (!clips.is_empty()).then_some(&clip as &dyn Fn(&ShapeOptions<'_>) -> ClipShape),
        precision,
    )
}

/// Approximate and exact coverage of the shape `prepare` gives.
fn both(
    area: &Area,
    prepare: &dyn Fn(&Prep<'_>) -> Option<Shape>,
    clips: &[(&Path, FillRule)],
) -> (Vec<u8>, Vec<u8>) {
    (
        coverage(area, prepare, clips, Precision::Approximate),
        coverage(area, prepare, clips, Precision::Exact),
    )
}

/// Fills of any path and strokes are invariant; triangles take f32 cells
/// within one level of exact, and strokes, whose round parts are flattened
/// at the cells' tolerance and whose pieces not proven to tile take their
/// union's boundary, within three.
#[test]
fn fills_and_strokes_are_invariant_and_simple_shapes_near_exact() {
    let mut rng = Lcg(65);
    let area = area(0, 0, 40, 32, 16);
    let (mut fills, mut triangles, mut strokes) = ((0, 0), (0, 0), (0, 0));
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
            let (approximate, exact) = both(
                &area,
                &|o| Shape::fill_in(&path, Transform::IDENTITY, rule, o).unwrap(),
                &[],
            );
            let (count, largest) = compare(&approximate, &exact);
            fills = (fills.0 + count, fills.1.max(largest));
        }
        let triangle = polygon(&[
            Point::new(rng.grid(0.0, 40.0, 10), rng.grid(0.0, 32.0, 10)),
            Point::new(rng.grid(0.0, 40.0, 10), rng.grid(0.0, 32.0, 10)),
            Point::new(rng.grid(0.0, 40.0, 10), rng.grid(0.0, 32.0, 10)),
        ]);
        let (approximate, exact) = both(
            &area,
            &|o| Shape::fill_in(&triangle, Transform::IDENTITY, FillRule::NonZero, o).unwrap(),
            &[],
        );
        let (count, largest) = compare(&approximate, &exact);
        assert!(largest <= 1, "triangle {case}: {count} by {largest}");
        triangles = (triangles.0 + count, triangles.1.max(largest));
        let stroke = Stroke {
            width: 0.4 + f64::from(case % 5),
            ..Stroke::default()
        };
        let (approximate, exact) = both(
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
        let (count, largest) = compare(&approximate, &exact);
        assert!(largest <= 3, "stroke {case}: {count} by {largest}");
        strokes = (strokes.0 + count, strokes.1.max(largest));
    }
    eprintln!(
        "approximate against exact (pixels, largest): fills {fills:?}, triangles {triangles:?}, \
         strokes {strokes:?}"
    );
}

/// Clips are multiplied in as their own coverage (or integrated, a
/// quadrilateral around a quadrilateral), so a pixel is painted only where
/// the mark and every clipping path cover some of it.
#[test]
fn clipped_marks_are_invariant_and_stay_within_every_clip() {
    let mut rng = Lcg(91);
    let limits = Limits::default();
    let area = area(0, 0, 32, 28, 8);
    let (mut differing, mut pixels) = (0, 0);
    let exact = |clips: &[(&Path, FillRule)]| {
        let clip = ClipShape::new(
            clips,
            Transform::IDENTITY,
            &public(area, &limits, Precision::Exact),
        )
        .unwrap();
        raster(&area, None, Some(&clip)).0
    };
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
            let fill = |o: &Prep<'_>| {
                Shape::fill_in(mark, Transform::IDENTITY, FillRule::NonZero, o).unwrap()
            };
            let (clipped, joint) = both(&area, &fill, clips);
            let (_, alone) = both(&area, &fill, &[]);
            let each: Vec<Vec<u8>> = clips.iter().map(|&clip| exact(&[clip])).collect();
            for (i, &value) in clipped.iter().enumerate() {
                if alone[i] == 0 || each.iter().any(|c| c[i] == 0) {
                    assert_eq!(value, 0, "case {case} kind {kind}: outside at {i}");
                }
            }
            differing += compare(&clipped, &joint).0;
            pixels += clipped.len();
        }
    }
    eprintln!("approximate clipped marks: {differing} of {pixels} pixels differ from exact");
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
