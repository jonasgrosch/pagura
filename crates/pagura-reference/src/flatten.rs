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
use crate::double::{Double, DoublePoint};
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

/// A flattened contour in the path's own space; a true curve's in device
/// space, its curves' inner vertices placed in double-words and every
/// other one placed in doubles.
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
        if reference == Reference::Curve {
            return p;
        }
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
    let placing = (curves == Curves::Curve).then_some(device);
    let vertex = |p: Point| placing.map_or(p, |device| place(p, device));
    let mut push = |contour: &mut Contour, point: Point| -> Result<()> {
        if !point.is_finite() || (placing.is_none() && !(point * device).is_finite()) {
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
                points: vec![vertex(start)],
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
                    push(contour, vertex(point))?;
                }
                start = point;
                position = point;
            }
            Segment::LineTo(point) => {
                if let Some(contour) = contours.last_mut() {
                    contour.degenerate &= contour.points.first().is_none_or(|&p| same(p, point));
                    push(contour, vertex(point))?;
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
        if let Some(controls @ [_, c1, c2, end]) = piece {
            if let Some(contour) = contours.last_mut() {
                contour.degenerate &= [c1, c2, end].iter().all(|&p| same(p, position));
                let mut points = Vec::new();
                flatten_curve(
                    (controls, *segment),
                    device,
                    curves,
                    allowance,
                    (&mut points, &mut hulls),
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

/// A curve's cubic controls from `from`: a quadratic flattens as its
/// elevated cubic. `None` for a move, line or close.
fn cubic(segment: Segment, from: Point) -> Option<[Point; 4]> {
    match segment {
        Segment::CubicTo { c1, c2, end } => Some([from, c1, c2, end]),
        Segment::QuadTo { control, end } => {
            let [c1, c2] = Segment::elevated(from, control, end);
            Some([from, c1, c2, end])
        }
        Segment::MoveTo(_) | Segment::LineTo(_) | Segment::Close => None,
    }
}

/// Appends the end points of the pieces of a cubic, after its start: a
/// true curve's placed, any other's within the curves' tolerance of their
/// chords.
fn flatten_curve(
    (points, segment): ([Point; 4], Segment),
    device: Transform,
    curves: Curves,
    precision: Precision,
    (out, hulls): (&mut Vec<Point>, &mut Vec<Hull>),
    limits: &Limits,
) -> Result<()> {
    if curves == Curves::Curve {
        return flatten_true_curve(
            (segment, points[0]),
            device,
            precision,
            (out, hulls),
            limits,
        );
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
/// The curve is halved in its own frame, in double-words: its controls less
/// its start, exact, with a quadratic elevated there, mapped by the linear
/// map alone and placed by adding its start placed in double-words. Each
/// coordinate of a piece's placed controls at depth `d` then lies within
///
/// ```text
/// δ(d) = 4·(d + 4)·ε²·(own + placed) + (d + 4)·MIN_POSITIVE
/// ```
///
/// of the exact one (see [`control_error`]). Its vertices are rounded to
/// doubles, and the curve's ends, like every other vertex of the outline,
/// are placed in doubles: a vertex lies within `δ(d) + 2ε·placed + tail`,
/// where `tail` is a localized pixel's two-sum tail.
///
/// A piece stops at its own flatness: its hull, outward for controls within
/// `δ(d)`, meets its share. The vertex error does not shrink with depth, so
/// it does not decide when to stop; it stays in the emitted hull's area and
/// bounds, which the pixel's interval sums as before.
fn flatten_true_curve(
    (segment, from): (Segment, Point),
    device: Transform,
    precision: Precision,
    (out, hulls): (&mut Vec<Point>, &mut Vec<Hull>),
    limits: &Limits,
) -> Result<()> {
    let Some(Frame { controls, end, own }) = Frame::new(segment, from, device) else {
        return Ok(());
    };
    let anchor = DoublePoint::placed(from, device);
    let placed = placed_extent(&controls, anchor, [from, end], device);
    let vertex = 2.0 * f64::EPSILON * placed + precision.rounding;
    let emitted = |depth: u32| control_error(depth, own, placed) + vertex;
    // The hulls of a piece's halves lie in its control box, widened by the
    // deepest pieces' errors: a piece whose widened box misses the focus
    // cannot touch its pixel, and takes the coarse budget.
    let widening = 3.0 * control_error(MAX_DEPTH, own, placed) + vertex;
    let misses = |[x0, y0, x1, y1]: [f64; 4]| {
        precision.focus.is_some_and(|focus| {
            let [left, top, right, bottom] = focus.bounds;
            x1 + widening <= left
                || x0 - widening >= right
                || y1 + widening <= top
                || y0 - widening >= bottom
        })
    };
    let coarse = precision.focus.map_or(precision.area, |focus| focus.coarse);
    let mut pending = vec![(controls, 0, (precision.area, coarse), true)];
    while let Some((piece, depth, (fine, coarse), last)) = pending.pop() {
        let corners = piece.map(|p| anchor.add(p).point());
        let bounds = corners.iter().fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |[x0, y0, x1, y1], p| [x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)],
        );
        let share = if misses(bounds) { coarse } else { fine };
        if hull_area(&piece, control_error(depth, own, placed)) <= share {
            if out.len() >= limits.path_segments {
                return Err(Error::limit("flattened segment budget exceeded"));
            }
            // The last piece ends at the curve's own end, placed as every
            // other vertex is.
            out.push(if last { place(end, device) } else { corners[3] });
            let error = emitted(depth);
            hulls.push(Hull {
                bounds: [
                    (bounds[0] - error).next_down(),
                    (bounds[1] - error).next_down(),
                    (bounds[2] + error).next_up(),
                    (bounds[3] + error).next_up(),
                ],
                area: hull_area(&piece, error),
            });
            continue;
        }
        if depth >= MAX_DEPTH {
            return Err(Error::limit("curve subdivision depth exceeded"));
        }
        let (first, second) = halve_frame(piece);
        let shares = (fine * 0.5, coarse * 0.5);
        pending.push((second, depth + 1, shares, last));
        pending.push((first, depth + 1, shares, false));
    }
    Ok(())
}

/// `p` placed by `device` in doubles, as every vertex of a true-curve
/// outline but its curves' inner ones is.
fn place(p: Point, device: Transform) -> Point {
    let q = p * Transform {
        e: 0.0,
        f: 0.0,
        ..device
    };
    Point::new(q.x + device.e, q.y + device.f)
}

/// Bounds the curve's placed coordinates and their terms: every placed
/// control, and the sums of terms of its ends placed in doubles.
fn placed_extent(
    controls: &[DoublePoint; 4],
    anchor: DoublePoint,
    ends: [Point; 2],
    device: Transform,
) -> f64 {
    controls
        .iter()
        .map(|&p| anchor.add(p).point())
        .map(|p| p.x.abs().max(p.y.abs()) * (1.0 + 4.0 * f64::EPSILON))
        .chain(ends.map(|p| magnitude(p, device)))
        .fold(0.0_f64, f64::max)
}

/// The largest absolute sum of the terms of either coordinate of `p` placed
/// by `t`: doubles placing it err by at most `2ε` of it.
fn magnitude(p: Point, t: Transform) -> f64 {
    ((p.x * t.a).abs() + (p.y * t.c).abs() + t.e.abs())
        .max((p.x * t.b).abs() + (p.y * t.d).abs() + t.f.abs())
        * (1.0 + 4.0 * f64::EPSILON)
}

/// A true curve in its own frame: its cubic controls less its start, mapped
/// by the linear map, in double-words.
struct Frame {
    controls: [DoublePoint; 4],
    end: Point,
    /// Bounds every coordinate's sum of mapped terms, `|a|·|x| + |c|·|y|`,
    /// over the frame's controls before mapping.
    own: f64,
}

impl Frame {
    /// `None` for a move, line or close.
    fn new(segment: Segment, from: Point, device: Transform) -> Option<Self> {
        let frame = |p: Point| DoublePoint::difference(p, from);
        let (unmapped, end) = match segment {
            Segment::CubicTo { c1, c2, end } => {
                ([DoublePoint::ZERO, frame(c1), frame(c2), frame(end)], end)
            }
            // Elevated in the frame: c1 = 2/3·q1 and c2 = q2 + 2/3·(q1 − q2).
            Segment::QuadTo { control, end } => {
                let (q1, q2) = (frame(control), frame(end));
                let third = Double::two_thirds();
                let c1 = q1.scale(third);
                let c2 = q2.add(q1.sub(q2).scale(third));
                ([DoublePoint::ZERO, c1, c2, q2], end)
            }
            Segment::MoveTo(_) | Segment::LineTo(_) | Segment::Close => return None,
        };
        let largest = |coordinate: fn(&DoublePoint) -> Double| {
            unmapped
                .iter()
                .map(|p| coordinate(p).value().abs())
                .fold(0.0_f64, f64::max)
                * (1.0 + 4.0 * f64::EPSILON)
        };
        let (x, y) = (largest(|p| p.x), largest(|p| p.y));
        let own = ((device.a.abs() * x + device.c.abs() * y)
            .max(device.b.abs() * x + device.d.abs() * y))
            * (1.0 + 4.0 * f64::EPSILON);
        Some(Self {
            controls: unmapped.map(|p| p.mapped(device)),
            end,
            own,
        })
    }
}

/// The per-coordinate error of a piece's controls at `depth`, placed or in
/// the frame, in units of `u = ε/2` (see [`crate::double`]):
///
/// - the frame's differences are exact; elevating a quadratic errs by at
///   most `19u²` of its largest frame coordinate, and the linear map adds
///   `5u²` of the sum of mapped terms: `24u²·own` in all;
/// - each halving forms three rows of midpoints, `3u²·own` each, as the
///   pieces' coordinates lie within the frame's hull: `9u²·own` a depth;
/// - placing the start errs by `6u²·placed`, and adding it to a control by
///   `3u²·placed` more;
/// - a result below `MIN_POSITIVE` errs by at most that much an operation.
///
/// `(24 + 9d)·u²·own + 9u²·placed` is at most `4·(d + 4)·ε²·(own + placed)`.
fn control_error(depth: u32, own: f64, placed: f64) -> f64 {
    let depth = f64::from(depth + 4);
    4.0 * depth * f64::EPSILON * f64::EPSILON * (own + placed) + depth * 8.0 * f64::MIN_POSITIVE
}

/// The area of a piece's control hull, outward for controls within `error`
/// of these and for the rounding of the differences to its first control.
fn hull_area([a, b, c, d]: &[DoublePoint; 4], error: f64) -> f64 {
    let relative = |p: &DoublePoint| p.sub(*a).point();
    let (b, c, d) = (relative(b), relative(c), relative(d));
    let triangle = |u: Point, v: Point| {
        let products = (u.x * v.y).abs() + (u.y * v.x).abs();
        let determinant = (u.x * v.y - u.y * v.x).abs()
            + 16.0 * f64::EPSILON * products
            + 2.0 * error * (u.x.abs() + u.y.abs() + v.x.abs() + v.y.abs())
            + 8.0 * error * error;
        determinant * (0.5 + 16.0 * f64::EPSILON) + f64::MIN_POSITIVE
    };
    (triangle(b, c) + triangle(b, d) + triangle(c, d)) * (1.0 + 16.0 * f64::EPSILON)
}

/// The two halves of a piece in its frame.
fn halve_frame([a, b, c, d]: [DoublePoint; 4]) -> ([DoublePoint; 4], [DoublePoint; 4]) {
    let (ab, bc, cd) = (a.midpoint(b), b.midpoint(c), c.midpoint(d));
    let (abc, bcd) = (ab.midpoint(bc), bc.midpoint(cd));
    let m = abc.midpoint(bcd);
    ([a, ab, abc, m], [m, bcd, cd, d])
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
    use crate::double::rational::Rational;
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

    /// Exact images of a frame's controls and the curve's placed ends.
    struct Exact {
        frame: [[Rational; 2]; 4],
        anchor: [Rational; 2],
        end: [Rational; 2],
    }

    impl Exact {
        fn new(segment: Segment, from: Point, t: Transform) -> Self {
            let exact = |p: Point| [Rational::from_f64(p.x), Rational::from_f64(p.y)];
            let difference = |p: Point| {
                let [x, y] = exact(p);
                let [fx, fy] = exact(from);
                [x.sub(&fx), y.sub(&fy)]
            };
            let zero = || [Rational::from_f64(0.0), Rational::from_f64(0.0)];
            let (unmapped, end) = match segment {
                Segment::CubicTo { c1, c2, end } => (
                    [zero(), difference(c1), difference(c2), difference(end)],
                    end,
                ),
                Segment::QuadTo { control, end } => {
                    let (q1, q2) = (difference(control), difference(end));
                    let c1 = [q1[0].two_thirds(), q1[1].two_thirds()];
                    let c2 = [0, 1].map(|k| q2[k].add(&q1[k].sub(&q2[k]).two_thirds()));
                    ([zero(), c1, c2, q2], end)
                }
                _ => unreachable!(),
            };
            let linear = |[x, y]: &[Rational; 2]| {
                [
                    x.times(t.a).add(&y.times(t.c)),
                    x.times(t.b).add(&y.times(t.d)),
                ]
            };
            let placed = |p: Point| {
                let [x, y] = exact(p);
                [
                    x.times(t.a)
                        .add(&y.times(t.c))
                        .add(&Rational::from_f64(t.e)),
                    x.times(t.b)
                        .add(&y.times(t.d))
                        .add(&Rational::from_f64(t.f)),
                ]
            };
            Self {
                frame: unmapped.each_ref().map(linear),
                anchor: placed(from),
                end: placed(end),
            }
        }
    }

    fn exact_halves(piece: &[[Rational; 2]; 4]) -> ([[Rational; 2]; 4], [[Rational; 2]; 4]) {
        let mid = |p: &[Rational; 2], q: &[Rational; 2]| [0, 1].map(|k| p[k].add(&q[k]).half());
        let [a, b, c, d] = piece;
        let (ab, bc, cd) = (mid(a, b), mid(b, c), mid(c, d));
        let (abc, bcd) = (mid(&ab, &bc), mid(&bc, &cd));
        let m = mid(&abc, &bcd);
        (
            [a.clone(), ab, abc.clone(), m.clone()],
            [m, bcd, cd, d.clone()],
        )
    }

    #[test]
    fn frames_stay_within_their_bound_of_exact_arithmetic() {
        // Random curves, near and far from the origin, with near-cancelling
        // maps and placements and controls a few ulps apart, followed down
        // a random path of halvings to the deepest piece.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut checked = 0;
        for case in 0..160_u32 {
            let mut unit =
                || f64::from(u32::try_from(next() >> 40).unwrap()) / f64::from(1_u32 << 24) - 0.5;
            let scale = [1.0, 2_500.0, 1e6, 2.0_f64.powi(40)][(case % 4) as usize];
            let size = [1e-9, 1e-3, 1.0, 300.0][((case / 4) % 4) as usize];
            let from = Point::new(scale * unit() + 1.0 / 3.0, scale * unit() - 2.0 / 7.0);
            let near = |k: f64, u: f64, v: f64| {
                if case % 7 == 0 {
                    // A few ulps from the start: frame differences of a few bits.
                    let ulp = |x: f64| f64::from_bits(x.to_bits() + 1) - x;
                    Point::new(
                        from.x + k * ulp(from.x.abs()),
                        from.y - k * ulp(from.y.abs()),
                    )
                } else {
                    Point::new(from.x + size * u, from.y + size * v)
                }
            };
            let (c1, c2, end) = (
                near(1.0, unit(), unit()),
                near(2.0, unit(), unit()),
                near(3.0, unit(), unit()),
            );
            let segment = if case % 2 == 0 {
                Segment::QuadTo { control: c1, end }
            } else {
                Segment::CubicTo { c1, c2, end }
            };
            let (a, b, c, d) = (2.0 * unit(), unit() / 3.0, unit() / 7.0, 2.0 * unit());
            // Placements that cancel the start's image to a few pixels.
            let cancel = case % 3 == 0;
            let e = if cancel {
                -(from.x * a + from.y * c) + unit()
            } else {
                1e4 * unit()
            };
            let f = if cancel {
                -(from.x * b + from.y * d) + unit()
            } else {
                1e4 * unit()
            };
            let device = Transform { a, b, c, d, e, f };
            let frame = Frame::new(segment, from, device).unwrap();
            let exact = Exact::new(segment, from, device);
            let anchor = DoublePoint::placed(from, device);
            let placed = placed_extent(&frame.controls, anchor, [from, frame.end], device);
            let vertex = 2.0 * f64::EPSILON * placed;
            // The curve's end, placed in doubles as every outline vertex is.
            let end = place(frame.end, device);
            assert!(exact.end[0].within(end.x, vertex), "end, case {case}");
            assert!(exact.end[1].within(end.y, vertex), "end, case {case}");
            let (mut piece, mut truth) = (frame.controls, exact.frame);
            for depth in 0..=MAX_DEPTH {
                let error = control_error(depth, frame.own, placed);
                for (p, q) in piece.iter().zip(&truth) {
                    let coordinates = [p.x, p.y];
                    let placed_point = anchor.add(*p);
                    let placed_coordinates = [placed_point.x, placed_point.y];
                    let rounded = placed_point.point();
                    for k in 0..2 {
                        let exact_placed = q[k].add(&exact.anchor[k]);
                        assert!(
                            q[k].within_double(coordinates[k].hi, coordinates[k].lo, error),
                            "frame, case {case} depth {depth}"
                        );
                        assert!(
                            exact_placed.within_double(
                                placed_coordinates[k].hi,
                                placed_coordinates[k].lo,
                                error
                            ),
                            "placed, case {case} depth {depth}"
                        );
                        assert!(
                            exact_placed.within([rounded.x, rounded.y][k], error + vertex),
                            "vertex, case {case} depth {depth}"
                        );
                        checked += 1;
                    }
                }
                let (first, second) = halve_frame(piece);
                let (exact_first, exact_second) = exact_halves(&truth);
                (piece, truth) = if next() % 2 == 0 {
                    (first, exact_first)
                } else {
                    (second, exact_second)
                };
            }
        }
        assert_eq!(checked, 160 * (u32_to_usize(MAX_DEPTH) + 1) * 8);
    }
}
