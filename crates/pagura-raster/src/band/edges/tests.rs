use super::*;
use crate::PathBuilder;

#[test]
fn colliding_relink_keys_use_bounded_lookup_and_release_the_table() {
    let sides: Vec<_> = (0..1024_u32)
        .map(|i| ((i64::from(i) << 32, i64::from(i)), i as usize))
        .collect();
    assert!(sides.iter().all(|&(key, _)| slot(key) == 0));
    let mut bytes = crate::testing::bytes();
    bytes.charge(7).unwrap();
    let firsts = Firsts::new(&sides, &mut bytes).unwrap();
    assert!(firsts.places.is_empty());
    assert_eq!(bytes.used(), 7);
    for (i, &(key, _)) in sides.iter().enumerate() {
        assert_eq!(firsts.get(&sides, key), i);
    }
    for key in [(-1, 0), (1, 0), (i64::MAX, i64::MAX)] {
        assert_eq!(firsts.get(&sides, key), sides.len());
    }
}

#[test]
fn many_sides_at_one_vertex_keep_the_previous_link_order() {
    for count in [8, 64, 257] {
        let origin = (0_i64, 0_i64);
        let mut keys = vec![origin; count];
        keys.extend((1..=count).rev().map(|i| (i64::try_from(i).unwrap(), 0)));
        let next: Vec<_> = (count..2 * count).collect();
        let points = vec![Point::default(); count];
        let mut kept: Vec<_> = (0..count).map(|i| (origin, i)).collect();
        let mut previous: Vec<_> = kept
            .iter()
            .map(|&(from, a)| {
                let to = keys[next[a]];
                Keyed {
                    lo: from.min(to),
                    hi: from.max(to),
                    forward: from <= to,
                    from,
                    to,
                    a: points[a],
                }
            })
            .collect();
        previous.sort_unstable_by_key(|e| (e.lo, e.hi, e.forward));
        previous.sort_unstable_by_key(|e| e.from);
        shared_order(
            &mut kept,
            (&keys, &next, &points),
            &mut crate::testing::bytes(),
        )
        .unwrap();
        assert!(
            kept.iter()
                .zip(&previous)
                .all(|(&(_, a), e)| keys[next[a]] == e.to)
        );
    }
}

#[test]
fn line_paths_skip_flattening_with_its_contours() {
    let limits = Limits::default();
    let t = Transform::new(1.5, 0.25, -0.5, 2.0, 3.0, -1.0);
    let p = |x: f64, y: f64| Point::new(x, y);
    let mut paths = Vec::new();
    let mut b = PathBuilder::new();
    b.move_to(p(0.0, 0.0));
    b.line_to(p(4.0, 0.0)).unwrap();
    b.line_to(p(4.0, 3.0)).unwrap();
    b.close();
    // A line after a close reopens at the start; a lone move is a point.
    b.line_to(p(-2.0, 5.0)).unwrap();
    b.line_to(p(1.0, 6.0)).unwrap();
    b.move_to(p(9.0, 9.0));
    b.move_to(p(7.0, 1.0));
    b.line_to(p(7.0, 1.0)).unwrap();
    b.close();
    b.move_to(p(2.0, 2.0));
    b.line_to(p(5.0, 2.0)).unwrap();
    b.line_to(p(5.0, 2.0)).unwrap();
    b.line_to(p(2.0, 7.0)).unwrap();
    b.move_to(p(8.0, 8.0));
    paths.push(b.finish());
    let mut b = PathBuilder::new();
    b.rect(1.0, 2.0, 3.0, 4.0);
    paths.push(b.finish());
    for path in &paths {
        let fast = Polygon::from_lines(path, t, &limits, &mut crate::testing::bytes())
            .unwrap()
            .unwrap();
        let slow = Polygon::from_path(path, t, &limits, &mut crate::testing::bytes()).unwrap();
        assert_eq!(fast.points, slow.points);
        assert_eq!(fast.starts, slow.starts);
        assert_eq!(fast.points_cells, slow.points_cells);
    }
}

#[test]
fn shared_edges_cancel_into_the_union_boundary() {
    let square = |x: f64| {
        [
            Point::new(x, 0.0),
            Point::new(x + 1.0, 0.0),
            Point::new(x + 1.0, 1.0),
            Point::new(x, 1.0),
        ]
    };
    let mut polygon = Polygon::default();
    for x in [0.0, 1.0] {
        polygon.starts.push(polygon.points.len());
        polygon.points.extend(square(x));
    }
    let mut bytes = crate::testing::bytes();
    bytes.charge(polygon.heap_bytes()).unwrap();
    assert!(polygon.cancel_shared(&mut bytes).unwrap());
    assert_eq!(polygon.starts, [0]);
    assert_eq!(polygon.points.len(), 6);
    assert!(crate::band::certify::certified(&polygon, 0.0, &mut crate::testing::bytes()).unwrap());
    // Nothing shared: unchanged.
    let before = polygon.points.clone();
    assert!(!polygon.cancel_shared(&mut bytes).unwrap());
    assert_eq!(polygon.points, before);
}

#[test]
fn sides_sharing_a_start_relink_in_their_sorted_order() {
    // Squares meeting at corners: after the shared sides cancel, two kept
    // sides start at (2, 0) and two at (2, 1).
    let mut polygon = Polygon::default();
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (2.0, 1.0), (2.0, -1.0), (3.0, -1.0)] {
        polygon.starts.push(polygon.points.len());
        polygon.points.extend([
            Point::new(x, y),
            Point::new(x + 1.0, y),
            Point::new(x + 1.0, y + 1.0),
            Point::new(x, y + 1.0),
        ]);
    }
    assert!(polygon.cancel_shared(&mut crate::testing::bytes()).unwrap());
    assert_eq!(polygon.starts, [0, 12]);
    let points: Vec<_> = polygon.points.iter().map(|p| (p.x, p.y)).collect();
    #[rustfmt::skip]
    assert_eq!(points, [
        (0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (2.0, -1.0), (3.0, -1.0), (4.0, -1.0),
        (4.0, 0.0), (3.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0), (0.0, 1.0),
        (2.0, 1.0), (3.0, 1.0), (3.0, 2.0), (2.0, 2.0),
    ]);
}

#[test]
fn polygon_output_is_charged_beside_its_flattened_source() {
    let mut path = PathBuilder::new();
    path.polygon([
        Point::new(0.0, 0.0),
        Point::new(2.0, 0.0),
        Point::new(0.0, 2.0),
    ]);
    let path = path.finish();
    let limits = Limits::default();
    let contours = crate::flatten::flatten(
        &path,
        Transform::IDENTITY,
        &limits,
        &mut crate::testing::bytes(),
    )
    .unwrap();
    let source = crate::flatten::retained_bytes(&contours);
    let output = 3 * size_of::<Point>() + size_of::<usize>();
    for allowance in [source + output - 1, source + output] {
        let mut bytes = Meter::new(allowance, super::super::RENDER_BYTES);
        bytes.charge(source).unwrap();
        let mut polygon = Polygon::default();
        let result = polygon.extend(&contours, Transform::IDENTITY, &limits, &mut bytes);
        if allowance < source + output {
            assert_eq!(result.unwrap_err().message, super::super::RENDER_BYTES);
        } else {
            result.unwrap();
            assert_eq!(polygon.vertices(), 3);
        }
        assert_eq!(bytes.used(), source + polygon.heap_bytes());
    }
}

#[test]
fn cancellation_refuses_before_copying_its_source_edges() {
    let mut polygon = Polygon::default();
    polygon.starts.push(0);
    polygon.points.extend([
        Point::new(0.0, 0.0),
        Point::new(2.0, 0.0),
        Point::new(0.0, 2.0),
    ]);
    let source = polygon.heap_bytes();
    let before = (polygon.points.clone(), polygon.starts.clone());
    let mut bytes = Meter::new(
        source + polygon.points.len() * size_of::<(i64, i64)>() - 1,
        super::super::RENDER_BYTES,
    );
    bytes.charge(source).unwrap();
    let error = polygon.cancel_shared(&mut bytes).unwrap_err();
    assert_eq!(error.message, super::super::RENDER_BYTES);
    assert_eq!((&polygon.points, &polygon.starts), (&before.0, &before.1));
    assert_eq!(bytes.used(), source, "failed scratch is released");
}

#[test]
fn rectangle_clipping_projects_outside_parts_onto_the_sides() {
    let bounds = Bounds {
        left: 1.0,
        top: 0.0,
        right: 3.0,
        bottom: 4.0,
    };
    let edge = Line {
        a: Point::new(0.0, -1.0),
        b: Point::new(4.0, 7.0),
    };
    let mut lines = Vec::new();
    clip_line(edge, bounds, &mut lines).unwrap();
    assert_eq!(
        lines,
        [
            Line {
                a: Point::new(1.0, 0.0),
                b: Point::new(1.0, 1.0)
            },
            Line {
                a: Point::new(1.0, 1.0),
                b: Point::new(2.5, 4.0)
            },
        ]
    );
}

#[test]
fn polygons_drop_repeats_and_keep_point_cells() {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(1.5, 2.5));
    path.close();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(0.0, 0.0)).unwrap();
    path.line_to(Point::new(2.0, 0.0)).unwrap();
    path.line_to(Point::new(2.0, 2.0)).unwrap();
    path.line_to(Point::new(0.0, 0.0)).unwrap();
    let polygon = Polygon::from_path(
        &path.finish(),
        Transform::IDENTITY,
        &Limits::default(),
        &mut crate::testing::bytes(),
    )
    .unwrap();
    let contours: Vec<_> = polygon.contours().collect();
    assert_eq!(contours.len(), 2);
    assert_eq!(contours[0][0], Point::new(1.0, 2.0));
    assert_eq!(contours[1].len(), 3);
    assert!(polygon.points_cells);
}

#[test]
fn plain_interpolation_stays_within_the_interpolation_bound() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 random bits map to a uniform fraction"
        )]
        let unit = (state >> 11) as f64 / (1u64 << 53) as f64;
        unit
    };
    for case in 0..20_000 {
        // Ordinary coordinates, then ones beyond the plain range or too
        // flat for it, which must take the exact form.
        let scale = match case % 4 {
            0 | 1 => 2000.0,
            2 => 1e7,
            _ => 1.0,
        };
        let a = Point::new(scale * (next() - 0.5), scale * (next() - 0.5));
        let height = if case % 4 == 3 {
            1e-10 * next()
        } else {
            scale * (next() - 0.5)
        };
        let b = Point::new(scale * (next() - 0.5), a.y + height);
        let line = Line { a, b };
        let (lo, hi) = (a.y.min(b.y), a.y.max(b.y));
        let y = lo + (hi - lo) * next();
        let plain = a.x.abs().max(b.x.abs()) <= PLAIN_LIMIT
            && a.y.abs().max(b.y.abs()) <= PLAIN_LIMIT
            && (hi - lo) >= PLAIN_HEIGHT;
        assert_eq!(line.plain(), plain, "{case}: {line:?}");
        if plain {
            let (fast, exact) = (line.plain_x(y), line.x(y));
            assert!(
                (fast - exact).abs() <= line.interpolation_error().x,
                "{case}: {fast} vs {exact} for {line:?} at {y}"
            );
        }
    }
}
