use super::*;
use crate::PathBuilder;

#[test]
fn many_sides_at_one_vertex_keep_the_previous_link_order() {
    /// The keyed link as it was before it carried its side: the unstable
    /// sorts' tie order depends on the size, which is the same.
    #[derive(Clone, Copy)]
    struct Previous {
        lo: (i64, i64),
        hi: (i64, i64),
        forward: bool,
        from: (i64, i64),
        to: (i64, i64),
        #[expect(dead_code, reason = "only the prior layout is compared")]
        a: Point,
    }
    assert_eq!(size_of::<Keyed>(), size_of::<Previous>());
    for count in [8, 64, 257] {
        let origin = (0_i64, 0_i64);
        let mut keys = vec![origin; count];
        keys.extend((1..=count).rev().map(|i| (i64::try_from(i).unwrap(), 0)));
        let next: Vec<_> = (count..2 * count).collect();
        let mut kept: Vec<_> = (0..count).map(|i| (origin, i)).collect();
        let mut previous: Vec<_> = kept
            .iter()
            .map(|&(from, a)| {
                let to = keys[next[a]];
                Previous {
                    lo: from.min(to),
                    hi: from.max(to),
                    forward: from <= to,
                    from,
                    to,
                    a: Point::default(),
                }
            })
            .collect();
        previous.sort_unstable_by_key(|e| (e.lo, e.hi, e.forward));
        previous.sort_unstable_by_key(|e| e.from);
        shared_order(&mut kept, (&keys, &next), &mut crate::testing::bytes()).unwrap();
        assert!(
            kept.iter()
                .zip(&previous)
                .all(|(&(_, a), e)| keys[next[a]] == e.to)
        );
    }
}

#[test]
fn relinked_contours_match_a_search_of_the_start_keys() {
    // Random unit squares on a small grid, some reversed or repeated: shared
    // sides cancel, and several kept sides can start at one corner.
    let mut state = 7_u64;
    let mut random = |count: u64| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) % count
    };
    for _ in 0..2000 {
        let mut polygon = Polygon::default();
        for _ in 0..=random(12) {
            let (x, y) = (random(4) as f64, random(4) as f64);
            let mut square = [(x, y), (x + 1.0, y), (x + 1.0, y + 1.0), (x, y + 1.0)];
            if random(4) == 0 {
                square.reverse();
            }
            polygon.starts.push(polygon.points.len());
            polygon.points.extend(square.map(|(x, y)| Point::new(x, y)));
        }
        let expected = searched(&polygon);
        let changed = polygon.cancel_shared(&mut crate::testing::bytes()).unwrap();
        assert_eq!(changed, expected.is_some());
        if let Some((points, starts)) = expected {
            assert_eq!((polygon.points, polygon.starts), (points, starts));
        }
    }
}

/// The contours `cancel_shared` relinks `polygon` into, each successor found
/// by a search of all kept sides in their order.
fn searched(polygon: &Polygon) -> Option<(Vec<Point>, Vec<usize>)> {
    let n = polygon.points.len();
    let keys: Vec<_> = polygon
        .points
        .iter()
        .map(|p| (ordered(p.x), ordered(p.y)))
        .collect();
    let mut next = Vec::new();
    for (contour, &start) in polygon.starts.iter().enumerate() {
        let end = polygon.starts.get(contour + 1).copied().unwrap_or(n);
        next.extend(start + 1..end);
        next.push(start);
    }
    let mut bytes = crate::testing::bytes();
    let mut kept = uncancelled(&keys, &next, &mut Vec::new(), &mut bytes).unwrap();
    if kept.len() == n {
        return None;
    }
    kept.sort_unstable();
    shared_order(&mut kept, (&keys, &next), &mut bytes).unwrap();
    let mut used = vec![false; kept.len()];
    let (mut points, mut starts) = (Vec::new(), Vec::new());
    for first in 0..kept.len() {
        if used[first] {
            continue;
        }
        starts.push(points.len());
        let mut current = first;
        loop {
            used[current] = true;
            let a = kept[current].1;
            points.push(polygon.points[a]);
            let end = keys[next[a]];
            if end == kept[first].0 {
                break;
            }
            current = (0..kept.len()).find(|&i| kept[i].0 == end && !used[i])?;
        }
    }
    Some((points, starts))
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
