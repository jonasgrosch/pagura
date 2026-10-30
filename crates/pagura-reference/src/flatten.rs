//! Paths to device-space regions.
//!
//! Curves are halved (de Casteljau, in the path's own space) until both inner
//! control points lie within the tolerance of the chord in device space; by
//! the convex-hull property the curve then lies within the tolerance of its
//! chord. Paths and glyph outlines referenced against their polygon use the
//! documented 1/256-pixel tolerance (ADR 0004 §2, §4). A glyph outline
//! referenced against its true curve is flattened with a whole-outline
//! initial symmetric-difference area bound of 1/65536 pixel (under 0.004
//! levels), refined when its error interval cannot decide a coverage byte.
//! Each admitted chord and the curve lie in their common control hull;
//! the sum of those hull areas bounds the entire outline's difference.
//!
//! The contour policy is the renderer's (ADR 0004 §4): every contour is
//! closed; a contour whose points all coincide covers the device pixel under
//! its first point; a final open contour of one point is dropped. Path
//! points are compared in the path's own space, glyph points after the
//! glyph's linear map, so a singular map can make a genuine point.

use crate::area::Region;
use crate::glyph::Reference;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};
use pagura_raster::{DeviceRect, FillRule, Path, Point, Segment, Transform};
use std::sync::Arc;

/// Distance of a flattened curve from its chord, in device pixels.
pub(crate) const TOLERANCE: f64 = 1.0 / 256.0;

/// Whole-outline area allowance for true-curve glyph references.
pub(crate) const CURVE_AREA: f64 = 1.0 / 65536.0;

/// Halvings after which a curve piece is taken as its chord.
const MAX_DEPTH: u32 = 48;

/// The area budget, a retained coordinate error from localization and,
/// for a localized pixel, the curves its refinement leaves as they were.
#[derive(Debug, Clone, Copy)]
struct Precision {
    area: f64,
    rounding: f64,
    focus: Option<Focus>,
}

/// A localized pixel's square: a curve piece whose hulls cannot meet it
/// keeps the `coarse` budget when the others are refined, as it adds
/// nothing to the pixel's interval.
#[derive(Debug, Clone, Copy)]
struct Focus {
    bounds: [f64; 4],
    coarse: f64,
}

impl Precision {
    const INITIAL: Self = Self {
        area: CURVE_AREA,
        rounding: 0.0,
        focus: None,
    };

    /// Each of `curves` curves' share of the budgets.
    fn per_curve(self, curves: usize) -> Self {
        let curves = f64::from(u32::try_from(curves.max(1)).unwrap_or(u32::MAX));
        Self {
            area: self.area / curves,
            focus: self.focus.map(|focus| Focus {
                coarse: focus.coarse / curves,
                ..focus
            }),
            ..self
        }
    }
}

/// Error-free subtraction: `high + tail` is the exact difference of the
/// stored inputs (two-sum, without assuming Sterbenz applies).
pub(crate) fn subtract(value: f64, integer: f64) -> (f64, f64) {
    let high = value - integer;
    let virtual_integer = high - value;
    let tail = (value - (high - virtual_integer)) + (-integer - virtual_integer);
    (high, tail)
}

/// A true curve can be rebuilt more finely from the original controls.
#[derive(Debug, Clone)]
pub(crate) struct CurveReference {
    outline: Arc<Path>,
    linear: Transform,
    origin: Point,
    precision: Precision,
    limits: Limits,
}

impl CurveReference {
    pub(crate) fn refine(&self) -> Result<Region> {
        glyph_region_at_area(
            &self.outline,
            self.linear,
            self.origin,
            Reference::Curve,
            Precision {
                area: self.precision.area / 16.0,
                ..self.precision
            },
            &self.limits,
        )
    }
    pub(crate) fn localized(&self, x: u32, y: u32) -> Result<Region> {
        let (x, dx) = subtract(self.origin.x, f64::from(x));
        let (y, dy) = subtract(self.origin.y, f64::from(y));
        let rounding =
            (self.precision.rounding + dx.abs().max(dy.abs())) * (1.0 + 4.0 * f64::EPSILON);
        glyph_region_at_area(
            &self.outline,
            self.linear,
            Point::new(x, y),
            Reference::Curve,
            Precision {
                rounding,
                // The localized pixel is the unit square at the origin.
                focus: Some(Focus {
                    bounds: [0.0, 0.0, 1.0, 1.0],
                    coarse: self.precision.area,
                }),
                ..self.precision
            },
            &self.limits,
        )
    }
}

/// An accepted chord and its curve differ only inside their control hull.
/// Its outward rectangle localizes the area uncertainty to touched pixels.
#[derive(Debug, Clone)]
pub(crate) struct Hull {
    bounds: [f64; 4],
    area: f64,
}

impl Hull {
    pub(crate) fn bounds(&self) -> [f64; 4] {
        self.bounds
    }
    pub(crate) fn restricted(&self, bounds: [f64; 4]) -> Option<Self> {
        let bounds = [
            self.bounds[0].max(bounds[0]),
            self.bounds[1].max(bounds[1]),
            self.bounds[2].min(bounds[2]),
            self.bounds[3].min(bounds[3]),
        ];
        (bounds[0] < bounds[2] && bounds[1] < bounds[3]).then_some(Self {
            bounds,
            area: self.area,
        })
    }

    pub(crate) fn error(&self, x: u32, y: u32) -> f64 {
        let [x0, y0, x1, y1] = self.bounds;
        if x0 < f64::from(x) + 1.0
            && f64::from(x) < x1
            && y0 < f64::from(y) + 1.0
            && f64::from(y) < y1
        {
            self.area
        } else {
            0.0
        }
    }

    /// Endpoint rounding sweeps at most dx*variation_y+dy*variation_x in
    /// a unit pixel. Each coordinate of a line has one monotone interval;
    /// pure tangential rounding of an axis-aligned edge adds no area.
    pub(crate) fn line_error(a: Point, b: Point, delta: Point) -> Option<Self> {
        let error = delta.x.max(delta.y);
        (error != 0.0).then(|| Self {
            bounds: [
                (a.x.min(b.x) - error).next_down(),
                (a.y.min(b.y) - error).next_down(),
                (a.x.max(b.x) + error).next_up(),
                (a.y.max(b.y) + error).next_up(),
            ],
            area: 2.0
                * (delta.x * ((b.y - a.y).abs() + 2.0 * delta.y).min(1.0)
                    + delta.y * ((b.x - a.x).abs() + 2.0 * delta.x).min(1.0))
                + 8.0 * error * error,
        })
    }

    /// Adds this hull's bound only to pixels its rectangle can meet.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite bounds clamped into a u32 device rectangle"
    )]
    pub(crate) fn accumulate(&self, rect: DeviceRect, errors: &mut [f64]) {
        let [x0, y0, x1, y1] = self.bounds;
        let column = |x: f64| x.clamp(f64::from(rect.x0), f64::from(rect.x1)) as u32;
        let row = |y: f64| y.clamp(f64::from(rect.y0), f64::from(rect.y1)) as u32;
        let width = u32_to_usize(rect.width());
        for y in row(y0.floor())..row(y1.ceil()) {
            let start = u32_to_usize(y - rect.y0) * width;
            for x in column(x0.floor())..column(x1.ceil()) {
                let error = &mut errors[start + u32_to_usize(x - rect.x0)];
                *error = (*error + self.area) * (1.0 + 4.0 * f64::EPSILON);
            }
        }
    }
}

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
    /// A glyph outline referenced against its true curve: bounded total
    /// control-hull area, points compared after the linear map.
    Curve,
    /// Any other glyph outline: curves within [`TOLERANCE`], points
    /// compared after the linear map.
    Glyph,
}

impl Curves {
    fn tolerance(self) -> f64 {
        match self {
            Self::Curve => 0.0,
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
/// translation), then `origin` is added (ADR 0004 §4). Glyphs fill with the
/// nonzero rule; `reference` selects the true curve or the polygon.
///
/// # Errors
///
/// As [`path_region`].
pub(crate) fn glyph_region(
    outline: &Path,
    linear: Transform,
    origin: Point,
    reference: Reference,
    limits: &Limits,
) -> Result<Region> {
    glyph_region_at_area(
        outline,
        linear,
        origin,
        reference,
        Precision::INITIAL,
        limits,
    )
}

fn glyph_region_at_area(
    outline: &Path,
    linear: Transform,
    origin: Point,
    reference: Reference,
    precision: Precision,
    limits: &Limits,
) -> Result<Region> {
    let linear = Transform {
        e: 0.0,
        f: 0.0,
        ..linear
    };
    let curves = match reference {
        Reference::Curve => Curves::Curve,
        Reference::Polygon => Curves::Glyph,
    };
    // True-curve intervals include addition of the placement origin:
    // outward determinant/mapping error is evaluated in placed coordinates.
    // Canonical polygon subdivision still depends only on the linear map.
    let device = if reference == Reference::Curve {
        Transform {
            e: origin.x,
            f: origin.y,
            ..linear
        }
    } else {
        linear
    };
    let (contours, hulls) = contours_with_area(outline, device, curves, precision, limits)?;
    let mut region = region(&contours, FillRule::NonZero, |p| {
        let q = p * linear;
        Point::new(q.x + origin.x, q.y + origin.y)
    });
    if reference == Reference::Curve {
        region.hulls = hulls;
        region.curve = Some(CurveReference {
            outline: Arc::new(outline.clone()),
            linear,
            origin,
            precision,
            limits: *limits,
        });
    }
    Ok(region)
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
    contours_with_area(path, device, curves, Precision::INITIAL, limits)
        .map(|(contours, _)| contours)
}

fn contours_with_area(
    path: &Path,
    device: Transform,
    curves: Curves,
    precision: Precision,
    limits: &Limits,
) -> Result<(Vec<Contour>, Vec<Hull>)> {
    if !device.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    let linear = Transform {
        e: 0.0,
        f: 0.0,
        ..device
    };
    let same = |a: Point, b: Point| match curves {
        Curves::Path => a == b,
        Curves::Curve | Curves::Glyph => a * linear == b * linear,
    };
    let mut contours: Vec<Contour> = Vec::new();
    let mut hulls = Vec::new();
    let mut count = 0_usize;
    let curve_count = path
        .segments()
        .iter()
        .filter(|s| matches!(s, Segment::CubicTo { .. } | Segment::QuadTo { .. }))
        .count();
    let allowance = precision.per_curve(curve_count);
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
        if !matches!(segment, Segment::MoveTo(_) | Segment::Close)
            && contours.last().is_none_or(|c| c.closed)
        {
            contours.push(Contour {
                points: vec![start],
                closed: false,
                degenerate: true,
            });
        }
        let piece = cubic(*segment, position);
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
            Segment::CubicTo { .. } | Segment::QuadTo { .. } => {}
            Segment::Close => {
                if let Some(contour) = contours.last_mut() {
                    contour.closed = true;
                }
                position = start;
            }
        }
        if let Some((controls @ [_, c1, c2, end], elevation)) = piece {
            if let Some(contour) = contours.last_mut() {
                contour.degenerate &= [c1, c2, end].iter().all(|&p| same(p, position));
                let mut points = Vec::new();
                flatten_curve(
                    (controls, elevation),
                    device,
                    curves,
                    allowance,
                    &mut points,
                    &mut hulls,
                    limits,
                )?;
                for point in points {
                    push(contour, point)?;
                }
            }
            position = end;
        }
    }
    contours.retain(|c| !c.points.is_empty());
    Ok((contours, hulls))
}

/// Rounding units of the coordinates' magnitude that elevating a quadratic
/// adds to its cubic controls.
const ELEVATION: f64 = 32.0;

/// A curve's cubic controls from `from`, with the rounding units their
/// elevation adds: a quadratic flattens as its elevated cubic. `None` for
/// a move, line or close.
fn cubic(segment: Segment, from: Point) -> Option<([Point; 4], f64)> {
    match segment {
        Segment::CubicTo { c1, c2, end } => Some(([from, c1, c2, end], 0.0)),
        Segment::QuadTo { control, end } => {
            let [c1, c2] = Segment::elevated(from, control, end);
            Some(([from, c1, c2, end], ELEVATION))
        }
        Segment::MoveTo(_) | Segment::LineTo(_) | Segment::Close => None,
    }
}

/// Appends the end points of the pieces of a cubic, after its start; its
/// controls carry `elevation` rounding units beyond the source's.
fn flatten_curve(
    (points, elevation): ([Point; 4], f64),
    device: Transform,
    curves: Curves,
    precision: Precision,
    out: &mut Vec<Point>,
    hulls: &mut Vec<Hull>,
    limits: &Limits,
) -> Result<()> {
    if curves == Curves::Curve {
        return flatten_true_curve((points, elevation), device, precision, (out, hulls), limits);
    }
    let tolerance = curves.tolerance();
    let mut pending = vec![(points, 0)];
    while let Some((piece, depth)) = pending.pop() {
        let [a, b, c, d] = piece.map(|p| p * device);
        if near(b, a, d, tolerance) && near(c, a, d, tolerance) {
            if out.len() >= limits.path_segments {
                return Err(Error::limit("flattened segment budget exceeded"));
            }
            out.push(piece[3]);
            continue;
        }
        if depth >= MAX_DEPTH {
            return Err(Error::limit("curve subdivision depth exceeded"));
        }
        let (first, second) = halve(piece);
        pending.push((second, depth + 1));
        pending.push((first, depth + 1));
    }
    Ok(())
}

/// A curve of a glyph referenced against its true curve, halved until each
/// piece's control hull, which holds the piece and its chord, has at most
/// its share of the area budget.
///
/// The curve is halved in its own frame: its controls less its first one,
/// in the path's space, mapped by the linear map alone. Each coordinate of
/// a piece's mapped controls, and of its emitted end point once placed,
/// then lies within
///
/// ```text
/// δ(d) = ε·(32·(d + 4)·own + (elevation + 8)·mapped + 4·placed) + rounding
/// ```
///
/// of the exact one, at depth `d`:
///
/// - `own`, the largest mapped control of the curve's frame, bounds the
///   rounding of forming the frame and of `d` halvings in it (the existing
///   allowance of 32 units a halving, four more for the frame and mapping);
/// - `mapped`, the largest mapped control, bounds the source's elevation of
///   a quadratic and the emitted point's return to the path's space;
/// - `placed`, the largest placed control, bounds the placement, which adds
///   the origin once and does not grow with depth;
/// - `rounding` is a localized pixel's two-sum tail.
///
/// The hull's area is outward for controls within δ, and its bounds are
/// widened by δ, as before; only the error is the curve's own instead of
/// the page's, so pieces far from the device origin still meet their share.
fn flatten_true_curve(
    (points, elevation): ([Point; 4], f64),
    device: Transform,
    precision: Precision,
    (out, hulls): (&mut Vec<Point>, &mut Vec<Hull>),
    limits: &Limits,
) -> Result<()> {
    let linear = Transform {
        e: 0.0,
        f: 0.0,
        ..device
    };
    let magnitude = |p: Point, t: Transform| {
        ((p.x * t.a).abs() + (p.y * t.c).abs() + t.e.abs())
            .max((p.x * t.b).abs() + (p.y * t.d).abs() + t.f.abs())
    };
    let start = points[0];
    let frame = points.map(|p| Point::new(p.x - start.x, p.y - start.y));
    let largest = |controls: &[Point; 4], t: Transform| {
        controls
            .iter()
            .map(|&p| magnitude(p, t))
            .fold(1.0_f64, f64::max)
    };
    let (own, mapped, placed) = (
        largest(&frame, linear),
        largest(&points, linear),
        largest(&points, device),
    );
    let fixed = f64::EPSILON * ((elevation + 8.0) * mapped + 4.0 * placed) + precision.rounding;
    let anchor = start * device;
    // The hulls of a piece's halves lie in its control box, widened by the
    // deepest piece's error: a piece whose widened box misses the focus
    // cannot touch its pixel, and takes the coarse budget.
    let widening = own * f64::EPSILON * 32.0 * f64::from(MAX_DEPTH + 4) + fixed;
    let misses = |controls: &[Point; 4]| {
        precision.focus.is_some_and(|focus| {
            let [x0, y0, x1, y1] = focus.bounds;
            controls.iter().all(|p| anchor.x + p.x + widening <= x0)
                || controls.iter().all(|p| anchor.x + p.x - widening >= x1)
                || controls.iter().all(|p| anchor.y + p.y + widening <= y0)
                || controls.iter().all(|p| anchor.y + p.y - widening >= y1)
        })
    };
    let coarse = precision.focus.map_or(precision.area, |focus| focus.coarse);
    let mut pending = vec![(frame, 0, (precision.area, coarse))];
    while let Some((piece, depth, (fine, coarse))) = pending.pop() {
        let controls = piece.map(|p| p * linear);
        let share = if misses(&controls) { coarse } else { fine };
        let [a, b, c, d] = controls;
        let error = own * f64::EPSILON * 32.0 * f64::from(depth + 4) + fixed;
        let triangle = |p: Point, q: Point| {
            let (u, v) = (p - a, q - a);
            let products = (u.x * v.y).abs() + (u.y * v.x).abs();
            let determinant = (u.x * v.y - u.y * v.x).abs()
                + 16.0 * f64::EPSILON * products
                + 2.0 * error * (u.x.abs() + u.y.abs() + v.x.abs() + v.y.abs())
                + 8.0 * error * error;
            determinant * (0.5 + 16.0 * f64::EPSILON) + f64::MIN_POSITIVE
        };
        let area = (triangle(b, c) + triangle(b, d) + triangle(c, d)) * (1.0 + 16.0 * f64::EPSILON);
        if area <= share {
            if out.len() >= limits.path_segments {
                return Err(Error::limit("flattened segment budget exceeded"));
            }
            // The last piece ends at the curve's own end.
            out.push(if piece[3] == frame[3] {
                points[3]
            } else {
                Point::new(start.x + piece[3].x, start.y + piece[3].y)
            });
            hulls.push(Hull {
                bounds: [
                    anchor.x + a.x.min(b.x).min(c.x).min(d.x) - error,
                    anchor.y + a.y.min(b.y).min(c.y).min(d.y) - error,
                    anchor.x + a.x.max(b.x).max(c.x).max(d.x) + error,
                    anchor.y + a.y.max(b.y).max(c.y).max(d.y) + error,
                ],
                area,
            });
            continue;
        }
        if depth >= MAX_DEPTH {
            return Err(Error::limit("curve subdivision depth exceeded"));
        }
        let (first, second) = halve(piece);
        let shares = (fine * 0.5, coarse * 0.5);
        pending.push((second, depth + 1, shares));
        pending.push((first, depth + 1, shares));
    }
    Ok(())
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

    #[test]
    fn placed_curve_hulls_bound_mapping_error_at_large_origins() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.cubic_to(
            Point::new(1.0 / 3.0, 0.5),
            Point::new(2.0 / 3.0, 0.5),
            Point::new(1.0, 0.0),
        )
        .unwrap();
        path.close();
        let path = path.finish();
        for offset in [0, 10_000] {
            let region = glyph_region(
                &path,
                Transform::IDENTITY,
                Point::new(f64::from(offset), f64::from(offset)),
                Reference::Curve,
                &Limits::default(),
            )
            .unwrap();
            let pixel = DeviceRect {
                x0: offset,
                y0: offset,
                x1: offset + 1,
                y1: offset + 1,
            };
            let polygon = areas(std::slice::from_ref(&region), pixel).get(offset, offset);
            let mut errors = [0.0];
            for hull in &region.hulls {
                hull.accumulate(pixel, &mut errors);
            }
            // This original cubic is degree-elevated from a quadratic of
            // control height3/4, so its area is exactly1/4 before mapping.
            assert!((polygon - 0.25).abs() <= errors[0]);
            let sum: f64 = region
                .hulls
                .iter()
                .map(|hull| hull.error(offset, offset))
                .sum();
            assert!(errors[0] >= sum);
            assert!(errors[0] <= CURVE_AREA);
        }
    }

    #[test]
    fn many_curves_far_from_the_origin_meet_their_shares() {
        // 30 coincident arches, 2 px wide: each curve's share of the area
        // budget is about 5e-7 px². At a placed magnitude of a million
        // pixels, rounding scaled by placed coordinates at every halving
        // exceeds that share at any depth; in each curve's own frame it does
        // not, and the hulls still bound the area.
        let mut path = PathBuilder::new();
        for _ in 0..30 {
            path.move_to(Point::new(0.0, 0.0));
            path.cubic_to(
                Point::new(2.0 / 3.0, 1.0),
                Point::new(4.0 / 3.0, 1.0),
                Point::new(2.0, 0.0),
            )
            .unwrap();
            path.close();
        }
        let path = path.finish();
        for offset in [0, 2500, 1_000_000] {
            let region = glyph_region(
                &path,
                Transform::IDENTITY,
                Point::new(f64::from(offset), f64::from(offset)),
                Reference::Curve,
                &Limits::default(),
            )
            .unwrap();
            let rect = DeviceRect {
                x0: offset,
                y0: offset,
                x1: offset + 2,
                y1: offset + 1,
            };
            let values = areas(std::slice::from_ref(&region), rect);
            let total = values.get(offset, offset) + values.get(offset + 1, offset);
            let error: f64 = region.hulls.iter().map(|hull| hull.area).sum();
            // The elevated quadratic of control height 1.5 covers 1 px².
            assert!((total - 1.0).abs() <= error, "{offset}: {total} ± {error}");
            assert!(error <= CURVE_AREA);
        }
    }

    #[test]
    fn local_half_level_intervals_preserve_translation_tails() {
        let (high, tail) = subtract(1.0 / 3.0, 17.0);
        assert_ne!(tail, 0.0);
        assert_eq!((high + 17.0) + tail, 1.0 / 3.0);

        let mut path = PathBuilder::new();
        let h = 3.0 * (47.5 + 0.000_022) / 255.0;
        for _ in 0..32 {
            path.move_to(Point::default());
            path.cubic_to(
                Point::new(1.0 / 3.0, 2.0 * h / 3.0),
                Point::new(2.0 / 3.0, 2.0 * h / 3.0),
                Point::new(1.0, 0.0),
            )
            .unwrap();
            path.close();
        }
        let path = path.finish();
        // Same-direction coincident contours still have this one arch's
        // analytic area h/3. Refining at a large absolute origin must not
        // confuse the rounding floor with a genuine half-level ambiguity.
        for offset in [0, 10_000] {
            let region = glyph_region(
                &path,
                Transform::IDENTITY,
                Point::new(f64::from(offset), f64::from(offset)),
                Reference::Curve,
                &Limits::default(),
            )
            .unwrap();
            let pixel = DeviceRect {
                x0: offset,
                y0: offset,
                x1: offset + 1,
                y1: offset + 1,
            };
            assert_eq!(crate::exact::coverage(&[region], pixel).unwrap(), [48]);
        }
    }

    /// An arch of control height `h` over `x..x + w`, as an elevated
    /// quadratic, and closed along its base.
    fn arch(path: &mut PathBuilder, x: f64, w: f64, h: f64) {
        path.move_to(Point::new(x, 0.0));
        path.cubic_to(
            Point::new(x + w / 3.0, 2.0 * h / 3.0),
            Point::new(x + 2.0 * w / 3.0, 2.0 * h / 3.0),
            Point::new(x + w, 0.0),
        )
        .unwrap();
        path.close();
    }

    #[test]
    fn refinement_leaves_curves_that_miss_the_pixel_as_they_were() {
        let mut path = PathBuilder::new();
        arch(&mut path, 0.0, 1.0, 0.75);
        arch(&mut path, 100.0, 40.0, 30.0);
        let path = path.finish();
        let region = glyph_region(
            &path,
            Transform::IDENTITY,
            Point::new(10_000.25, 5_000.0),
            Reference::Curve,
            &Limits::default(),
        )
        .unwrap();
        let curve = region.curve.as_ref().unwrap();
        let local = curve.localized(10_000, 5_000).unwrap();
        let refined = local.curve.as_ref().unwrap().refine().unwrap();
        // The far arch keeps its pieces; the near one is halved more finely.
        let far = |r: &Region| r.hulls.iter().filter(|h| h.bounds()[0] > 50.0).count();
        let near = |r: &Region| r.hulls.iter().filter(|h| h.bounds()[2] < 50.0).count();
        assert_eq!(far(&refined), far(&local));
        assert!(near(&refined) > near(&local));
        // Refining every curve gives the pixel the same area and interval.
        let reference = local.curve.as_ref().unwrap();
        let whole = glyph_region_at_area(
            &path,
            Transform::IDENTITY,
            reference.origin,
            Reference::Curve,
            Precision {
                area: reference.precision.area / 16.0,
                focus: None,
                ..reference.precision
            },
            &Limits::default(),
        )
        .unwrap();
        let pixel = window(1, 1);
        let interval = |r: &Region| -> f64 { r.hulls.iter().map(|h| h.error(0, 0)).sum() };
        assert_eq!(interval(&refined), interval(&whole));
        let area = |r: &Region| areas(std::slice::from_ref(r), pixel).get(0, 0);
        assert_eq!(area(&refined), area(&whole));
    }

    #[test]
    fn a_pixel_decides_where_refining_the_whole_glyph_exceeds_its_budget() {
        // An arch whose area is a hair above a half level, which takes
        // refinement to decide, beside 6 larger arches far from the pixel.
        let h = 3.0 * (47.5 + 0.000_022) / 255.0;
        let mut path = PathBuilder::new();
        arch(&mut path, 0.0, 1.0, h);
        for i in 0..6 {
            arch(&mut path, 50.0 + 20.0 * f64::from(i), 8.0, 6.0);
        }
        let path = path.finish();
        let origin = Point::new(10_000.0, 10_000.0);
        let place = |limits: &Limits| {
            glyph_region(&path, Transform::IDENTITY, origin, Reference::Curve, limits)
        };
        // A segment budget five times the first flattening's pieces.
        let pieces = place(&Limits::default()).unwrap().hulls.len();
        let limits = Limits {
            path_segments: 5 * pieces,
            ..Limits::default()
        };
        let pixel = DeviceRect {
            x0: 10_000,
            y0: 10_000,
            x1: 10_001,
            y1: 10_001,
        };
        let region = place(&limits).unwrap();
        assert_eq!(crate::exact::coverage(&[region], pixel).unwrap(), [48]);
        // Halving every curve sixteen times more finely twice exceeds it.
        let whole = glyph_region_at_area(
            &path,
            Transform::IDENTITY,
            origin,
            Reference::Curve,
            Precision {
                area: CURVE_AREA / 256.0,
                ..Precision::INITIAL
            },
            &limits,
        );
        assert!(whole.is_err());
    }

    #[test]
    fn translated_clips_exclude_hulls_outside_their_fractional_bounds() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(-1.0, 0.0));
        path.cubic_to(
            Point::new(0.0, 1.0),
            Point::new(1.0, 1.0),
            Point::new(2.0, 0.0),
        )
        .unwrap();
        path.close();
        let path = path.finish();
        // Across this pixel the quadratic boundary is above1/2. A clip at
        // height0.3 therefore covers exactly that stored height, even though
        // the curve's hulls touch the same pixel beyond the clip. Their area
        // uncertainty cannot affect this genuine coverage half-level.
        for offset in [0, 10_000] {
            let base = f64::from(offset);
            let mark = glyph_region(
                &path,
                Transform::IDENTITY,
                Point::new(base + 0.2, base),
                Reference::Curve,
                &Limits::default(),
            )
            .unwrap();
            assert!(
                mark.hulls
                    .iter()
                    .any(|hull| hull.error(offset, offset) > 0.0)
            );
            // A far endpoint makes subtraction retain a nonzero two-sum
            // tail at the translated placement; it is tangential to its edge.
            let left = if offset == 0 { -0.1 } else { 1.0 / 3.0 };
            let top = base + 0.3;
            let clip = Path::rect(
                pagura_core::Rect::from_corners(left, base - 0.1, base + 1.1, top).unwrap(),
            );
            let clip = path_region(
                &clip,
                Transform::IDENTITY,
                FillRule::NonZero,
                &Limits::default(),
            )
            .unwrap();
            let pixel = DeviceRect {
                x0: offset,
                y0: offset,
                x1: offset + 1,
                y1: offset + 1,
            };
            assert_eq!(
                crate::exact::coverage(&[mark, clip], pixel).unwrap(),
                [pagura_raster::quantize_unit(top - base)]
            );
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
    fn true_curve_glyphs_are_flattened_finer() {
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
        let fine = contours(&path, Transform::IDENTITY, Curves::Curve, &limits).unwrap();
        assert!(fine[0].points.len() > 3 * coarse[0].points.len());
        // The parabola's segment area is 2/3 of base times height: 2/3·8·4.
        let region = glyph_region(
            &path,
            Transform::IDENTITY,
            Point::new(0.0, 0.0),
            Reference::Curve,
            &limits,
        )
        .unwrap();
        let areas = areas(&[region], window(8, 8));
        let total: f64 = (0..8)
            .flat_map(|y| (0..8).map(move |x| (x, y)))
            .map(|(x, y)| areas.get(x, y))
            .sum();
        // The sum of control-hull areas bounds the whole outline.
        assert!((total - 64.0 / 3.0).abs() < CURVE_AREA, "{total}");
    }
}
