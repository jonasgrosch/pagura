//! Fast fill routes against the general polygon route: rectangles and
//! grid-accumulated shapes.

use super::*;

/// Rectangle fills skip the polygon and give its bytes, under axis-keeping
/// transforms and clips.
#[test]
fn rectangle_fills_skip_the_polygon_with_its_bytes() {
    let limits = Limits::default();
    let area = area(0, 0, 40, 32, 8);
    let mut rng = Lcg(31);
    let mut crop = PathBuilder::new();
    crop.rect(1.5, 0.25, 36.0, 30.0);
    let crop = crop.finish();
    let triangle = polygon(&[
        Point::new(2.0, 1.0),
        Point::new(39.0, 6.0),
        Point::new(9.0, 31.0),
    ]);
    let transforms = [
        Transform::IDENTITY,
        Transform::new(1.5, 0.0, 0.0, -1.25, 0.3, 31.0),
        Transform::new(0.0, 1.0, -1.0, 0.0, 39.0, 0.5),
    ];
    for case in 0..200 {
        let mut path = PathBuilder::new();
        path.rect(
            rng.grid(-2.0, 30.0, 64),
            rng.grid(-2.0, 22.0, 64),
            rng.grid(0.0, 14.0, 64),
            rng.grid(0.0, 12.0, 64),
        );
        let path = path.finish();
        let transform = transforms[case % 3];
        let clips: [Option<Vec<(&Path, FillRule)>>; 3] = [
            None,
            Some(vec![(&crop, FillRule::NonZero)]),
            Some(vec![(&triangle, FillRule::NonZero)]),
        ];
        for clips in &clips {
            let clip = clips.as_ref().map(|c| {
                ClipShape::new(
                    c,
                    Transform::IDENTITY,
                    &public(area, &limits, Precision::Exact),
                )
                .unwrap()
            });
            let prep = options(area, &limits, clip.as_ref());
            let fast = Shape::fill_in(&path, transform, FillRule::NonZero, &prep).unwrap();
            let (polygon, rule) =
                clip::fill_polygon(&path, transform, FillRule::NonZero, None, &limits).unwrap();
            let slow = Shape::polygon(polygon, rule, shape::Winding::Check(0.0), &prep).unwrap();
            let bytes = |s: Option<&Shape>| {
                s.map_or_else(
                    || vec![0; 40 * 32],
                    |s| raster(&area, Some(s), clip.as_ref()).0,
                )
            };
            assert_eq!(bytes(fast.as_ref()), bytes(slow.as_ref()), "case {case}");
        }
    }
}

/// Small shapes accumulated edge by edge in the grid give the bytes of the
/// row lists.
#[test]
fn grid_shapes_match_their_row_lists() {
    let limits = Limits::default();
    let area = area(0, 0, 48, 40, 4);
    let mut rng = Lcg(404);
    let mut grids = 0;
    for case in 0..120 {
        let points: Vec<Point> = (0..3 + case % 6)
            .map(|_| Point::new(rng.grid(3.0, 45.0, 64), rng.grid(3.0, 37.0, 64)))
            .collect();
        let scale = rng.grid(0.1, 0.5, 256);
        let centre = Point::new(rng.grid(10.0, 38.0, 16), rng.grid(10.0, 30.0, 16));
        let points: Vec<Point> = points
            .iter()
            .map(|p| {
                Point::new(
                    centre.x + (p.x - 24.0) * scale,
                    centre.y + (p.y - 20.0) * scale,
                )
            })
            .collect();
        let path = polygon(&points);
        let rule = [FillRule::NonZero, FillRule::EvenOdd][case % 2];
        let prep = options(area, &limits, None);
        let Some(mut shape) = Shape::fill_in(&path, Transform::IDENTITY, rule, &prep).unwrap()
        else {
            continue;
        };
        let grid = raster(&area, Some(&shape), None).0;
        let bounds = shape.bounds();
        if let shape::Body::Edges(e) = &mut shape.body
            && e.index.is_none()
        {
            grids += 1;
            let right = 48.0;
            let lines = e.lines.clone();
            e.index = Some(
                edges::RowList::new((bounds.y0, bounds.y1), lines.len(), |i| {
                    let l = lines[i];
                    if l.a.x.min(l.b.x) >= right {
                        return None;
                    }
                    edges::rows(l.a.y.min(l.b.y), l.a.y.max(l.b.y), bounds)
                })
                .unwrap(),
            );
            assert_eq!(raster(&area, Some(&shape), None).0, grid, "case {case}");
        }
    }
    assert!(grids > 40, "{grids}");
}
