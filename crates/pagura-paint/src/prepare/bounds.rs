//! Conservative device bounds for culling: control hulls of paths under
//! their transforms, which contain every pixel a mark's exact coverage can
//! reach.

use pagura_core::Rect;
use pagura_raster::{DeviceRect, Path, Point, Segment, Transform};

/// A rectangle in device space, `[x0, y0, x1, y1]`.
pub(crate) type Hull = [f64; 4];

/// The control hull of `points`; `None` when there are none.
pub(crate) fn points(points: impl IntoIterator<Item = Point>) -> Option<Hull> {
    points.into_iter().fold(None, |hull, p| {
        Some(match hull {
            None => [p.x, p.y, p.x, p.y],
            Some([x0, y0, x1, y1]) => [x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)],
        })
    })
}

/// The control hull of `path` under `transform`.
pub(crate) fn path(path: &Path, transform: Transform) -> Option<Hull> {
    points(path.segments().iter().flat_map(|segment| {
        let (a, b, c) = match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => (Some(p), None, None),
            Segment::CubicTo { c1, c2, end } => (Some(c1), Some(c2), Some(end)),
            Segment::QuadTo { control, end } => (Some(control), Some(end), None),
            Segment::Close => (None, None, None),
        };
        [a, b, c]
            .into_iter()
            .flatten()
            .map(move |p| transform.apply(p))
    }))
}

/// [`path`] for the canonical corners of a rectangle, without a temporary path.
pub(crate) fn rect(rect: Rect, transform: Transform) -> Option<Hull> {
    points(
        Path::rect_corners(rect)
            .into_iter()
            .map(|point| transform.apply(point)),
    )
}

/// `hull` grown by `margin` on every side.
pub(crate) fn expand(hull: Hull, margin: f64) -> Hull {
    [
        hull[0] - margin,
        hull[1] - margin,
        hull[2] + margin,
        hull[3] + margin,
    ]
}

/// The pixels of `region` that `hull` can touch. A hull with a non-finite
/// or NaN coordinate is taken as the whole region.
pub(crate) fn device(hull: Option<Hull>, region: DeviceRect) -> DeviceRect {
    let Some([x0, y0, x1, y1]) = hull else {
        return DeviceRect::default();
    };
    if [x0, y0, x1, y1].iter().any(|v| v.is_nan()) {
        return region;
    }
    let low = |v: f64, lo: u32, hi: u32| clamp(v.floor(), lo, hi);
    let high = |v: f64, lo: u32, hi: u32| clamp(v.ceil(), lo, hi);
    let rect = DeviceRect {
        x0: low(x0, region.x0, region.x1),
        y0: low(y0, region.y0, region.y1),
        x1: high(x1, region.x0, region.x1),
        y1: high(y1, region.y0, region.y1),
    };
    if rect.is_empty() {
        DeviceRect::default()
    } else {
        rect
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "an integral value clamped to a u32 range"
)]
fn clamp(value: f64, lo: u32, hi: u32) -> u32 {
    value.clamp(f64::from(lo), f64::from(hi)) as u32
}

/// The bound of the scale of `transform`'s linear part: its Frobenius
/// norm, at least its largest singular value.
pub(crate) fn scale(transform: Transform) -> f64 {
    (transform.a * transform.a
        + transform.b * transform.b
        + transform.c * transform.c
        + transform.d * transform.d)
        .sqrt()
}

/// How far a stroke with `pen` reaches beyond its path in device pixels
/// under `transform`: the pen's radius, times the miter limit for miter
/// joins or √2 for square caps. The radius is half the line width through
/// the pen's scale, and half a pixel for a hairline, whose pen is one device
/// pixel wide under any transform. Stroke adjustment rounds the pen's
/// diameters up by at most half a pixel, to at least one, and moves its
/// centrelines by at most half a pixel (10.7.5).
pub(crate) fn stroke_reach(pen: &pagura_interpret::Pen, transform: Transform) -> f64 {
    let stroke = &pen.stroke;
    let join = if stroke.join == pagura_raster::LineJoin::Miter {
        stroke.miter_limit.max(1.0)
    } else {
        1.0
    }
    .max(std::f64::consts::SQRT_2);
    let reach = if stroke.width == 0.0 {
        0.5 * join
    } else {
        stroke.width.max(0.0) / 2.0 * join * scale(pen.transform.then(transform))
    };
    match pen.adjustment {
        pagura_raster::Adjustment::None => reach,
        pagura_raster::Adjustment::Adjusted => (reach + 0.25 * join).max(0.5 * join) + 0.5,
    }
}

/// The unit square under `transform`, as a path.
#[cfg(test)]
pub(crate) fn unit_square() -> Path {
    let mut builder = pagura_raster::PathBuilder::new();
    builder.rect(0.0, 0.0, 1.0, 1.0);
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{Limits, Meter};

    #[test]
    fn hulls_round_outwards_and_clamp() {
        let region = DeviceRect {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        };
        let hull = path(
            &unit_square(),
            Transform::new(2.5, 0.0, 0.0, 2.0, 1.25, 3.0),
        );
        assert_eq!(hull, Some([1.25, 3.0, 3.75, 5.0]));
        assert_eq!(
            device(hull, region),
            DeviceRect {
                x0: 1,
                y0: 3,
                x1: 4,
                y1: 5
            }
        );
        assert!(device(Some([20.0, 0.0, 30.0, 5.0]), region).is_empty());
        assert_eq!(device(Some([f64::NAN, 0.0, 1.0, 1.0]), region), region);
        assert_eq!(device(Some([-1e300, -1e300, 1e300, 1e300]), region), region);
    }

    #[test]
    fn rectangle_bounds_match_canonical_paths_bit_for_bit() {
        for source in [
            Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap(),
            Rect::from_corners(-3.0, 0.1, 7.0, 0.3).unwrap(),
            Rect::from_corners(-0.0, -0.0, 0.0, 0.0).unwrap(),
            Rect::from_corners(-1e100, -2e100, 3e100, 4e100).unwrap(),
        ] {
            for transform in [
                Transform::IDENTITY,
                Transform::new(-0.0, 1.25, -2.0, 0.5, -0.0, 7.0),
                Transform::new(1e-90, -2e-90, 3e-90, 4e-90, 1e100, -1e100),
            ] {
                let bits = |hull: Option<Hull>| hull.map(|hull| hull.map(f64::to_bits));
                assert_eq!(
                    bits(rect(source, transform)),
                    bits(path(&Path::rect(source), transform))
                );
            }
        }
    }

    /// The pixels a shape covers over `area`.
    fn covered(shape: &pagura_raster::Shape, area: pagura_raster::Area) -> Vec<(u32, u32)> {
        use pagura_raster::{BandCoverage, BandRasterizer, MarkCoverage, Span};
        let mut bytes = Meter::new(usize::MAX, "test bytes");
        let mut raster = BandRasterizer::new(area, &mut bytes).unwrap();
        let mut band = BandCoverage::with_capacity(shape.capacity(), &mut bytes).unwrap();
        let mut pixels = Vec::new();
        for b in shape.bands(&area) {
            let mark = MarkCoverage {
                shape: Some(shape),
                clip: None,
            };
            raster.rasterize(mark, b, &mut band);
            for row in band.rows() {
                for span in row.spans() {
                    match span {
                        Span::Full { x, len } => pixels.extend((x..x + len).map(|x| (x, row.y))),
                        Span::Partial { x, coverage } => pixels.extend(
                            (x..)
                                .zip(coverage)
                                .filter(|&(_, &c)| c > 0)
                                .map(|(x, _)| (x, row.y)),
                        ),
                    }
                }
            }
        }
        pixels
    }

    /// A polyline through `points`, closed or not.
    fn polyline(points: &[[f64; 2]], closed: bool) -> Path {
        let mut builder = pagura_raster::PathBuilder::new();
        let mut points = points.iter().map(|&[x, y]| Point::new(x, y));
        if let Some(first) = points.next() {
            builder.move_to(first);
        }
        for point in points {
            builder.line_to(point).unwrap();
        }
        if closed {
            builder.close();
        }
        builder.finish()
    }

    /// A polyline from the origin turning by each of `corners` (degrees
    /// between the incoming and outgoing segment) after segments of length
    /// 6, back and forth across the x axis.
    fn zigzag(corners: &[f64]) -> Vec<[f64; 2]> {
        let mut points = vec![[0.0, 0.0]];
        let mut heading: f64 = 0.0;
        let (mut x, mut y) = (0.0, 0.0);
        for (i, corner) in corners.iter().enumerate() {
            x += 6.0 * heading.cos();
            y += 6.0 * heading.sin();
            points.push([x, y]);
            let turn = (180.0 - corner).to_radians();
            heading += if i % 2 == 0 { turn } else { -turn };
        }
        points.push([x + 6.0 * heading.cos(), y + 6.0 * heading.sin()]);
        points
    }

    /// Over a region far larger than any test mark.
    const AREA: pagura_raster::Area = pagura_raster::Area {
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: 4096,
            y1: 4096,
        },
        band_height: 32,
    };

    /// Asserts that every pixel `shape` covers lies in the culling rectangle
    /// of `hull`.
    fn within(what: &str, shape: Option<&pagura_raster::Shape>, hull: Option<Hull>) {
        let Some(shape) = shape else {
            return;
        };
        let culling = device(hull, AREA.region);
        for (x, y) in covered(shape, AREA) {
            assert!(
                (culling.x0..culling.x1).contains(&x) && (culling.y0..culling.y1).contains(&y),
                "{what}: pixel ({x}, {y}) outside {culling:?}"
            );
        }
    }

    /// Default user space to device pixels: unit and 300 dpi, flipped.
    const DEVICES: [Transform; 2] = [
        Transform::new(1.0, 0.0, 0.0, -1.0, 1000.0, 1000.0),
        Transform::new(25.0 / 6.0, 0.0, 0.0, -25.0 / 6.0, 1000.0, 2000.0),
    ];

    /// Corners from nearly retraced to nearly straight, in degrees.
    const CORNERS: [f64; 8] = [3.0, 6.0, 12.0, 30.0, 60.0, 90.0, 135.0, 170.0];

    /// Every pixel a stroke covers lies in the culling rectangle its hull
    /// gives, over strokes drawn from every join, cap, width, adjustment,
    /// dash and pen.
    #[test]
    fn stroke_coverage_stays_within_the_culling_hull() {
        use pagura_raster::{Adjustment, LineCap, LineJoin, Shape, ShapeOptions, Stroke};
        let limits = Limits::default();
        let options = ShapeOptions {
            area: AREA,
            limits: &limits,
        };
        let paths = [
            ("zigzag", polyline(&zigzag(&CORNERS), false)),
            (
                "triangle",
                polyline(&[[0.0, 0.0], [1.08, 4.32], [0.0, 4.32]], true),
            ),
            (
                "square",
                polyline(&[[0.0, 0.0], [5.0, 0.0], [5.0, 5.0], [0.0, 5.0]], true),
            ),
            (
                "ell",
                polyline(&[[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]], false),
            ),
            ("dot", polyline(&[[1.0, 1.0], [1.0, 1.0]], false)),
            (
                "sliver",
                polyline(&[[0.0, 0.0], [0.05, 0.0], [0.05, 0.05]], true),
            ),
        ];
        let pens = [
            Transform::IDENTITY,
            Transform::scale(0.12, 0.12),
            Transform::new(1.5, 0.4, -0.3, 0.6, 0.0, 0.0),
        ];
        let joins = [
            (LineJoin::Miter, 1.0),
            (LineJoin::Miter, 10.0),
            (LineJoin::Miter, 100.0),
            (LineJoin::Round, 10.0),
            (LineJoin::Bevel, 10.0),
        ];
        let caps = [LineCap::Butt, LineCap::Round, LineCap::ProjectingSquare];
        let widths = [0.0, 0.1, 2.0];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut pick = |n: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(seed >> 33).unwrap() % n
        };
        for _ in 0..400 {
            let (name, user) = &paths[pick(paths.len())];
            let device_transform = DEVICES[pick(DEVICES.len())];
            let (join, miter_limit) = joins[pick(joins.len())];
            let pen = pagura_interpret::Pen {
                stroke: Stroke {
                    width: widths[pick(widths.len())],
                    cap: caps[pick(caps.len())],
                    join,
                    miter_limit,
                    dash_array: [Vec::new(), vec![0.0, 1.5]][pick(2)].clone(),
                    dash_phase: 0.0,
                },
                transform: pens[pick(pens.len())],
                adjustment: [Adjustment::None, Adjustment::Adjusted][pick(2)],
            };
            let shape = Shape::stroke(
                user,
                &pen.stroke,
                pen.transform.inverse().unwrap(),
                device_transform,
                pen.adjustment,
                &options,
            )
            .unwrap();
            let reach = stroke_reach(&pen, device_transform);
            within(
                &format!("{name} {pen:?} under {device_transform:?}"),
                shape.as_ref(),
                path(user, device_transform).map(|h| expand(h, reach + 2.0)),
            );
        }
    }

    /// Every pixel a fill covers lies in the culling rectangle a fill's hull
    /// gives, a pixel wider than its path's, empty fills included: a
    /// retraced polyline paints its hairline outline, whose bevel joins keep
    /// within half a pixel of the corners it turns.
    #[test]
    fn fill_coverage_stays_within_the_culling_hull() {
        use pagura_raster::{FillRule, Shape, ShapeOptions};
        let limits = Limits::default();
        let options = ShapeOptions {
            area: AREA,
            limits: &limits,
        };
        for corner in CORNERS {
            let turned = zigzag(&[corner]);
            let retraced: Vec<[f64; 2]> = turned
                .iter()
                .chain(turned.iter().rev().skip(1))
                .copied()
                .collect();
            for user in [polyline(&retraced, true), polyline(&turned, true)] {
                for device_transform in DEVICES {
                    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
                        let shape = Shape::fill(&user, device_transform, rule, &options).unwrap();
                        within(
                            &format!("{corner}° fill {rule:?} under {device_transform:?}"),
                            shape.as_ref(),
                            path(&user, device_transform).map(|h| expand(h, 1.0)),
                        );
                    }
                }
            }
        }
    }
}
