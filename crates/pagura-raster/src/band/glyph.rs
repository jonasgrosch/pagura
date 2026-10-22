//! Glyph outlines prepared once per linear map and placed at exact origins
//! (ADR 0013), or reused per origin bin under the `glyph_origin_bins` fast
//! parameter.
//!
//! Only the linear map participates in flattening; each placement adds its
//! device origin to every vertex without rounding either to a grid. A small
//! TrueType outline keeps its quadratics and is integrated against the true
//! curve where its contours are certified at least two flattening
//! tolerances apart, so that the curves cannot meet either.

use super::binned;
use super::certify::certified;
use super::curve::{self, Piece, Quadratic};
use super::edges::{self, Bounds, Line, Polygon, RowList};
use super::rect::rectangle;
use super::shape::{Frame, Shape, Winding};
use super::{GlyphOrigins, Precision, Prep};
use crate::flatten::{CurveStack, Polyline, TOLERANCE, bounded, flatten_into};
use crate::{FillRule, Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

/// A glyph outline prepared once for one linear map (outline space to device
/// space without translation): its device-space edges relative to the glyph
/// origin, ready for translation and accumulation. Shareable across
/// placements, renders and pages, so it is a value for the `Store`.
#[derive(Debug)]
pub struct GlyphEdges {
    retained: usize,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Exact(Exact),
    Binned(binned::Prepared),
}

#[derive(Debug)]
struct Exact {
    polygon: Polygon,
    /// Geometric points, whose floor cells are taken once placed.
    points: Vec<Point>,
    certified: bool,
    /// Lines and monotone quadratics, when the outline has them and its
    /// contours stay apart.
    curves: Option<Vec<Piece>>,
    /// The edges a certified placement accumulates, ready for translation.
    relative: Option<Relative>,
}

/// A certified glyph's edges relative to its origin, listed per row for any
/// origin within one pixel row: a placement adds its origin to each edge as
/// the edge is accumulated, so placing costs no geometry work and no
/// allocation, and gives the bytes of translating every vertex first.
#[derive(Debug)]
pub(crate) struct Relative {
    /// Edges that are not horizontal, then curves, in accumulation order.
    pub(crate) lines: Vec<Line>,
    pub(crate) curves: Vec<Quadratic>,
    /// Items by row, from relative row `first`; an item is listed one row
    /// further than it reaches at a whole origin, for fractional ones.
    pub(crate) index: RowList,
    pub(crate) first: i64,
    /// The extent the pixels and the scanned columns come from.
    pub(crate) hull: [Point; 2],
    /// An axis-aligned rectangle, filled in closed form.
    pub(crate) rect: Option<Bounds>,
    /// Edge visits and four units per cell they can touch, over all rows.
    pub(crate) work: usize,
}

impl Relative {
    /// From every line (horizontal ones bound the extent only) and curve.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded coordinates; rows are checked to fit u32"
    )]
    fn new(all: &[Line], curves: Vec<Quadratic>) -> Result<Self> {
        let allocation = |_| Error::limit("glyph geometry allocation failed");
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let ends = all
            .iter()
            .flat_map(|l| [l.a, l.b])
            .chain(curves.iter().flat_map(|q| [q.points[0], q.points[2]]));
        for p in ends {
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let mut lines: Vec<Line> = Vec::new();
        lines.try_reserve_exact(all.len()).map_err(allocation)?;
        lines.extend(all.iter().filter(|l| !l.is_horizontal()));
        let span = |i: usize| -> (f64, f64) {
            let (a, b) = match lines.get(i) {
                Some(l) => (l.a.y, l.b.y),
                None => {
                    let q = curves[i - lines.len()];
                    (q.points[0].y, q.points[2].y)
                }
            };
            (a.min(b), a.max(b))
        };
        let count = lines.len() + curves.len();
        let (first, last) = (0..count).fold((f64::INFINITY, f64::NEG_INFINITY), |(f, l), i| {
            let (a, b) = span(i);
            (f.min(a.floor()), l.max(b.ceil()))
        });
        let (first, rows) = if count == 0 {
            (0, 0)
        } else {
            let rows = u32::try_from(last as i64 - first as i64 + 1)
                .map_err(|_| Error::limit("glyph extent budget exceeded"))?;
            (first as i64, rows)
        };
        let relative = |i: usize| {
            let (a, b) = span(i);
            (
                (a.floor() as i64 - first) as u32,
                (b.ceil() as i64 - first) as u32,
            )
        };
        let index = RowList::new((0, rows), count, |i| Some(relative(i)))?;
        let mut work = index.len();
        for i in 0..count {
            let line = match lines.get(i) {
                Some(&l) => l,
                None => {
                    let q = curves[i - lines.len()];
                    Line {
                        a: q.points[0],
                        b: q.points[2],
                    }
                }
            };
            work = work
                .saturating_add(edges::line_work(line, relative(i), usize::MAX).saturating_mul(4));
        }
        Ok(Self {
            lines,
            curves,
            index,
            first,
            hull: [lo, hi],
            rect: None,
            work,
        })
    }

    fn retained_bytes(&self) -> usize {
        (self.lines.capacity() * size_of::<Line>())
            .saturating_add(self.curves.capacity() * size_of::<Quadratic>())
            .saturating_add(self.index.retained_bytes())
    }
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
        let mut contours: Vec<Polyline> = Vec::new();
        let count = flatten_into(
            outline,
            transform,
            limits,
            &mut contours,
            &mut CurveStack::new(),
        )?;
        contours.truncate(count);
        classify(outline, transform, &mut contours)?;
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
                points
                    .try_reserve(1)
                    .map_err(|_| Error::limit("glyph geometry allocation failed"))?;
                points.push(p);
                continue;
            }
            polygon.begin(limits, contour.points.len())?;
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
        let certified_polygon = points.is_empty() && certified(&polygon, 0.0);
        let vertices: usize = contours.iter().map(|c| c.points.len()).sum();
        let curves = if outline.segments().len() > 32
            || contours.iter().any(|c| c.degenerate)
            || !outline
                .segments()
                .iter()
                .any(|s| matches!(s, Segment::CubicTo { .. }))
        {
            None
        } else {
            // At most 32 pieces, and fewer than a quarter of the polygon's
            // vertices.
            let cap = (vertices.saturating_sub(1) / 4).min(32);
            curve::pieces(outline, transform, cap, limits)?
                .filter(|_| certified(&polygon, 2.0 * TOLERANCE))
        };
        let relative = if let Some(pieces) = &curves {
            let mut lines = Vec::new();
            let mut quadratics = Vec::new();
            for piece in pieces {
                match *piece {
                    Piece::Line(line) => lines.push(line),
                    Piece::Curve(q) => quadratics.push(q),
                }
            }
            Some(Relative::new(&lines, quadratics)?)
        } else if certified_polygon && !polygon.points_cells {
            let mut relative = Relative::new(&edges::lines(&polygon)?, Vec::new())?;
            relative.rect = rectangle(&polygon);
            Some(relative)
        } else {
            None
        };
        let retained = size_of::<Self>()
            .saturating_add(polygon.points.capacity() * size_of::<Point>())
            .saturating_add(polygon.starts.capacity() * size_of::<usize>())
            .saturating_add(
                curves
                    .as_ref()
                    .map_or(0, |c| c.capacity() * size_of::<Piece>()),
            )
            .saturating_add(relative.as_ref().map_or(0, Relative::retained_bytes));
        if retained > limits.decoded_bytes {
            return Err(Error::limit("glyph geometry byte budget exceeded"));
        }
        Ok(Self {
            retained,
            kind: Kind::Exact(Exact {
                polygon,
                points,
                certified: certified_polygon,
                curves,
                relative,
            }),
        })
    }

    /// Prepares `outline` under `linear` for placements whose origins snap
    /// to bins (the `glyph_origin_bins` fast parameter). `scale` is the
    /// render's device pixels per default user-space unit (1.0 at 72 dpi),
    /// which chooses the bins and the flattening.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn binned(outline: &Path, linear: [f64; 4], scale: f64, limits: &Limits) -> Result<Self> {
        let prepared = binned::Prepared::new(outline, linear, scale, limits)?;
        Ok(Self {
            retained: size_of::<Self>().saturating_add(prepared.retained_bytes()),
            kind: Kind::Binned(prepared),
        })
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// The edges a certified exact placement translates.
    pub(crate) fn relative(&self) -> Option<&Relative> {
        match &self.kind {
            Kind::Exact(exact) => exact.relative.as_ref(),
            Kind::Binned(_) => None,
        }
    }

    /// The binned preparation, if the edges were prepared for bins.
    pub(crate) fn prepared(&self) -> Option<&binned::Prepared> {
        match &self.kind {
            Kind::Binned(prepared) => Some(prepared),
            Kind::Exact(_) => None,
        }
    }
}

/// Places `edges` at `origin`.
pub(crate) fn place(
    edges: &Arc<GlyphEdges>,
    origin: Point,
    origins: GlyphOrigins,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    bounded(origin)?;
    let exact = match &edges.kind {
        Kind::Binned(prepared) => {
            let _ = origins;
            return binned::place(edges, prepared, origin, options);
        }
        Kind::Exact(exact) => exact,
    };
    if options.precision == Precision::Exact
        && let Some(relative) = &exact.relative
    {
        let Some(mut frame) = Frame::new(options) else {
            return Ok(None);
        };
        let [lo, hi] = relative.hull;
        if lo.x > hi.x {
            // No edges: nothing to cover.
            return Ok(None);
        }
        let lo = Point::new(lo.x + origin.x, lo.y + origin.y);
        let hi = Point::new(hi.x + origin.x, hi.y + origin.y);
        bounded(lo)?;
        bounded(hi)?;
        frame.fit([lo, hi]);
        if frame.rows.is_none() && frame.rect.is_none() {
            return Shape::translated(edges, relative, origin, &frame, options);
        }
    }
    general(exact, origin, options)
}

/// [`place`] translating every vertex first, for exact edges.
#[cfg(test)]
pub(crate) fn place_general(
    edges: &GlyphEdges,
    origin: Point,
    options: &Prep<'_>,
) -> Result<Option<Shape>> {
    match &edges.kind {
        Kind::Exact(exact) => general(exact, origin, options),
        Kind::Binned(_) => Ok(None),
    }
}

/// A placement that translates the outline's vertices and prepares them as
/// any shape: where the clip cuts the glyph or the glyph is not certified.
fn general(exact: &Exact, origin: Point, options: &Prep<'_>) -> Result<Option<Shape>> {
    if options.precision == Precision::Exact
        && let Some(pieces) = &exact.curves
    {
        let Some(mut frame) = Frame::new(options) else {
            return Ok(None);
        };
        // Monotone pieces lie within the boxes of their ends.
        let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for piece in pieces {
            for p in piece.translated(origin).ends() {
                bounded(p)?;
                lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
            }
        }
        frame.fit([lo, hi]);
        if frame.rows.is_none() {
            let mut lines = Vec::new();
            let mut curves = Vec::new();
            lines
                .try_reserve(pieces.len())
                .and_then(|()| curves.try_reserve(pieces.len()))
                .map_err(|_| Error::limit("glyph placement allocation failed"))?;
            for piece in pieces {
                match piece.translated(origin) {
                    Piece::Line(line) => lines.push(line),
                    Piece::Curve(q) => curves.push(q),
                }
            }
            return Shape::pieces(lines, curves, &frame, options);
        }
    }
    let mut polygon = Polygon::default();
    polygon
        .points
        .try_reserve(exact.polygon.points.len())
        .and_then(|()| polygon.starts.try_reserve(exact.polygon.starts.len()))
        .map_err(|_| Error::limit("glyph placement allocation failed"))?;
    for &p in &exact.polygon.points {
        let p = Point::new(p.x + origin.x, p.y + origin.y);
        bounded(p)?;
        polygon.points.push(p);
    }
    polygon.starts.extend_from_slice(&exact.polygon.starts);
    for &p in &exact.points {
        // A floor cell belongs to the placed point, not to the outline origin.
        let p = Point::new(p.x + origin.x, p.y + origin.y);
        bounded(p)?;
        polygon.cell(p, options.limits)?;
    }
    if options.precision == Precision::Approximate {
        return super::approximate::glyph(&polygon, options);
    }
    let winding = if exact.certified {
        Winding::Certified
    } else {
        Winding::Unknown
    };
    Shape::polygon(polygon, FillRule::NonZero, winding, options)
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
