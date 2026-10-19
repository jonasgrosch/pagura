use super::super::{Scratch, coverage};
use super::*;
use crate::{FillRule, Mask, Path, PathBuilder, Rasterizer, Transform};
use std::sync::Arc;

// Independent oracle: each contour clipped to each cell (Sutherland and
// Hodgman), with signed shoelace areas summed over contours.
fn clip(polygon: &[Point], axis: bool, value: f64, below: bool) -> Vec<Point> {
    let coordinate = |p: Point| if axis { p.y } else { p.x };
    let keep = |p: Point| {
        if below {
            coordinate(p) <= value
        } else {
            coordinate(p) >= value
        }
    };
    let mut out = Vec::new();
    for (i, &p) in polygon.iter().enumerate() {
        let q = polygon[(i + 1) % polygon.len()];
        if keep(p) {
            out.push(p);
        }
        if keep(p) != keep(q) {
            let t = (value - coordinate(p)) / (coordinate(q) - coordinate(p));
            out.push(Point::new(p.x + t * (q.x - p.x), p.y + t * (q.y - p.y)));
        }
    }
    out
}

fn oracle(glyph: &GlyphGeometry, origin: Point, cell: (i64, i64)) -> f64 {
    let (x, y) = (cell.0 as f64, cell.1 as f64);
    let mut area = 0.0;
    for contour in glyph.contours() {
        let mut polygon: Vec<Point> = contour
            .points()
            .iter()
            .map(|p| Point::new(p.x + origin.x, p.y + origin.y))
            .collect();
        for (axis, value, below) in [
            (true, y, false),
            (true, y + 1.0, true),
            (false, x, false),
            (false, x + 1.0, true),
        ] {
            if polygon.len() < 3 {
                break;
            }
            polygon = clip(&polygon, axis, value, below);
        }
        for (i, a) in polygon.iter().enumerate() {
            let b = polygon[(i + 1) % polygon.len()];
            area += ((a.x - x) * (b.y - y) - (b.x - x) * (a.y - y)) * 0.5;
        }
    }
    area.abs()
}

fn outlines() -> Vec<Path> {
    let mut ring = PathBuilder::new();
    // An "o": outer counterclockwise ellipse, inner clockwise one.
    for (rx, ry, reverse) in [(5.0, 7.0, false), (2.5, 4.5, true)] {
        let k = 0.552_284_749_8;
        let mut points = [
            (rx, 0.0),
            (rx, k * ry),
            (k * rx, ry),
            (0.0, ry),
            (-k * rx, ry),
            (-rx, k * ry),
            (-rx, 0.0),
            (-rx, -k * ry),
            (-k * rx, -ry),
            (0.0, -ry),
            (k * rx, -ry),
            (rx, -k * ry),
            (rx, 0.0),
        ];
        if reverse {
            points.reverse();
        }
        let p = |(x, y): (f64, f64)| Point::new(x + 6.0, y + 8.0);
        ring.move_to(p(points[0]));
        for c in points[1..].chunks(3) {
            ring.cubic_to(p(c[0]), p(c[1]), p(c[2])).unwrap();
        }
        ring.close();
    }
    let mut slanted = PathBuilder::new();
    // A slanted stem with a horizontal serif and an open final contour.
    for point in [
        (0.0, 0.0),
        (9.0, 0.0),
        (9.0, 1.5),
        (6.5, 1.5),
        (3.25, 13.0),
        (0.5, 12.0),
    ] {
        if point == (0.0, 0.0) {
            slanted.move_to(Point::new(point.0, point.1));
        } else {
            slanted.line_to(Point::new(point.0, point.1)).unwrap();
        }
    }
    let mut thin = PathBuilder::new();
    // A long sliver crossing many columns within single rows.
    thin.move_to(Point::new(0.0, 0.0));
    thin.line_to(Point::new(31.0, 2.1)).unwrap();
    thin.line_to(Point::new(31.0, 2.3)).unwrap();
    thin.close();
    vec![ring.finish(), slanted.finish(), thin.finish()]
}

fn page(width: u32, height: u32) -> Mask {
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, f64::from(width), f64::from(height));
    Mask::from_path(
        width,
        height,
        &path.finish(),
        Transform::IDENTITY,
        FillRule::NonZero,
        usize::MAX,
    )
    .unwrap()
}

fn glyphs() -> Vec<Arc<GlyphGeometry>> {
    let limits = Limits::default();
    let mut glyphs = Vec::new();
    for outline in outlines() {
        for linear in [
            [1.0, 0.0, 0.0, 1.0],
            [2.75, 0.0, 0.31, -2.75],
            [0.0, 1.3, -1.7, 0.2],
        ] {
            let glyph = Arc::new(GlyphGeometry::new(&outline, linear, &limits).unwrap());
            assert!(glyph.signed_area());
            glyphs.push(glyph);
        }
    }
    glyphs
}

#[test]
fn every_cell_rounds_the_exact_polygon_area() {
    let clip = page(160, 160);
    let mut raster = Rasterizer::default();
    for (n, glyph) in glyphs().into_iter().enumerate() {
        for k in 0..12 {
            let f = f64::from(k);
            let origin = Point::new(
                60.0 + f * 0.618_033_988_7 + n as f64 * 0.1,
                70.0 - f * 0.414_213_562_3,
            );
            let mask = raster
                .clipped_glyph(clip.region(), Arc::clone(&glyph), origin, Limits::default())
                .unwrap()
                .unwrap();
            let (x0, y0) = mask.origin();
            for (i, &value) in mask.coverage().iter().enumerate() {
                let w = mask.width() as usize;
                let cell = (
                    i64::from(x0) + i64::try_from(i % w).unwrap(),
                    i64::from(y0) + i64::try_from(i / w).unwrap(),
                );
                let exact = oracle(&glyph, origin, cell) * 255.0;
                if (exact.fract() - 0.5).abs() < 1e-6 {
                    assert!((f64::from(value) - exact).abs() <= 0.5 + 1e-6);
                } else {
                    assert_eq!(f64::from(value), exact.round(), "cell {cell:?}");
                }
            }
            raster.recycle(mask);
        }
    }
}

#[test]
fn tiles_and_rectangle_clips_cut_the_same_cells() {
    let limits = Limits::default();
    for glyph in glyphs() {
        let origin = Point::new(20.375, 21.8125);
        let mut whole = [0; 64 * 64];
        let shape = Shape {
            geometry: Geometry::Glyph(Arc::clone(&glyph)),
            transform: Transform::translate(origin.x, origin.y),
            rule: FillRule::NonZero,
        };
        coverage(
            64,
            64,
            std::slice::from_ref(&shape),
            &mut whole,
            (0, 0),
            &limits,
            &mut Scratch::default(),
        )
        .unwrap();
        // Integer clip rectangles that do not cover the tile restrict it exactly.
        let mut rectangle = PathBuilder::new();
        rectangle.rect(23.0, 0.0, 9.0, 40.0);
        let clip = Shape {
            geometry: Geometry::Path(Arc::new(rectangle.finish())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        };
        for (x, y, w, h) in [
            (0, 0, 7, 9),
            (7, 9, 13, 5),
            (20, 14, 30, 33),
            (31, 3, 1, 61),
        ] {
            let mut tile = vec![0; w * h];
            let size = (u32::try_from(w).unwrap(), u32::try_from(h).unwrap());
            let shapes = [shape.clone(), clip.clone()];
            let mut scratch = Scratch::default();
            coverage(
                size.0,
                size.1,
                &shapes[..1],
                &mut tile,
                (x, y),
                &limits,
                &mut scratch,
            )
            .unwrap();
            for (row, values) in tile.chunks(w).enumerate() {
                let start = (y as usize + row) * 64 + x as usize;
                assert_eq!(values, &whole[start..start + w]);
            }
            coverage(
                size.0,
                size.1,
                &shapes,
                &mut tile,
                (x, y),
                &limits,
                &mut scratch,
            )
            .unwrap();
            for (row, values) in tile.chunks(w).enumerate() {
                for (column, &value) in values.iter().enumerate() {
                    let (gx, gy) = (x as usize + column, y as usize + row);
                    let inside = (23..32).contains(&gx) && gy < 40;
                    assert_eq!(value, if inside { whole[gy * 64 + gx] } else { 0 });
                }
            }
        }
    }
}

#[test]
fn fractional_clip_edges_take_the_geometric_clip_only_where_they_cut() {
    let limits = Limits::default();
    let glyph = &glyphs()[0];
    let origin = Point::new(10.3, 11.6);
    let glyph_shape = Shape {
        geometry: Geometry::Glyph(Arc::clone(glyph)),
        transform: Transform::translate(origin.x, origin.y),
        rule: FillRule::NonZero,
    };
    let mut rectangle = PathBuilder::new();
    rectangle.rect(12.25, 13.5, 10.5, 7.75);
    let clipped = [
        glyph_shape.clone(),
        Shape {
            geometry: Geometry::Path(Arc::new(rectangle.finish())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        },
    ];
    let mut whole = [0; 40 * 40];
    coverage(
        40,
        40,
        &clipped[..1],
        &mut whole,
        (0, 0),
        &limits,
        &mut Scratch::default(),
    )
    .unwrap();
    let mut actual = [0; 40 * 40];
    coverage(
        40,
        40,
        &clipped,
        &mut actual,
        (0, 0),
        &limits,
        &mut Scratch::default(),
    )
    .unwrap();
    let mut reference = [0; 40 * 40];
    let mut scratch = Scratch::default();
    let prepared = scratch
        .exact
        .prepare(&clipped, (0, 0), (40, 40), &limits)
        .unwrap();
    super::super::polygon(
        40,
        40,
        &clipped,
        &mut reference,
        (0, 0),
        &limits,
        &mut scratch,
        (prepared, None),
    )
    .unwrap();
    for (i, &value) in actual.iter().enumerate() {
        let (x, y) = ((i % 40) as f64, (i / 40) as f64);
        let expected = if x >= 13.0 && x + 1.0 <= 22.75 && y >= 14.0 && y + 1.0 <= 21.25 {
            whole[i]
        } else {
            reference[i]
        };
        assert_eq!(value, expected, "cell {x} {y}");
    }
    assert!(actual.iter().any(|&v| v != 0 && v != 255));
}

#[test]
fn wide_glyphs_accumulate_in_row_bands() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(900.0, 150.0)).unwrap();
    path.line_to(Point::new(0.0, 301.0)).unwrap();
    path.close();
    let glyph =
        Arc::new(GlyphGeometry::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(glyph.signed_area());
    let origin = Point::new(0.25, 0.5);
    let clip = page(904, 304);
    let mask = Rasterizer::default()
        .clipped_glyph(clip.region(), Arc::clone(&glyph), origin, limits)
        .unwrap()
        .unwrap();
    assert!(mask.height() as usize > BAND_CELLS / (mask.width() as usize + 2));
    for (i, &value) in mask.coverage().iter().enumerate().step_by(97) {
        let w = mask.width() as usize;
        let cell = (i64::try_from(i % w).unwrap(), i64::try_from(i / w).unwrap());
        let exact = oracle(&glyph, origin, cell) * 255.0;
        assert!((f64::from(value) - exact).abs() <= 0.5 + 1e-6);
    }
}

#[test]
fn budget_errors_leave_no_partial_sums() {
    let glyph = Arc::clone(&glyphs()[0]);
    let clip = page(40, 40);
    let mut raster = Rasterizer::default();
    let exhausted = Limits {
        raster_work: 40,
        ..Limits::default()
    };
    let origin = Point::new(3.5, 4.25);
    assert!(
        raster
            .clipped_glyph(clip.region(), Arc::clone(&glyph), origin, exhausted)
            .is_err()
    );
    let mask = raster
        .clipped_glyph(clip.region(), Arc::clone(&glyph), origin, Limits::default())
        .unwrap()
        .unwrap();
    let reference = Rasterizer::default()
        .clipped_glyph(clip.region(), glyph, origin, Limits::default())
        .unwrap()
        .unwrap();
    assert_eq!(mask.coverage(), reference.coverage());
}

#[test]
fn uncertified_or_curved_glyphs_keep_their_paths() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    // Two overlapping squares of the same orientation reach winding 2.
    path.rect(0.0, 0.0, 4.0, 4.0);
    path.rect(2.0, 2.0, 4.0, 4.0);
    let overlap = GlyphGeometry::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
    assert!(!overlap.signed_area());
    // A degree-elevated quadratic keeps the curve sweep and its true-curve oracle.
    let mut quadratic = PathBuilder::new();
    let (p0, p1, p2) = (
        Point::new(0.0, 0.0),
        Point::new(6.0, 0.0),
        Point::new(6.0, 6.0),
    );
    let c1 = Point::new(
        p0.x + (p1.x - p0.x) * 2.0 / 3.0,
        p0.y + (p1.y - p0.y) * 2.0 / 3.0,
    );
    let c2 = Point::new(
        p2.x + (p1.x - p2.x) * 2.0 / 3.0,
        p2.y + (p1.y - p2.y) * 2.0 / 3.0,
    );
    quadratic.move_to(p0);
    quadratic.cubic_to(c1, c2, p2).unwrap();
    quadratic.close();
    let curve = GlyphGeometry::new(&quadratic.finish(), [4.0, 0.0, 0.0, 4.0], &limits).unwrap();
    assert!(curve.curves().is_some());
    assert!(!curve.signed_area());
}

// A star-shaped contour around `center` with radii in `radii`, turning by
// `sign`. Angle steps stay below 2.02 radians, so its edges keep at least
// 0.53 times the smallest radius from the centre.
fn star(
    path: &mut PathBuilder,
    (center, radii, sign): (Point, [f64; 2], f64),
    snap: impl Fn(f64) -> f64,
    uniform: &mut impl FnMut() -> f64,
) {
    let n = 5 + (0..7).filter(|_| uniform() < 0.5).count();
    let step = std::f64::consts::TAU / n as f64;
    for k in 0..n {
        let angle = sign * step * (k as f64 + 0.6 * (uniform() - 0.5));
        let radius = radii[0] + (radii[1] - radii[0]) * uniform();
        let point = Point::new(
            snap(center.x + radius * angle.cos()),
            snap(center.y + radius * angle.sin()),
        );
        if k == 0 {
            path.move_to(point);
        } else {
            path.line_to(point).unwrap();
        }
    }
    path.close();
}

// Star contours side by side, some with holes and islands of alternating
// orientation, some overlapping or of opposite orientation, some replaced by
// two random triangles, and half of them on a quarter-pixel grid, which adds
// horizontal edges and shared heights.
fn random_outline(uniform: &mut impl FnMut() -> f64) -> Path {
    let mut path = PathBuilder::new();
    let grid = uniform() < 0.5;
    let snap = |v: f64| if grid { (v * 4.0).round() / 4.0 } else { v };
    let orientation = if uniform() < 0.5 { 1.0 } else { -1.0 };
    let groups = 1 + usize::from(uniform() < 0.6) + usize::from(uniform() < 0.4);
    let mut center = Point::new(10.0, 10.0);
    for group in 0..groups {
        if group > 0 {
            center = if uniform() < 0.25 {
                Point::new(
                    center.x + 3.0 + 5.0 * uniform(),
                    center.y + 8.0 * (uniform() - 0.5),
                )
            } else {
                Point::new(10.0 + 20.0 * group as f64, 10.0)
            };
        }
        let sign = if uniform() < 0.2 {
            -orientation
        } else {
            orientation
        };
        if uniform() < 0.2 {
            // Two triangles in one box often cross within a single slab.
            for _ in 0..2 {
                let mut corner = || {
                    Point::new(
                        snap(center.x + 16.0 * (uniform() - 0.5)),
                        snap(center.y + 16.0 * (uniform() - 0.5)),
                    )
                };
                path.move_to(corner());
                path.line_to(corner()).unwrap().line_to(corner()).unwrap();
                path.close();
            }
            continue;
        }
        let r = 3.0 + 3.5 * uniform();
        star(&mut path, (center, [r, 1.4 * r], sign), snap, uniform);
        if groups == 1 || uniform() < 0.6 {
            let hole = (center, [0.2 * r, 0.4 * r], -sign);
            star(&mut path, hole, snap, uniform);
            if !grid && uniform() < 0.5 {
                let island = (center, [0.05 * r, 0.1 * r], sign);
                star(&mut path, island, |v| v, uniform);
            }
        }
    }
    path.finish()
}

// The nonzero winding number of the glyph's closed contours at `p`.
fn winding(glyph: &GlyphGeometry, p: Point) -> i32 {
    let mut winding = 0;
    for contour in glyph.contours() {
        let points = contour.points();
        for (i, &a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            let side = (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y);
            if a.y <= p.y && p.y < b.y && side > 0.0 {
                winding += 1;
            } else if b.y <= p.y && p.y < a.y && side < 0.0 {
                winding -= 1;
            }
        }
    }
    winding
}

#[test]
fn random_certified_glyphs_match_the_polygon_path_and_their_windings() {
    let limits = Limits::default();
    let mut seed = 0x5851_f42d_4c95_7f2d_u64;
    let mut uniform = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1_u64 << 53) as f64
    };
    let (mut certified, mut violated) = (0, 0);
    for _ in 0..300 {
        let outline = random_outline(&mut uniform);
        let glyph = Arc::new(GlyphGeometry::new(&outline, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        // The certificate claims winding 0 or one common sign everywhere.
        let [lo, hi] = glyph.bounds().unwrap();
        let mut common = None;
        let holds = (0..256).all(|_| {
            let p = Point::new(
                lo.x + (hi.x - lo.x) * uniform(),
                lo.y + (hi.y - lo.y) * uniform(),
            );
            match winding(&glyph, p) {
                0 => true,
                w => w.abs() == 1 && *common.get_or_insert(w) == w,
            }
        });
        violated += usize::from(!holds);
        if !glyph.signed_area() {
            continue;
        }
        assert!(
            holds,
            "certified glyph {outline:?} violates its winding claim"
        );
        assert!(glyph.contours().len() > 1);
        certified += 1;
        let origin = Point::new(3.0 + uniform(), 4.0 + uniform());
        let shapes = [Shape {
            geometry: Geometry::Glyph(Arc::clone(&glyph)),
            transform: Transform::translate(origin.x, origin.y),
            rule: FillRule::NonZero,
        }];
        let mut kernel = [0; 80 * 32];
        let mut scratch = Scratch::default();
        coverage(80, 32, &shapes, &mut kernel, (0, 0), &limits, &mut scratch).unwrap();
        let mut reference = [0; 80 * 32];
        let prepared = scratch
            .exact
            .prepare(&shapes, (0, 0), (80, 32), &limits)
            .unwrap();
        super::super::polygon(
            80,
            32,
            &shapes,
            &mut reference,
            (0, 0),
            &limits,
            &mut scratch,
            (prepared, None),
        )
        .unwrap();
        for (i, (&k, &r)) in kernel.iter().zip(&reference).enumerate() {
            assert!(
                k.abs_diff(r) <= 1,
                "cell {i} of {outline:?}: {k} against {r}"
            );
        }
    }
    // Both outcomes occur, and every glyph whose sampled windings break the
    // claim was rejected.
    assert!(
        certified >= 100 && violated >= 100,
        "{certified} {violated}"
    );
}
