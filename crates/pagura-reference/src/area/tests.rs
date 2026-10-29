use super::*;

fn window(x0: u32, y0: u32, x1: u32, y1: u32) -> DeviceRect {
    DeviceRect { x0, y0, x1, y1 }
}

fn polygon(rule: FillRule, points: &[(f64, f64)]) -> Region {
    let mut region = Region::new(rule);
    let points: Vec<Point> = points.iter().map(|&(x, y)| Point::new(x, y)).collect();
    region.push_contour(&points);
    region
}

fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64) -> Region {
    polygon(FillRule::NonZero, &[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

/// The overlap of `[a0, a1]` and `[b0, b1]`.
fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// The area of a convex polygon clipped to a pixel square, by
/// Sutherland–Hodgman clipping and the shoelace formula: a method
/// independent of the slab integration.
fn convex_area(points: &[Point], x: f64, y: f64) -> f64 {
    let mut polygon = points.to_vec();
    for (axis, bound, sign) in [
        (0, x, 1.0),
        (0, x + 1.0, -1.0),
        (1, y, 1.0),
        (1, y + 1.0, -1.0),
    ] {
        let coordinate = |p: Point| if axis == 0 { p.x } else { p.y };
        let mut output = Vec::new();
        for (i, &a) in polygon.iter().enumerate() {
            let b = polygon[(i + 1) % polygon.len()];
            let (da, db) = (
                sign * (coordinate(a) - bound),
                sign * (coordinate(b) - bound),
            );
            if da >= 0.0 {
                output.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                output.push(Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y)));
            }
        }
        polygon = output;
        if polygon.is_empty() {
            return 0.0;
        }
    }
    let mut twice = 0.0;
    for (i, a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        twice += a.x * b.y - a.y * b.x;
    }
    twice.abs() / 2.0
}

#[test]
fn rectangles_cover_the_products_of_their_overlaps() {
    let (x0, y0, x1, y1) = (1.25, 0.5, 4.75, 2.125);
    let areas = areas(&[rectangle(x0, y0, x1, y1)], window(0, 0, 8, 4));
    assert_eq!(areas.rect(), window(1, 0, 5, 3));
    for y in 0..4 {
        for x in 0..8 {
            let (fx, fy) = (f64::from(x), f64::from(y));
            let expected = overlap(fx, fx + 1.0, x0, x1) * overlap(fy, fy + 1.0, y0, y1);
            assert!((areas.get(x, y) - expected).abs() < 1e-12, "({x}, {y})");
        }
    }
}

#[test]
fn a_right_triangle_halves_the_pixels_on_its_hypotenuse() {
    // x + y = 4 passes through opposite corners of the pixels with x + y = 3.
    let triangle = polygon(FillRule::NonZero, &[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)]);
    let areas = areas(&[triangle], window(0, 0, 6, 6));
    for y in 0..6 {
        for x in 0..6 {
            let expected = match x + y {
                0..=2 => 1.0,
                3 => 0.5,
                _ => 0.0,
            };
            assert!((areas.get(x, y) - expected).abs() < 1e-12, "({x}, {y})");
        }
    }
}

#[test]
fn a_circle_polygon_matches_independent_convex_clipping() {
    let (cx, cy, r) = (7.3, 6.1, 5.4);
    let points: Vec<Point> = (0..97)
        .map(|i| {
            let angle = f64::from(i) * std::f64::consts::TAU / 97.0;
            Point::new(cx + r * angle.cos(), cy + r * angle.sin())
        })
        .collect();
    let mut region = Region::new(FillRule::EvenOdd);
    region.push_contour(&points);
    let areas = areas(&[region], window(0, 0, 16, 14));
    let mut total = 0.0;
    for y in 0..14 {
        for x in 0..16 {
            let expected = convex_area(&points, f64::from(x), f64::from(y));
            assert!((areas.get(x, y) - expected).abs() < 1e-12, "({x}, {y})");
            total += areas.get(x, y);
        }
    }
    let exact = 97.0 / 2.0 * r * r * (std::f64::consts::TAU / 97.0).sin();
    assert!((total - exact).abs() < 1e-9);
}

#[test]
fn fill_rules_decide_overlapping_and_nested_contours() {
    let mut twice = Region::new(FillRule::NonZero);
    let square = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)].map(|(x, y)| Point::new(x, y));
    twice.push_contour(&square);
    twice.push_contour(&square);
    let w = window(0, 0, 2, 2);
    assert_eq!(areas(&[twice.clone()], w).get(1, 1), 1.0);
    let even_odd = Region {
        rule: FillRule::EvenOdd,
        ..twice
    };
    assert_eq!(areas(&[even_odd], w).get(1, 1), 0.0);

    // A ring: an outer square and an inner one wound the other way.
    let mut ring = Region::new(FillRule::NonZero);
    ring.push_contour(
        &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)].map(|(x, y)| Point::new(x, y)),
    );
    ring.push_contour(
        &[(1.5, 1.5), (1.5, 2.5), (2.5, 2.5), (2.5, 1.5)].map(|(x, y)| Point::new(x, y)),
    );
    let areas = areas(&[ring], window(0, 0, 4, 4));
    assert_eq!(areas.get(0, 0), 1.0);
    assert!((areas.get(1, 1) - 0.75).abs() < 1e-12);
    assert!((areas.get(2, 2) - 0.75).abs() < 1e-12);
}

#[test]
fn intersections_take_every_region_and_crossing_edges() {
    // Two rectangles: only their overlap counts.
    let a = rectangle(0.5, 0.5, 3.5, 2.5);
    let b = rectangle(2.25, 1.0, 6.0, 4.0);
    let w = window(0, 0, 8, 8);
    let both = areas(&[a, b], w);
    for y in 0..8 {
        for x in 0..8 {
            let (fx, fy) = (f64::from(x), f64::from(y));
            let expected = overlap(fx, fx + 1.0, 2.25, 3.5) * overlap(fy, fy + 1.0, 1.0, 2.5);
            assert!((both.get(x, y) - expected).abs() < 1e-12, "({x}, {y})");
        }
    }
    // A bow tie: two triangles meeting at (1, 1) that cover half of each
    // pixel.
    let bow = polygon(
        FillRule::NonZero,
        &[(0.0, 0.0), (2.0, 2.0), (2.0, 0.0), (0.0, 2.0)],
    );
    let areas = areas(&[bow], window(0, 0, 2, 2));
    for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        assert!((areas.get(x, y) - 0.5).abs() < 1e-12, "({x}, {y})");
    }
}

#[test]
fn geometry_beside_the_window_keeps_its_winding() {
    // A wide rectangle seen through a narrow window, and a slanted one
    // whose left edge lies far outside it.
    let wide = rectangle(-100.0, 0.25, 100.0, 1.0);
    let areas = areas(&[wide], window(3, 0, 5, 2));
    assert_eq!(areas.rect(), window(3, 0, 5, 1));
    assert!((areas.get(3, 0) - 0.75).abs() < 1e-12);
    assert!((areas.get(4, 0) - 0.75).abs() < 1e-12);
    let slanted = polygon(
        FillRule::EvenOdd,
        &[(-50.0, 0.0), (10.0, 0.0), (10.0, 1.0), (-49.0, 1.0)],
    );
    let areas = super::areas(&[slanted], window(2, 0, 4, 1));
    assert!((areas.get(2, 0) - 1.0).abs() < 1e-12);
    assert!((areas.get(3, 0) - 1.0).abs() < 1e-12);
}

#[test]
fn empty_regions_and_no_regions() {
    let w = window(0, 0, 3, 2);
    let none = areas(&[], w);
    assert_eq!(none.rect(), w);
    assert_eq!(none.get(2, 1), 1.0);
    let empty = areas(&[Region::new(FillRule::NonZero)], w);
    assert_eq!(empty.rect(), DeviceRect::default());
    assert_eq!(empty.get(0, 0), 0.0);
}
