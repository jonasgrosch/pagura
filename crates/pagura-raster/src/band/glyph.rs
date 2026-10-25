//! Glyph outlines prepared once per linear map and placed at exact origins
//! (ADR 0013).
//!
//! Only the linear map shapes the edges; each placement adds its device
//! origin to every point without rounding either to a grid. An outline is
//! integrated as quadratics when its pieces certify winding zero or one
//! sign everywhere, with the floor cell of one explicit closed move point
//! when it has one. Uncertified quadratic outlines and placements crossed
//! by a clip path use the same flattener with a whole-outline area bound;
//! an uncertified spline's overlaps are resolved once into the boundary of
//! its covered set, kept as parts of its own curves, for every placement no
//! clip cuts, and failing that an uncertified polygon's.
//! Every placement that approximates its quadratics, as a polygon or by
//! reducing degree-elevated cubics, certifies its coverage bytes before
//! execution, refining ambiguous pixels from the original controls. General
//! cubics retain the canonical 1/256-pixel polygon.

use super::certify::{covered, doubtful, spline_certified};
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

mod overlap;
mod polygon;
mod rounding;
pub(super) use polygon::Summary;
pub(crate) use polygon::{PolygonLines, point_sides};

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
    /// clip's rectangle cuts, and the edges a placement translates, with
    /// the one geometric point of an explicit closed move, whose floor cell
    /// a placement covers (8.5.3.3.1). The outline and map stay for
    /// placements a clip path crosses, or a rectangle cuts with a point,
    /// which take its polygon.
    Curve {
        spline: Spline,
        relative: Relative,
        point: Option<Point>,
        source: Arc<rounding::Proof>,
    },
    /// An uncertified spline's covered set: the parts of its chains where
    /// the winding changes between zero and not zero, turned so that it
    /// winds one, and the area by which they can miss it. Every placement
    /// certifies its bytes against the outline; one a clip cuts takes the
    /// outline's polygon.
    Union {
        relative: Relative,
        allowance: f64,
        source: Arc<rounding::Proof>,
    },
    /// The polygon at the flattening tolerance with its geometric points'
    /// floor cells, and whether it is certified.
    Polygon {
        polygon: Polygon,
        points: Vec<Point>,
        state: PolygonState,
    },
}

/// The polygon's placement proof. Resolved sides always retain the
/// original curves that certify their coverage; a summary belongs only
/// to a polygon with certified winding.
#[derive(Debug)]
pub(super) enum PolygonState {
    Certified {
        summary: Option<Summary>,
        source: Option<Arc<rounding::Proof>>,
    },
    Checked {
        source: Option<Arc<rounding::Proof>>,
    },
    Resolved {
        relative: Box<Relative>,
        source: Arc<rounding::Proof>,
    },
}

impl PolygonState {
    fn source(&self) -> Option<&rounding::Proof> {
        match self {
            Self::Certified { source, .. } | Self::Checked { source } => source.as_deref(),
            Self::Resolved { source, .. } => Some(source),
        }
    }

    fn relative(&self) -> Option<&Relative> {
        match self {
            Self::Resolved { relative, .. } => Some(relative),
            _ => None,
        }
    }

    fn summary(&self) -> Option<Summary> {
        match self {
            Self::Certified { summary, .. } => *summary,
            _ => None,
        }
    }

    fn winding(&self) -> Winding {
        if matches!(self, Self::Certified { .. }) {
            Winding::Certified
        } else {
            Winding::Check(0.0)
        }
    }
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

    /// From the parts of a covered set: their extent includes each
    /// curve's whole x range.
    fn parts(lines: Vec<Line>, curves: Vec<Mono>) -> Self {
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut work = 0_usize;
        let chords = lines.iter().copied().chain(curves.iter().map(|m| Line {
            a: Point::new(m.x0, m.top),
            b: Point::new(m.x1, m.bottom),
        }));
        for chord in chords {
            for p in [chord.a, chord.b] {
                lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
            }
            let rows = rows_reached((chord.a.y.min(chord.b.y), chord.a.y.max(chord.b.y)));
            work = work.saturating_add(edges::line_work(chord, rows, usize::MAX).saturating_mul(4));
        }
        Self {
            lines,
            curves,
            hull: [lo, hi],
            work,
        }
    }

    fn retained_bytes(&self) -> usize {
        self.lines.capacity() * size_of::<Line>() + self.curves.capacity() * size_of::<Mono>()
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
        let curves = outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. } | Segment::QuadTo { .. }));
        let spline = if regular {
            curve::spline(outline, transform, limits, bytes)?
        } else {
            None
        };
        let spline = match spline {
            Some(spline) => Some((spline, None)),
            // A curved outline's pieces without the one point of an
            // explicit closed move.
            None if regular && curves => curve::point_spline(outline, transform, limits, bytes)?
                .map(|(spline, point)| (spline, Some(point))),
            None => None,
        };
        if let Some((spline, point)) = spline {
            // The certificate reads the curves the placements integrate.
            let relative = Relative::new(&spline, bytes)?;
            if spline_certified(&spline, &relative.curves, bytes)? {
                let source = rounding::Proof::new(outline, transform, Some(&relative), bytes)?;
                return Ok(Kind::Curve {
                    spline,
                    relative,
                    point,
                    source,
                });
            }
            // Its covered set where every placement certifies its bytes
            // against the outline's curves, unless it has a point cell.
            let union = if point.is_none() && curves && curve::quadratic_outline(outline, transform)
            {
                covered(&spline, &relative.curves, bytes)?
            } else {
                None
            };
            let released = spline.retained_bytes() + relative.retained_bytes();
            drop((spline, relative));
            bytes.refund(released);
            if let Some(union) = union {
                return Ok(Kind::Union {
                    relative: Relative::parts(union.lines, union.curves),
                    allowance: union.allowance,
                    source: rounding::Proof::new(outline, transform, None, bytes)?,
                });
            }
        }
        Self::polygon_kind(outline, transform, limits, bytes)
    }

    /// The polygon of an outline no spline represents, with an uncertified
    /// short polygon's overlaps resolved once where its placements certify
    /// their bytes.
    fn polygon_kind(
        outline: &Path,
        transform: Transform,
        limits: &Limits,
        bytes: &mut Meter,
    ) -> Result<Kind> {
        let regular = transform.determinant() != 0.0;
        let curves = outline
            .segments()
            .iter()
            .any(|s| matches!(s, Segment::CubicTo { .. } | Segment::QuadTo { .. }));
        let (polygon, points) = polygon(outline, transform, limits, bytes)?;
        let doubtful = if points.is_empty() {
            doubtful(&polygon, 0.0, bytes)?
        } else {
            None
        };
        let certified = doubtful.as_ref().is_some_and(|d| d.slabs.is_empty());
        let summary = if certified {
            Summary::new(&polygon)
        } else {
            None
        };
        let source_available = regular && curves && curve::quadratic_outline(outline, transform);
        // Other sides than the polygon's only for placements that certify
        // their bytes.
        let resolved = match &doubtful {
            Some(doubtful)
                if source_available
                    && !certified
                    && polygon.bounds().is_some_and(Summary::short) =>
            {
                overlap::resolved(&polygon, doubtful, limits, bytes)
            }
            _ => None,
        };
        if let Some(doubtful) = doubtful {
            bytes.refund(doubtful.slabs.capacity() * size_of::<[f64; 2]>());
        }
        let source = if source_available {
            Some(rounding::Proof::new(outline, transform, None, bytes)?)
        } else {
            None
        };
        let state = if certified {
            PolygonState::Certified { summary, source }
        } else {
            match (resolved, source) {
                (Some(relative), Some(source)) => PolygonState::Resolved { relative, source },
                (_, source) => PolygonState::Checked { source },
            }
        };
        Ok(Kind::Polygon {
            polygon,
            points,
            state,
        })
    }

    fn source(&self) -> Option<&rounding::Proof> {
        match &self.kind {
            Kind::Curve { source, .. } | Kind::Union { source, .. } => Some(source),
            Kind::Polygon { state, .. } => state.source(),
        }
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

    /// The edges a certified or resolved placement translates.
    pub(crate) fn relative(&self) -> Option<&Relative> {
        match &self.kind {
            Kind::Curve { relative, .. } | Kind::Union { relative, .. } => Some(relative),
            Kind::Polygon { state, .. } => state.relative(),
        }
    }

    /// The geometric point whose floor cell a placement of those edges
    /// covers, relative to its origin.
    pub(crate) fn point(&self) -> Option<Point> {
        match &self.kind {
            Kind::Curve { point, .. } => *point,
            Kind::Union { .. } | Kind::Polygon { .. } => None,
        }
    }

    /// Whether the stored outline admits analytic placements. A placement
    /// whose coverage byte is ambiguous takes the shared bounded polygon
    /// refinement before execution (ADR 0004 §4).
    pub fn curved(&self) -> bool {
        matches!(self.kind, Kind::Curve { .. } | Kind::Union { .. })
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

/// The area allowance of a glyph polygon, in square pixels, over the whole
/// outline or, where its bytes are certified, over a pixel that meets at
/// most [`SHARING`] curves.
pub(super) const AREA: f64 = 1.0 / 4096.0;

/// The curves a byte-certified polygon shares [`AREA`] among: a pixel that
/// meets more is certified from its curves' shares instead.
const SHARING: usize = 8;

/// An outline's curves and each one's share of the polygon's area
/// allowance, the area its accepted hulls can cover together: [`AREA`]
/// divided among every curve, or among at most [`SHARING`] where every
/// placement certifies its bytes.
fn area_share(outline: &Path, certified: bool) -> (usize, f64) {
    let curves = outline
        .segments()
        .iter()
        .filter(|s| matches!(s, Segment::CubicTo { .. } | Segment::QuadTo { .. }))
        .count()
        .max(1);
    let sharing = if certified {
        curves.min(SHARING)
    } else {
        curves
    };
    (
        curves,
        AREA / f64::from(u32::try_from(sharing).unwrap_or(u32::MAX)),
    )
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
        // Under a regular map, every placement of a polygon with curves
        // certifies its bytes from the original outline.
        let certified = transform.determinant() != 0.0;
        Flatness::Area(area_share(outline, certified).1)
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
    // A quadratic counts its elevated controls, as the cubic it once was.
    let (mut start, mut current) = (Point::default(), Point::default());
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                add(p);
                (start, current) = (p, p);
            }
            Segment::LineTo(p) => {
                add(p);
                current = p;
            }
            Segment::CubicTo { c1, c2, end } => {
                add(c1);
                add(c2);
                add(end);
                current = end;
            }
            Segment::QuadTo { control, end } => {
                let [c1, c2] = Segment::elevated(current, control, end);
                add(c1);
                add(c2);
                add(end);
                current = end;
            }
            Segment::Close => current = start,
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
            point,
            source,
        } => {
            let Some(mut frame) = Frame::new(options) else {
                return Ok(None);
            };
            let [lo, hi] = relative.hull;
            if lo.x > hi.x {
                // No edges: nothing to cover.
                return Ok(None);
            }
            let hull = relative.placed_hull(origin, *point)?;
            let [lo, hi] = hull;
            frame.fit([lo, hi]);
            match (frame.rect, frame.paths, point) {
                // The clip keeps it whole: its translated edges.
                (None, None, _) => {
                    Shape::translated(edges, relative, origin, hull, &frame, options)
                }
                // A rectangle cuts it: its pieces, clipped.
                (Some(_), None, None) => pieces(spline, origin, &frame, options, &mut bytes),
                // A clip path crosses it, or a rectangle cuts it with its
                // point: its polygon, resolved with the clip.
                _ => clipped(
                    (&source.outline, source.linear),
                    origin,
                    options,
                    &mut bytes,
                ),
            }
        }
        Kind::Union {
            relative, source, ..
        } => union_placed(
            edges,
            relative,
            (&source.outline, source.linear),
            origin,
            options,
        ),
        Kind::Polygon {
            polygon,
            points,
            state,
        } => {
            if let Some(relative) = state.relative() {
                let hull = relative.placed_hull(origin, None)?;
                let Some(mut frame) = Frame::new(options) else {
                    return Ok(None);
                };
                frame.fit(hull);
                // A window visits every side of an unlisted placement, as
                // it does a short polygon's.
                if Summary::short(hull) && frame.rect.is_none() && frame.paths.is_none() {
                    return Shape::translated(edges, relative, origin, hull, &frame, options);
                }
            }
            if let Some(summary) = state.summary() {
                let hull = summary.placed_hull(origin)?;
                if Summary::short(hull) {
                    let Some(mut frame) = Frame::new(options) else {
                        return Ok(None);
                    };
                    if frame.paths.is_none() {
                        frame.fit(hull);
                        if frame.rect.is_none() {
                            let shape = Shape::polygon_glyph(
                                edges, summary, origin, hull, &frame, options,
                            )?;
                            if let Some(shape) = &shape {
                                bytes.charge(shape.retained_bytes())?;
                            }
                            return Ok(shape);
                        }
                    }
                }
            }
            let winding = state.winding();
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

/// A placement of a covered set: its translated parts where the clip keeps
/// it whole, else the outline's polygon, resolved with the clip.
fn union_placed(
    edges: &Arc<GlyphEdges>,
    relative: &Relative,
    source: (&Path, Transform),
    origin: Point,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    let Some(mut frame) = Frame::new(options) else {
        return Ok(None);
    };
    let [lo, hi] = relative.hull;
    if lo.x > hi.x {
        // No parts: nothing to cover.
        return Ok(None);
    }
    let hull = relative.placed_hull(origin, None)?;
    frame.fit(hull);
    if frame.rect.is_none() && frame.paths.is_none() {
        return Shape::translated(edges, relative, origin, hull, &frame, options);
    }
    let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
    clipped(source, origin, options, &mut bytes)
}

/// A placement of the polygon of `outline` under `linear` moved to
/// `origin`, resolved with the clip.
fn clipped(
    (outline, linear): (&Path, Transform),
    origin: Point,
    options: &Prep<'_>,
    bytes: &mut Meter,
) -> Result<Option<Shape>> {
    let (polygon, points) = polygon(outline, linear, options.limits, bytes)?;
    translated(
        (&polygon, &points),
        origin,
        Winding::Check(0.0),
        options,
        bytes,
        None,
        None,
    )
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
    let (mut start, mut current) = (Point::default(), Point::default());
    for segment in outline.segments() {
        let reopens = matches!(
            segment,
            Segment::LineTo(_) | Segment::CubicTo { .. } | Segment::QuadTo { .. }
        );
        if (closed && reopens) || (occupied && matches!(segment, Segment::MoveTo(_))) {
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
            Segment::MoveTo(point) => {
                occupied = true;
                closed = false;
                (start, current) = (point, point);
            }
            Segment::LineTo(point) => {
                contour.degenerate &= same(point);
                occupied = true;
                current = point;
            }
            Segment::CubicTo { c1, c2, end } => {
                contour.degenerate &= [c1, c2, end].into_iter().all(same);
                occupied = true;
                current = end;
            }
            Segment::QuadTo { control, end } => {
                let [c1, c2] = Segment::elevated(current, control, end);
                contour.degenerate &= [c1, c2, end].into_iter().all(same);
                occupied = true;
                current = end;
            }
            Segment::Close => {
                closed = true;
                current = start;
            }
        }
    }
    Ok(())
}
