//! Paths to device-space regions.
//!
//! Curves are halved (de Casteljau, in the path's own space) until both inner
//! control points lie within the tolerance of the chord in device space; by
//! the convex-hull property the curve then lies within the tolerance of its
//! chord. Paths and glyph outlines with general cubics use the documented
//! 1/256-pixel tolerance (ADR 0004 §2, §4). A glyph outline whose curves are
//! all exact quadratics is referenced against its true curve, so it is
//! flattened to 1/16384 pixel, which keeps every pixel's area within 1/10000
//! of a pixel (0.03 levels) of the curve's.
//!
//! The contour policy is the renderer's (ADR 0004 §4): every contour is
//! closed; a contour whose points all coincide covers the device pixel under
//! its first point; a final open contour of one point is dropped. Path
//! points are compared in the path's own space, glyph points after the
//! glyph's linear map, so a singular map can make a genuine point.

use crate::area::Region;
use pagura_core::{Error, Limits, Result};
use pagura_raster::{FillRule, Path, Point, Segment, Transform};

/// Distance of a flattened curve from its chord, in device pixels.
pub(crate) const TOLERANCE: f64 = 1.0 / 256.0;

/// The tolerance of glyph curves that are exact quadratics.
pub(crate) const QUADRATIC_TOLERANCE: f64 = 1.0 / 16384.0;

/// Third differences up to this many device pixels per coordinate make a
/// cubic an exact quadratic (degree elevation with rounding).
const QUADRATIC: f64 = 1.0 / 16_777_216.0;

/// Halvings after which a curve piece is taken as its chord.
const MAX_DEPTH: u32 = 48;

/// A flattened contour in the path's own space.
#[derive(Debug, Clone, PartialEq)]
struct Contour {
    points: Vec<Point>,
    closed: bool,
    /// Every point of the source subpath coincides.
    degenerate: bool,
}

/// How curves are flattened and points classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Curves {
    /// A path: curves within [`TOLERANCE`], points compared in its space.
    Path,
    /// A glyph outline whose curves are all exact quadratics: curves within
    /// [`QUADRATIC_TOLERANCE`], points compared after the linear map.
    Quadratic,
    /// Any other glyph outline: curves within [`TOLERANCE`], points
    /// compared after the linear map.
    Glyph,
}

impl Curves {
    fn tolerance(self) -> f64 {
        match self {
            Self::Quadratic => QUADRATIC_TOLERANCE,
            Self::Path | Self::Glyph => TOLERANCE,
        }
    }
}

/// The region of `path` under `transform` and `rule`.
///
/// # Errors
///
/// Returns `InvalidArgument` for non-finite geometry and a `Limit` error
/// beyond `limits.path_segments` flattened points.
pub(crate) fn path_region(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    limits: &Limits,
) -> Result<Region> {
    let contours = contours(path, transform, Curves::Path, limits)?;
    Ok(region(&contours, rule, |p| p * transform))
}

/// The region of a glyph outline placed at a device origin: outline points
/// are mapped by the linear map `linear` (glyph space to device pixels, no
/// translation), then `origin` is added (ADR 0004, canonical glyph
/// polygons). Glyphs fill with the nonzero rule.
///
/// # Errors
///
/// As [`path_region`].
pub(crate) fn glyph_region(
    outline: &Path,
    linear: Transform,
    origin: Point,
    limits: &Limits,
) -> Result<Region> {
    let linear = Transform {
        e: 0.0,
        f: 0.0,
        ..linear
    };
    let curves = if quadratic_outline(outline, linear) {
        Curves::Quadratic
    } else {
        Curves::Glyph
    };
    let contours = contours(outline, linear, curves, limits)?;
    Ok(region(&contours, FillRule::NonZero, |p| {
        let q = p * linear;
        Point::new(q.x + origin.x, q.y + origin.y)
    }))
}

/// Whether every curve of `outline` is an exact quadratic under `linear`.
fn quadratic_outline(outline: &Path, linear: Transform) -> bool {
    let mut position = Point::default();
    let mut start = Point::default();
    let mut quadratic = true;
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) => (start, position) = (p, p),
            Segment::LineTo(p) => position = p,
            Segment::CubicTo { c1, c2, end } => {
                quadratic &= is_quadratic(&[position, c1, c2, end].map(|p| p * linear));
                position = end;
            }
            Segment::Close => position = start,
        }
    }
    quadratic
}

/// Device edges of `contours` under the contour policy.
fn region(contours: &[Contour], rule: FillRule, map: impl Fn(Point) -> Point) -> Region {
    let mut region = Region::new(rule);
    let mut device = Vec::new();
    for (i, contour) in contours.iter().enumerate() {
        let Some(&first) = contour.points.first() else {
            continue;
        };
        if i + 1 == contours.len() && !contour.closed && contour.points.len() == 1 {
            continue;
        }
        device.clear();
        if contour.degenerate {
            let p = map(first);
            let (x, y) = (p.x.floor(), p.y.floor());
            device.extend([
                Point::new(x, y),
                Point::new(x + 1.0, y),
                Point::new(x + 1.0, y + 1.0),
                Point::new(x, y + 1.0),
            ]);
        } else {
            device.extend(contour.points.iter().map(|&p| map(p)));
        }
        region.push_contour(&device);
    }
    region
}

/// The contours of `path`, curves flattened against `device`.
fn contours(
    path: &Path,
    device: Transform,
    curves: Curves,
    limits: &Limits,
) -> Result<Vec<Contour>> {
    if !device.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    let same = |a: Point, b: Point| match curves {
        Curves::Path => a == b,
        Curves::Quadratic | Curves::Glyph => a * device == b * device,
    };
    let mut contours: Vec<Contour> = Vec::new();
    let mut count = 0_usize;
    let mut start = Point::default();
    let mut position = Point::default();
    let mut push = |contour: &mut Contour, point: Point| -> Result<()> {
        if !(point * device).is_finite() || !point.is_finite() {
            return Err(Error::invalid_argument("path geometry is not finite"));
        }
        count += 1;
        if count > limits.path_segments {
            return Err(Error::limit("flattened segment budget exceeded"));
        }
        contour.points.push(point);
        Ok(())
    };
    for segment in path.segments() {
        // A line or curve after a close starts again at the subpath start.
        if matches!(segment, Segment::LineTo(_) | Segment::CubicTo { .. })
            && contours.last().is_none_or(|c| c.closed)
        {
            contours.push(Contour {
                points: vec![start],
                closed: false,
                degenerate: true,
            });
        }
        match *segment {
            Segment::MoveTo(point) => {
                if contours.last().is_none_or(|c| !c.points.is_empty()) {
                    contours.push(Contour {
                        points: Vec::new(),
                        closed: false,
                        degenerate: true,
                    });
                }
                if let Some(contour) = contours.last_mut() {
                    push(contour, point)?;
                }
                start = point;
                position = point;
            }
            Segment::LineTo(point) => {
                if let Some(contour) = contours.last_mut() {
                    contour.degenerate &= contour.points.first().is_none_or(|&p| same(p, point));
                    push(contour, point)?;
                }
                position = point;
            }
            Segment::CubicTo { c1, c2, end } => {
                if let Some(contour) = contours.last_mut() {
                    contour.degenerate &= [c1, c2, end].iter().all(|&p| same(p, position));
                    let mut points = Vec::new();
                    flatten_curve([position, c1, c2, end], device, curves, &mut points);
                    for point in points {
                        push(contour, point)?;
                    }
                }
                position = end;
            }
            Segment::Close => {
                if let Some(contour) = contours.last_mut() {
                    contour.closed = true;
                }
                position = start;
            }
        }
    }
    contours.retain(|c| !c.points.is_empty());
    Ok(contours)
}

/// Appends the end points of the pieces of a cubic, after its start.
fn flatten_curve(points: [Point; 4], device: Transform, curves: Curves, out: &mut Vec<Point>) {
    let tolerance = curves.tolerance();
    let mut pending = vec![(points, 0)];
    while let Some((piece, depth)) = pending.pop() {
        let [a, b, c, d] = piece.map(|p| p * device);
        if depth >= MAX_DEPTH || (near(b, a, d, tolerance) && near(c, a, d, tolerance)) {
            out.push(piece[3]);
            continue;
        }
        let (first, second) = halve(piece);
        pending.push((second, depth + 1));
        pending.push((first, depth + 1));
    }
}

/// Whether a device-space cubic is an exact quadratic.
fn is_quadratic(p: &[Point; 4]) -> bool {
    let x = -p[0].x + 3.0 * p[1].x - 3.0 * p[2].x + p[3].x;
    let y = -p[0].y + 3.0 * p[1].y - 3.0 * p[2].y + p[3].y;
    x.abs() <= QUADRATIC && y.abs() <= QUADRATIC
}

/// The two halves of a cubic.
fn halve([a, b, c, d]: [Point; 4]) -> ([Point; 4], [Point; 4]) {
    let mid = |p: Point, q: Point| Point::new(f64::midpoint(p.x, q.x), f64::midpoint(p.y, q.y));
    let (ab, bc, cd) = (mid(a, b), mid(b, c), mid(c, d));
    let (abc, bcd) = (mid(ab, bc), mid(bc, cd));
    let m = mid(abc, bcd);
    ([a, ab, abc, m], [m, bcd, cd, d])
}

/// Whether `p` lies within `bound` of the segment `a`–`b`.
fn near(p: Point, a: Point, b: Point, bound: f64) -> bool {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let square = dx * dx + dy * dy;
    let t = if square == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / square).clamp(0.0, 1.0)
    };
    let (x, y) = (p.x - a.x - t * dx, p.y - a.y - t * dy);
    x.hypot(y) <= bound
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::area::areas;
    use pagura_raster::{DeviceRect, PathBuilder};

    fn window(width: u32, height: u32) -> DeviceRect {
        DeviceRect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        }
    }

    /// A circle of four cubic arcs (the usual 0.5523 control distance).
    fn circle(cx: f64, cy: f64, r: f64) -> Path {
        let k = 0.552_284_749_830_793_4 * r;
        let mut path = PathBuilder::new();
        path.move_to(Point::new(cx + r, cy));
        let arcs = [
            ((cx + r, cy + k), (cx + k, cy + r), (cx, cy + r)),
            ((cx - k, cy + r), (cx - r, cy + k), (cx - r, cy)),
            ((cx - r, cy - k), (cx - k, cy - r), (cx, cy - r)),
            ((cx + k, cy - r), (cx + r, cy - k), (cx + r, cy)),
        ];
        for (c1, c2, end) in arcs {
            path.cubic_to(
                Point::new(c1.0, c1.1),
                Point::new(c2.0, c2.1),
                Point::new(end.0, end.1),
            )
            .unwrap();
        }
        path.close();
        path.finish()
    }

    #[test]
    fn curves_stay_within_the_tolerance_of_their_chords() {
        let path = circle(10.0, 10.0, 8.0);
        let contours =
            contours(&path, Transform::IDENTITY, Curves::Path, &Limits::default()).unwrap();
        assert_eq!(contours.len(), 1);
        let points = &contours[0].points;
        assert!(points.len() > 40);
        // Every chord's midpoint is within the tolerance of the true arc,
        // whose radius is within 0.03% of 8.
        for pair in points.windows(2) {
            let m = Point::new(
                f64::midpoint(pair[0].x, pair[1].x),
                f64::midpoint(pair[0].y, pair[1].y),
            );
            let radius = (m.x - 10.0).hypot(m.y - 10.0);
            assert!(radius > 8.0 * (1.0 - 3e-4) - TOLERANCE, "{radius}");
        }
        let total: f64 = {
            let region = path_region(
                &path,
                Transform::IDENTITY,
                FillRule::NonZero,
                &Limits::default(),
            )
            .unwrap();
            let areas = areas(&[region], window(20, 20));
            (0..20)
                .flat_map(|y| (0..20).map(move |x| (x, y)))
                .map(|(x, y)| areas.get(x, y))
                .sum()
        };
        // The cubic circle's area is π r² to within 0.06%; its chords lose at
        // most two thirds of the tolerance times the perimeter.
        assert!((total - std::f64::consts::PI * 64.0).abs() < 0.3, "{total}");
    }

    #[test]
    fn degenerate_contours_cover_their_pixel_and_a_final_point_is_dropped() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(2.5, 1.5));
        path.line_to(Point::new(2.5, 1.5)).unwrap();
        path.move_to(Point::new(0.5, 0.5));
        let region = path_region(
            &path.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
            &Limits::default(),
        )
        .unwrap();
        let areas = areas(&[region], window(4, 4));
        assert_eq!(areas.get(2, 1), 1.0);
        assert_eq!(areas.get(0, 0), 0.0);
    }

    #[test]
    fn a_line_after_close_starts_at_the_subpath_start() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(2.0, 0.0)).unwrap();
        path.line_to(Point::new(2.0, 2.0)).unwrap();
        path.close();
        path.line_to(Point::new(0.0, 2.0)).unwrap();
        let contours = contours(
            &path.finish(),
            Transform::IDENTITY,
            Curves::Path,
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(contours.len(), 2);
        assert_eq!(
            contours[1].points,
            [Point::new(0.0, 0.0), Point::new(0.0, 2.0)]
        );
    }

    #[test]
    fn quadratic_glyph_curves_are_flattened_finer() {
        // An elevated quadratic: controls at 1/3 and 2/3 towards (4, 8).
        let q = [
            Point::new(0.0, 0.0),
            Point::new(4.0, 8.0),
            Point::new(8.0, 0.0),
        ];
        let c1 = Point::new(
            q[0].x + 2.0 / 3.0 * (q[1].x - q[0].x),
            q[0].y + 2.0 / 3.0 * (q[1].y - q[0].y),
        );
        let c2 = Point::new(
            q[2].x + 2.0 / 3.0 * (q[1].x - q[2].x),
            q[2].y + 2.0 / 3.0 * (q[1].y - q[2].y),
        );
        let mut path = PathBuilder::new();
        path.move_to(q[0]);
        path.cubic_to(c1, c2, q[2]).unwrap();
        path.close();
        let path = path.finish();
        let limits = Limits::default();
        let coarse = contours(&path, Transform::IDENTITY, Curves::Path, &limits).unwrap();
        let fine = contours(&path, Transform::IDENTITY, Curves::Quadratic, &limits).unwrap();
        assert!(quadratic_outline(&path, Transform::IDENTITY));
        assert!(fine[0].points.len() > 3 * coarse[0].points.len());
        // The parabola's segment area is 2/3 of base times height: 2/3·8·4.
        let region =
            glyph_region(&path, Transform::IDENTITY, Point::new(0.0, 0.0), &limits).unwrap();
        let areas = areas(&[region], window(8, 8));
        let total: f64 = (0..8)
            .flat_map(|y| (0..8).map(move |x| (x, y)))
            .map(|(x, y)| areas.get(x, y))
            .sum();
        // Chords lose at most 2/3 of the tolerance times the arc length
        // (about 12.6 pixels) in total.
        assert!(
            (total - 64.0 / 3.0).abs() < 2.0 / 3.0 * QUADRATIC_TOLERANCE * 13.0,
            "{total}"
        );
    }
}
