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
use super::curve::{self, Piece};
use super::edges::{Line, Polygon};
use super::shape::{Shape, Winding};
use super::{GlyphOrigins, Precision, ShapeOptions};
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
        let count = flatten_into(outline, transform, limits, &mut contours, &mut CurveStack::new())?;
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
        let retained = size_of::<Self>()
            .saturating_add(polygon.points.capacity() * size_of::<Point>())
            .saturating_add(polygon.starts.capacity() * size_of::<usize>())
            .saturating_add(curves.as_ref().map_or(0, |c| c.capacity() * size_of::<Piece>()));
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
    options: &ShapeOptions<'_>,
) -> Result<Option<Shape>> {
    bounded(origin)?;
    let exact = match &edges.kind {
        Kind::Binned(prepared) => {
            let _ = origins;
            return binned::place(edges, prepared, origin, options);
        }
        Kind::Exact(exact) => exact,
    };
    let rectangular = options.clip.is_none_or(|clip| clip.rows().is_none());
    if options.precision == Precision::Exact
        && rectangular
        && let Some(pieces) = &exact.curves
    {
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
            for p in piece.translated(origin).ends() {
                bounded(p)?;
            }
        }
        return Shape::pieces(lines, curves, options);
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
        return super::approximate::glyph(polygon, options);
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

