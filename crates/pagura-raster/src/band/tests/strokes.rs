//! Strokes against the old exact backends (`stroke_path_at` fills the
//! component outline that `stroke_outline` returns).

use super::*;
use crate::{LineCap, LineJoin, Stroke};

/// A stroke class's differences from the old backends: (strokes, pixels
/// compared, pixels differing, largest difference).
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    strokes: usize,
    pixels: usize,
    differing: usize,
    largest: u8,
}

fn check(
    area: &Area,
    path: &Path,
    stroke: &Stroke,
    (pen, transform): (Transform, Transform),
    (adjustment, partitions): (Adjustment, bool),
    tally: &mut Tally,
) {
    let prepare = |options: &Prep<'_>| {
        Shape::stroke_in(
            path,
            stroke,
            (pen.inverse().unwrap(), transform),
            adjustment,
            options,
        )
        .unwrap()
    };
    let new = if partitions {
        invariant(area, &prepare, None)
    } else {
        let limits = Limits::default();
        prepare(&options(*area, &limits, None)).map_or_else(
            || vec![0; (area.region.width() * area.region.height()) as usize],
            |s| raster(area, Some(&s), None).0,
        )
    };
    let user = path.transformed(pen.inverse().unwrap());
    let outline = match adjustment {
        Adjustment::None => {
            crate::stroke_outline(&user, pen.then(transform), stroke, &Limits::default())
        }
        Adjustment::Adjusted => {
            crate::adjusted_stroke_outline(&user, pen.then(transform), stroke, &Limits::default())
        }
    }
    .unwrap();
    let old = old_fill(area, &outline.0, outline.1, FillRule::NonZero);
    let (count, largest) = compare(&new, &old);
    tally.strokes += 1;
    tally.pixels += new.len();
    tally.differing += count;
    tally.largest = tally.largest.max(largest);
}

fn paths(rng: &mut Lcg) -> Vec<Path> {
    let mut out = Vec::new();
    // Two-segment polylines, as most marks of dense vector pages are.
    for _ in 0..6 {
        let p = |rng: &mut Lcg| Point::new(rng.grid(4.0, 36.0, 10), rng.grid(4.0, 28.0, 10));
        let mut path = PathBuilder::new();
        path.move_to(p(rng));
        path.line_to(p(rng)).unwrap();
        path.line_to(p(rng)).unwrap();
        out.push(path.finish());
    }
    // Axis-aligned rules, a closed square, a curve, a single point.
    let mut path = PathBuilder::new();
    path.move_to(Point::new(3.0, 10.3));
    path.line_to(Point::new(37.0, 10.3)).unwrap();
    path.move_to(Point::new(20.5, 2.0));
    path.line_to(Point::new(20.5, 30.0)).unwrap();
    out.push(path.finish());
    let mut path = PathBuilder::new();
    path.rect(8.25, 6.5, 20.0, 15.0);
    out.push(path.finish());
    let mut path = PathBuilder::new();
    path.move_to(Point::new(5.0, 25.0));
    path.cubic_to(
        Point::new(12.0, 2.0),
        Point::new(30.0, 40.0),
        Point::new(36.0, 8.0),
    )
    .unwrap();
    out.push(path.finish());
    let mut path = PathBuilder::new();
    path.move_to(Point::new(17.5, 17.5));
    path.close();
    out.push(path.finish());
    // Runs whose directions span more than a half turn: a letter-like
    // hook, a closed non-convex outline, a curve that turns back near
    // itself and a zigzag.
    let polyline = |points: &[(f64, f64)], closed: bool| {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(points[0].0, points[0].1));
        for &(x, y) in &points[1..] {
            path.line_to(Point::new(x, y)).unwrap();
        }
        if closed {
            path.close();
        }
        path.finish()
    };
    out.push(polyline(
        &[
            (6.0, 6.0),
            (6.0, 20.0),
            (8.1, 24.2),
            (12.0, 25.3),
            (16.2, 24.1),
            (18.0, 20.0),
            (18.0, 6.5),
        ],
        false,
    ));
    out.push(polyline(
        &[
            (24.0, 4.0),
            (38.0, 4.5),
            (38.0, 12.0),
            (30.2, 12.3),
            (30.0, 26.0),
            (24.3, 26.0),
        ],
        true,
    ));
    let mut path = PathBuilder::new();
    path.move_to(Point::new(8.0, 30.0));
    path.cubic_to(
        Point::new(40.0, 34.0),
        Point::new(36.0, 4.0),
        Point::new(20.0, 14.0),
    )
    .unwrap();
    path.cubic_to(
        Point::new(10.0, 20.0),
        Point::new(16.0, 31.0),
        Point::new(26.5, 27.5),
    )
    .unwrap();
    out.push(path.finish());
    out.push(polyline(
        &[
            (4.0, 8.0),
            (10.0, 16.0),
            (16.0, 8.5),
            (22.0, 16.5),
            (28.0, 9.0),
            (34.0, 17.0),
            (20.0, 30.0),
            (5.0, 24.0),
        ],
        false,
    ));
    out
}

#[test]
fn strokes_of_every_class_match_the_old_backends() {
    let mut rng = Lcg(5150);
    let area = area(0, 0, 44, 36, 16);
    let paths = paths(&mut rng);
    let transforms = [
        (Transform::IDENTITY, Transform::IDENTITY),
        (Transform::scale(0.5, 0.5), Transform::scale(2.0, 2.0)),
        (
            Transform::new(1.0, 0.0, 0.3, 0.8, 0.0, 0.0),
            Transform::IDENTITY,
        ),
    ];
    let mut tallies = std::collections::BTreeMap::new();
    for cap in [LineCap::Butt, LineCap::ProjectingSquare, LineCap::Round] {
        for join in [LineJoin::Miter, LineJoin::Bevel, LineJoin::Round] {
            for width in [0.0, 0.6, 1.0, 3.7] {
                for dashes in [vec![], vec![2.0, 1.5]] {
                    let stroke = Stroke {
                        width,
                        cap,
                        join,
                        miter_limit: if width > 1.0 { 2.0 } else { 10.0 },
                        dash_array: dashes.clone(),
                        dash_phase: 0.75,
                    };
                    for (t, &transform) in transforms.iter().enumerate() {
                        for path in &paths {
                            for adjustment in [Adjustment::None, Adjustment::Adjusted] {
                                let class = (
                                    format!("{cap:?}/{join:?}"),
                                    adjustment == Adjustment::Adjusted,
                                );
                                let tally: &mut Tally = tallies.entry(class).or_default();
                                let partitions = t == 0 && width != 0.6;
                                check(
                                    &area,
                                    path,
                                    &stroke,
                                    transform,
                                    (adjustment, partitions),
                                    tally,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    for ((class, adjusted), tally) in &tallies {
        eprintln!("strokes {class} adjusted={adjusted}: {tally:?}");
        assert!(tally.largest <= 2, "{class}: {tally:?}");
        if !class.contains("Round") || *adjusted {
            assert!(tally.largest <= 1, "{class}: {tally:?}");
        }
    }
}

#[test]
fn singular_pens_are_unsupported() {
    let limits = Limits::default();
    let area = area(0, 0, 8, 8, 4);
    let path = polygon(&[
        Point::new(1.0, 1.0),
        Point::new(5.0, 5.0),
        Point::new(1.0, 5.0),
    ]);
    let error = Shape::stroke(
        &path,
        &Stroke::default(),
        Transform::scale(0.0, 1.0),
        Transform::IDENTITY,
        Adjustment::None,
        &public(area, &limits, Precision::Exact),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        pagura_core::ErrorKind::Unsupported(crate::features::SINGULAR_PEN)
    );
}

/// A stroke whose union needs resolved cells keeps them when an
/// approximate clip is multiplied in: the Comment note icon's frame and
/// lines within its form's box.
#[test]
fn multiplied_clips_keep_resolved_cells() {
    let limits = Limits::default();
    let area = area(0, 0, 8, 8, 8);
    let path = {
        let mut b = PathBuilder::new();
        b.move_to(Point::new(0.5, 0.5));
        for (x, y) in [(7.5, 0.5), (7.5, 7.5), (0.5, 7.5)] {
            b.line_to(Point::new(x, y)).unwrap();
        }
        b.close();
        for line in [
            [(2.0, 5.5), (6.0, 5.5)],
            [(2.0, 4.0), (6.0, 4.0)],
            [(2.0, 2.5), (4.5, 2.5)],
            [(2.0, 0.5), (1.0, 0.0)],
        ] {
            b.move_to(Point::new(line[0].0, line[0].1));
            b.line_to(Point::new(line[1].0, line[1].1)).unwrap();
        }
        b.finish()
    };
    let pen = Transform::scale(2.0, 2.0);
    let device = Transform::new(1.0, 0.0, 0.0, -1.0, 0.0, 8.0);
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    let mut bbox = PathBuilder::new();
    bbox.rect(0.0, 0.0, 8.0, 8.0);
    let bbox = bbox.finish();
    let coverage = |precision| {
        let clip = ClipShape::new(
            &[(&bbox, FillRule::NonZero)],
            device,
            &public(area, &limits, precision),
        )
        .unwrap();
        let o = Prep {
            area,
            precision,
            limits: &limits,
            clip: Some(&clip),
        };
        let s = Shape::stroke_in(&path, &stroke, (pen, device), Adjustment::None, &o)
            .unwrap()
            .unwrap();
        raster(&area, Some(&s), Some(&clip)).0
    };
    assert_eq!(coverage(Precision::Approximate), coverage(Precision::Exact));
}
