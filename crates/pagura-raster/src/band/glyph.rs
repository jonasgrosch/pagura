//! Glyph outlines prepared once per linear map and placed at exact origins
//! (ADR 0013).
//!
//! Only the linear map shapes the edges; each placement adds its device
//! origin to every point without rounding either to a grid. An outline is
//! integrated as quadratics when its pieces certify winding zero or one
//! sign everywhere. Uncertified quadratic outlines and placements crossed
//! by a clip path use the same flattener with a whole-outline area bound;
//! every quadratic placement certifies its coverage bytes before execution,
//! refining ambiguous pixels from the original controls. General cubics
//! retain the canonical 1/256-pixel polygon.

use super::certify::{certified, spline_certified};
use super::curve::{self, Mono, Piece, Quadratic, Spline};
use super::edges::{self, Line, Polygon};
use super::shape::{Frame, Shape, Winding};
use super::{Prep, RENDER_BYTES};
use crate::flatten::{
    CurveStack, Flatness, Inspection, Polyline, bounded, flatten_inspected, flatten_with,
};
use crate::{FillRule, Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

#[cfg(test)]
mod tests;

mod point;
mod polygon;
mod rounding;
pub(crate) use polygon::PolygonLines;

/// A glyph outline prepared once for one linear map (outline space to device
/// space without translation): its device-space edges relative to the glyph
/// origin, ready for translation and accumulation. Shareable across
/// placements, renders and pages, so it is a value for the `Store`.
#[derive(Debug)]
pub struct GlyphEdges {
    retained: usize,
    hull: Option<[f64; 4]>,
    pub(super) kind: Kind,
}

#[derive(Debug)]
pub(super) enum Kind {
    /// A certified spline: its pieces in contour order, for placements a
    /// clip's rectangle cuts, and the edges a placement translates. The
    /// outline and map stay for placements a clip path crosses, which take
    /// its polygon.
    Curve {
        spline: Spline,
        relative: Relative,
        outline: Arc<Path>,
        linear: Transform,
    },
    /// The polygon at the flattening tolerance with its geometric points'
    /// floor cells, and whether it is certified.
    Polygon {
        polygon: Polygon,
        points: Vec<Point>,
        certified: bool,
        /// Original curves retained for coverage-byte certification.
        source: Option<(Arc<Path>, Transform)>,
        /// A certified nonpoint projection. The canonical polygon remains
        /// the owner used by clipped and ordinary polygon placements.
        relative: Option<Box<Relative>>,
    },
}

/// A certified glyph's edges relative to its origin: a placement adds its
/// origin to each edge as the edge is accumulated, so placing costs no
/// geometry work and no allocation, and gives the bytes of translating
/// every point first.
#[derive(Debug)]
pub(crate) struct Relative {
    /// Edges that are not horizontal, then curves, in accumulation order.
    pub(crate) lines: Vec<Line>,
    pub(crate) curves: Vec<Mono>,
    /// The extent the pixels and the scanned columns come from.
    pub(crate) hull: [Point; 2],
    /// Four units per cell and row the edges can touch, over every row an
    /// edge can reach at any fractional origin.
    pub(crate) work: usize,
}

impl Relative {
    /// From a certified spline's pieces.
    fn new(spline: &Spline, bytes: &mut Meter) -> Result<Self> {
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut lines: Vec<Line> = Vec::new();
        let mut curves: Vec<Mono> = Vec::new();
        let line_count = spline
            .pieces
            .iter()
            .filter(|p| matches!(p, Piece::Line(line) if !line.is_horizontal()))
            .count();
        let curve_count = spline
            .pieces
            .iter()
            .filter(|p| matches!(p, Piece::Curve(_)))
            .count();
        bytes.reserve(&mut lines, line_count)?;
        bytes.reserve(&mut curves, curve_count)?;
        let mut work = 0_usize;
        for &piece in &spline.pieces {
            for p in piece.ends() {
                lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
            }
            // A curve spans its ends, as monotone pieces do; a fractional
            // origin moves an edge into at most one more row.
            let [a, b] = piece.ends();
            let chord = Line { a, b };
            match piece {
                Piece::Line(line) if !line.is_horizontal() => lines.push(line),
                Piece::Line(_) => continue,
                Piece::Curve(q) => {
                    curves.push(Mono::new(q));
                }
            }
            let rows = rows_reached((a.y.min(b.y), a.y.max(b.y)));
            work = work.saturating_add(edges::line_work(chord, rows, usize::MAX).saturating_mul(4));
        }
        Ok(Self {
            lines,
            curves,
            hull: [lo, hi],
            work,
        })
    }

    fn placed_hull(&self, origin: Point, point: Option<Point>) -> Result<[Point; 2]> {
        let [lo, hi] = self.hull;
        let mut lo = lo + origin;
        let mut hi = hi + origin;
        bounded(lo)?;
        bounded(hi)?;
        if let Some(point) = point {
            let point = point + origin;
            bounded(point)?;
            let [a, _, b, _] = Polygon::cell_corners(point);
            lo = Point::new(lo.x.min(a.x), lo.y.min(a.y));
            hi = Point::new(hi.x.max(b.x), hi.y.max(b.y));
        }
        Ok([lo, hi])
    }
}

/// The rows, counted from the edge's own first one, that an edge spanning
/// heights `(lo, hi)` relative to an origin can reach at any fractional
/// origin, as `(0, last)`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "bounded glyph extents; saturating conversion of a nonnegative span"
)]
fn rows_reached((lo, hi): (f64, f64)) -> (u32, u32) {
    let span = (hi.ceil() - lo.floor()).max(0.0);
    (0, span as u32)
}

impl GlyphEdges {
    /// Prepares `outline` under `linear` (`[a, b, c, d]` of a transform) for
    /// exact placements.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error for an outline beyond `limits` and
    /// `InvalidArgument` for a non-finite map.
    pub fn new(outline: &Path, linear: [f64; 4], limits: &Limits) -> Result<Self> {
        let [a, b, c, d] = linear;
        let transform = Transform::new(a, b, c, d, 0.0, 0.0);
        if !transform.is_finite() {
            return Err(Error::invalid_argument("glyph linear map is not finite"));
        }
        if outline.segments().len() > limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        let hull = control_hull(outline, transform);
        let mut bytes = Meter::new(limits.decoded_bytes, "glyph geometry byte budget exceeded");
        bytes.charge_items::<Self>(1)?;
        let kind = Self::geometry(outline, transform, limits, &mut bytes)?;
        Ok(Self {
            retained: bytes.used(),
            hull,
            kind,
        })
    }

    fn geometry(
        outline: &Path,
        transform: Transform,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<Kind> {
        // A singular map can make a genuine point of a curve; the polygon
        // keeps the point rule.
        let regular = transform.determinant() != 0.0;
        let spline = if regular {
            curve::spline(outline, transform, limits, bytes)?
        } else {
            None
        };
        if let Some(spline) = spline {
            if spline_certified(&spline, bytes)? {
                let relative = Relative::new(&spline, bytes)?;
                // The outline's allocation includes the Arc counters and
                // Path, whose segment vector is charged separately.
                bytes.charge(size_of::<Path>() + 2 * size_of::<usize>())?;
                let outline = Arc::new(outline.clone_with_bytes(bytes)?);
                return Ok(Kind::Curve {
                    spline,
                    relative,
                    outline,
                    linear: transform,
                });
            }
            let released = spline.retained_bytes();
            drop(spline);
            bytes.refund(released);
        }
        let (polygon, points) = polygon(outline, transform, limits, bytes)?;
        let certified = points.is_empty() && certified(&polygon, 0.0, bytes)?;
        let source_available = regular
            && outline
                .segments()
                .iter()
                .any(|s| matches!(s, Segment::CubicTo { .. }))
            && curve::quadratic_outline(outline, transform);
        let relative = if source_available && points.len() == 1 {
            point::relative(outline, transform, limits, polygon.vertices(), bytes)?
        } else {
            None
        };
        let source = if source_available {
            bytes.charge(size_of::<Path>() + 2 * size_of::<usize>())?;
            Some((Arc::new(outline.clone_with_bytes(bytes)?), transform))
        } else {
            None
        };
        Ok(Kind::Polygon {
            polygon,
            points,
            certified,
            source,
            relative,
        })
    }

    /// The bounds of the outline's points under the linear map, relative to
    /// a placement's origin, `[x0, y0, x1, y1]`: each point mapped once, in
    /// path order. `None` for an outline without points.
    pub fn hull(&self) -> Option<[f64; 4]> {
        self.hull
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// The edges a certified placement translates.
    pub(crate) fn relative(&self) -> Option<&Relative> {
        match &self.kind {
            Kind::Curve { relative, .. } => Some(relative),
            Kind::Polygon { relative, .. } => relative.as_deref(),
        }
    }

    /// Whether the stored outline admits analytic placements. A placement
    /// whose coverage byte is ambiguous takes the shared bounded polygon
    /// refinement before execution (ADR 0004 §4).
    pub fn curved(&self) -> bool {
        self.relative().is_some()
    }
}

/// The polygon of `outline` under the translation-free `transform` at the
/// flattening tolerance, and its geometric points (8.5.3.3.1).
fn polygon(
    outline: &Path,
    transform: Transform,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<(Polygon, Vec<Point>)> {
    let before = bytes.used();
    let result = flattened(outline, transform, limits, bytes);
    let retained = result.as_ref().map_or(0, |(polygon, points)| {
        polygon
            .heap_bytes()
            .saturating_add(points.capacity().saturating_mul(size_of::<Point>()))
    });
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    result
}

fn flattened(
    outline: &Path,
    transform: Transform,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<(Polygon, Vec<Point>)> {
    flattened_with(outline, transform, limits, bytes, None)
}

fn flattened_with(
    outline: &Path,
    transform: Transform,
    limits: &Limits,
    bytes: &mut Meter,
    inspection: Option<Inspection<'_>>,
) -> Result<(Polygon, Vec<Point>)> {
    let mut contours: Vec<Polyline> = Vec::new();
    let flatness = if inspection.is_some() || curve::quadratic_outline(outline, transform) {
        // Global polygons bound the hull areas over the whole outline by
        // 1/4096 pixel: less than 0.063 levels, also after clipping. Local
        // interval proofs retain that bound only where hulls meet targets;
        // disjoint hulls may keep their chords without moving target winding.
        let curves = outline
            .segments()
            .iter()
            .filter(|s| matches!(s, Segment::CubicTo { .. }))
            .count();
        Flatness::Area(1.0 / 4096.0 / f64::from(u32::try_from(curves.max(1)).unwrap_or(u32::MAX)))
    } else {
        Flatness::Distance
    };
    let mut pending = CurveStack::new();
    let count = match inspection {
        Some(inspection) => flatten_inspected(
            outline,
            transform,
            limits,
            &mut contours,
            &mut pending,
            (flatness, bytes, inspection),
        )?,
        None => flatten_with(
            outline,
            transform,
            limits,
            &mut contours,
            &mut pending,
            flatness,
            bytes,
        )?,
    };
    let released = crate::flatten::stack_bytes(&pending);
    drop(pending);
    bytes.refund(released);
    contours.truncate(count);
    classify(
        outline,
        Transform {
            e: 0.0,
            f: 0.0,
            ..transform
        },
        &mut contours,
    )?;
    let mut polygon = Polygon::default();
    let mut points = Vec::new();
    let n = contours.len();
    for (i, contour) in contours.iter().enumerate() {
        if i + 1 == n && !contour.closed && contour.points.len() == 1 {
            continue;
        }
        let Some(&first) = contour.points.first() else {
            continue;
        };
        if contour.degenerate {
            let p = first * transform;
            bounded(p)?;
            bytes.reserve_capacity(&mut points, 1)?;
            points.push(p);
            continue;
        }
        polygon.begin(limits, contour.points.len(), bytes)?;
        for &point in &contour.points {
            let p = point * transform;
            bounded(p)?;
            polygon.points.push(p);
        }
        polygon.end();
    }
    if polygon.vertices().saturating_add(4 * points.len()) > limits.path_segments {
        return Err(Error::limit("flattened segment budget exceeded"));
    }
    Ok((polygon, points))
}

/// The bounds of `outline`'s points under `transform`, as
/// [`GlyphEdges::hull`] gives them.
fn control_hull(outline: &Path, transform: Transform) -> Option<[f64; 4]> {
    let mut hull: Option<[f64; 4]> = None;
    let mut add = |p: Point| {
        let p = transform.apply(p);
        hull = Some(match hull {
            None => [p.x, p.y, p.x, p.y],
            Some([x0, y0, x1, y1]) => [x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)],
        });
    };
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => add(p),
            Segment::CubicTo { c1, c2, end } => {
                add(c1);
                add(c2);
                add(end);
            }
            Segment::Close => {}
        }
    }
    hull
}

/// Places `edges` at `origin`.
pub(crate) fn place(
    edges: &Arc<GlyphEdges>,
    origin: Point,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    let mut shape = placed(edges, origin, options)?;
    if let Some(prepared) = shape.take() {
        shape = Some(rounding::certify(edges, origin, prepared, options)?);
    }
    if let Some(shape) = &mut shape {
        shape.add_work(merged(shape.bounds()));
    }
    Ok(shape)
}

/// Work keeping a placement's rows apart in a run and merging them.
fn merged(b: super::DeviceRect) -> usize {
    (b.height() as usize).saturating_mul(2 * b.width() as usize + 2) + 1
}

fn placed(edges: &Arc<GlyphEdges>, origin: Point, options: &Prep<'_>) -> Result<Option<Shape>> {
    bounded(origin)?;
    let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
    match &edges.kind {
        Kind::Curve {
            spline,
            relative,
            outline,
            linear,
        } => {
            let Some(mut frame) = Frame::new(options) else {
                return Ok(None);
            };
            let [lo, hi] = relative.hull;
            if lo.x > hi.x {
                // No edges: nothing to cover.
                return Ok(None);
            }
            let hull = relative.placed_hull(origin, None)?;
            let [lo, hi] = hull;
            frame.fit([lo, hi]);
            match (frame.rect, frame.paths) {
                // The clip keeps it whole: its translated edges.
                (None, None) => Shape::translated(edges, relative, origin, hull, &frame, options),
                // A rectangle cuts it: its pieces, clipped.
                (Some(_), None) => pieces(spline, origin, &frame, options, &mut bytes),
                // A clip path crosses it: its polygon, resolved with the
                // clip's paths.
                (_, Some(_)) => {
                    let (polygon, points) = polygon(outline, *linear, options.limits, &mut bytes)?;
                    translated(
                        (&polygon, &points),
                        origin,
                        Winding::Check(0.0),
                        options,
                        &mut bytes,
                        None,
                        None,
                    )
                }
            }
        }
        Kind::Polygon {
            polygon,
            points,
            certified,
            relative,
            ..
        } => {
            if let (Some(relative), [point], Some(mut frame)) =
                (relative.as_deref(), points.as_slice(), Frame::new(options))
            {
                let hull = relative.placed_hull(origin, Some(*point))?;
                frame.fit(hull);
                if frame.rect.is_none() && frame.paths.is_none() {
                    return point::placed(
                        (edges, relative, *point),
                        origin,
                        hull,
                        &frame,
                        options,
                        &mut bytes,
                    );
                }
            }
            let winding = if *certified {
                Winding::Certified
            } else {
                Winding::Check(0.0)
            };
            translated(
                (polygon, points),
                origin,
                winding,
                options,
                &mut bytes,
                None,
                Some(edges),
            )
        }
    }
}

/// A placement of a certified spline through its pieces moved to `origin`
/// first, as a clip rectangle prepares them.
#[cfg(test)]
pub(crate) fn moved(
    edges: &Arc<GlyphEdges>,
    origin: Point,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
    match (&edges.kind, Frame::new(options)) {
        (Kind::Curve { spline, .. }, Some(frame)) => {
            pieces(spline, origin, &frame, options, &mut bytes)
        }
        _ => placed(edges, origin, options),
    }
}

/// A certified spline's pieces moved to `origin` and clipped to the clip's
/// rectangle.
fn pieces(
    spline: &Spline,
    origin: Point,
    frame: &Frame<'_>,
    options: &Prep<'_>,
    bytes: &mut Meter,
) -> Result<Option<Shape>> {
    let mut lines = Vec::new();
    let mut curves: Vec<Quadratic> = Vec::new();
    let line_count = spline
        .pieces
        .iter()
        .filter(|piece| matches!(piece, Piece::Line(_)))
        .count();
    bytes.reserve(&mut lines, line_count)?;
    bytes.reserve(&mut curves, spline.pieces.len() - line_count)?;
    for piece in &spline.pieces {
        match piece.translated(origin) {
            Piece::Line(line) => {
                for p in [line.a, line.b] {
                    bounded(p)?;
                }
                lines.push(line);
            }
            Piece::Curve(q) => {
                for p in q.points {
                    bounded(p)?;
                }
                curves.push(q);
            }
        }
    }
    Shape::pieces(lines, curves, frame, options, bytes)
}

/// A placement that translates a polygon's points and its geometric points
/// and prepares them as any shape.
fn translated(
    (source, points): (&Polygon, &[Point]),
    origin: Point,
    winding: Winding,
    options: &Prep<'_>,
    bytes: &mut Meter,
    work: Option<&mut Meter>,
    shared: Option<&Arc<GlyphEdges>>,
) -> Result<Option<Shape>> {
    let mut polygon = Polygon::default();
    bytes.reserve(&mut polygon.points, source.points.len())?;
    bytes.reserve(&mut polygon.starts, source.starts.len())?;
    for &p in &source.points {
        let p = Point::new(p.x + origin.x, p.y + origin.y);
        bounded(p)?;
        polygon.points.push(p);
    }
    polygon.starts.extend_from_slice(&source.starts);
    for &p in points {
        // A floor cell belongs to the placed point, not to the outline origin.
        let p = Point::new(p.x + origin.x, p.y + origin.y);
        bounded(p)?;
        polygon.cell(p, options.limits, bytes)?;
    }
    match work {
        Some(work) => {
            work.charge(polygon.vertices())?;
            Shape::polygon_metered(
                polygon,
                FillRule::NonZero,
                winding,
                options,
                bytes,
                work,
                shared.map(|edges| (edges, origin)),
            )
        }
        None => Shape::polygon(
            polygon,
            FillRule::NonZero,
            winding,
            options,
            bytes,
            shared.map(|edges| (edges, origin)),
        ),
    }
}

// The shared flattener retains source point classification. A singular linear
// map can additionally collapse all source controls to one genuine point.
// Classify before mapping the emitted vertices, so approximation never creates
// a geometric point from a nonpoint curve (8.5.3.3.1).
fn classify(outline: &Path, transform: Transform, contours: &mut [Polyline]) -> Result<()> {
    let mut index = 0;
    let mut occupied = false;
    let mut closed = false;
    for segment in outline.segments() {
        if (closed && matches!(segment, Segment::LineTo(_) | Segment::CubicTo { .. }))
            || (occupied && matches!(segment, Segment::MoveTo(_)))
        {
            index += 1;
            occupied = false;
            closed = false;
        }
        let contour = contours
            .get_mut(index)
            .ok_or(Error::invalid_argument("invalid flattened contour"))?;
        if !occupied {
            contour.degenerate = true;
        }
        let first = contour.points.first().copied().unwrap_or_default() * transform;
        let same = |point: Point| point * transform == first;
        match *segment {
            Segment::MoveTo(_) => {
                occupied = true;
                closed = false;
            }
            Segment::LineTo(point) => {
                contour.degenerate &= same(point);
                occupied = true;
            }
            Segment::CubicTo { c1, c2, end } => {
                contour.degenerate &= [c1, c2, end].into_iter().all(same);
                occupied = true;
            }
            Segment::Close => closed = true,
        }
    }
    Ok(())
}
