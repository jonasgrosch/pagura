use super::*;
use crate::band::DeviceRect;
use crate::band::edges::{StripList, rows};
use pagura_core::Meter;

#[test]
fn keyed_sides_keep_contour_closing_and_final_point_order() {
    let polygon = Polygon {
        points: vec![
            Point::new(2.0, 1.0),
            Point::new(5.0, 3.0),
            Point::new(2.0, 5.0),
            Point::new(6.0, 2.0),
            Point::new(8.0, 4.0),
            Point::new(6.0, 6.0),
        ],
        starts: vec![0, 3],
        points_cells: false,
    };
    let points = [Point::new(-0.25, 3.75), Point::new(-0.25, 3.75)];
    let view = PolygonLines::new(&polygon, &points, Point::new(0.5, -0.5));
    let indexed: Vec<_> = view.indexed().collect();
    assert_eq!(
        indexed.iter().map(|&(key, _)| key).collect::<Vec<_>>(),
        [0, 1, 6, 3, 4, 7, 8, 9, 10, 11]
    );
    for &(key, line) in &indexed {
        assert_eq!(view.line(key), Some(line));
    }
    assert_eq!(
        indexed[6].1,
        Line {
            a: Point::new(1.0, 3.0),
            b: Point::new(1.0, 4.0)
        }
    );
    assert_eq!(
        indexed[7].1,
        Line {
            a: Point::new(0.0, 4.0),
            b: Point::new(0.0, 3.0)
        }
    );
    assert_eq!(
        &indexed[6..8].iter().map(|(_, l)| *l).collect::<Vec<_>>(),
        &indexed[8..].iter().map(|(_, l)| *l).collect::<Vec<_>>()
    );
}

#[test]
fn point_sides_keep_the_final_floor_and_signed_zero_bits() {
    for (point, expected) in [
        (Point::new(-0.0, -0.0), [1.0, -0.0, -0.0, 1.0]),
        (Point::new(-0.25, 3.75), [0.0, 3.0, -1.0, 4.0]),
        (Point::new(1e12, 1e12), [1e12 + 1.0, 1e12, 1e12, 1e12 + 1.0]),
    ] {
        let [right, left] = point_sides(point);
        let [right_x, top, left_x, bottom] = expected;
        for (actual, expected) in [
            (right.a, Point::new(right_x, top)),
            (right.b, Point::new(right_x, bottom)),
            (left.a, Point::new(left_x, bottom)),
            (left.b, Point::new(left_x, top)),
        ] {
            assert_eq!(actual.x.to_bits(), expected.x.to_bits());
            assert_eq!(actual.y.to_bits(), expected.y.to_bits());
        }
    }
}

#[test]
fn source_keys_preserve_filtered_strip_order_and_capacity() {
    let polygon = Polygon {
        points: vec![
            Point::new(2.0, 0.0),
            Point::new(4.0, 0.0),
            Point::new(3.0, 40.0),
            Point::new(5.0, 3.0),
            Point::new(7.0, 3.0),
            Point::new(6.0, 39.0),
        ],
        starts: vec![0, 3],
        points_cells: false,
    };
    let points = [Point::new(8.25, 17.75)];
    let view = PolygonLines::new(&polygon, &points, Point::new(-0.5, 0.25));
    let region = DeviceRect {
        x0: 0,
        y0: 0,
        x1: 16,
        y1: 48,
    };
    let range = |l: Line| rows(l.a.y.min(l.b.y), l.a.y.max(l.b.y), region);
    let lines: Vec<_> = view
        .indexed()
        .map(|(_, l)| l)
        .filter(|l| !l.is_horizontal())
        .collect();
    let mut indexed_bytes = Meter::new(1 << 20, "bytes");
    let indexed = StripList::new(
        (0, 48),
        view.keys().unwrap(),
        view.indexed().filter_map(|(key, line)| {
            (!line.is_horizontal())
                .then(|| range(line))
                .flatten()
                .map(|r| (key, r))
        }),
        &mut indexed_bytes,
    )
    .unwrap();
    let mut owned_bytes = Meter::new(1 << 20, "bytes");
    let owned = StripList::new(
        (0, 48),
        lines.len(),
        lines
            .iter()
            .enumerate()
            .filter_map(|(i, &line)| range(line).map(|r| (i, r))),
        &mut owned_bytes,
    )
    .unwrap();
    assert_eq!(
        (
            indexed.len(),
            indexed.retained_bytes(),
            indexed_bytes.used()
        ),
        (owned.len(), owned.retained_bytes(), owned_bytes.used())
    );
    for strip in 0..3 {
        let shared: Vec<_> = indexed
            .strip(strip)
            .iter()
            .map(|&i| view.line(i as usize).unwrap())
            .collect();
        let copied: Vec<_> = owned
            .strip(strip)
            .iter()
            .map(|&i| lines[i as usize])
            .collect();
        assert_eq!(shared, copied);
    }
}

#[test]
fn key_ranges_refuse_overflow_without_an_allocation() {
    assert_eq!(key_count(6, 2, 2), Some(12));
    assert_eq!(key_count(usize::MAX, 1, 0), None);
    assert_eq!(key_count(0, 0, usize::MAX), None);
    assert_eq!(key_count(u32::MAX as usize, 1, 0), None);
}
