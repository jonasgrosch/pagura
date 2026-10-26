//! Marks prepared for the band rasterizer: fills, strokes and glyph
//! placements in device space, clipped by their clip state's rectangle and
//! convex paths, with the rows their signed area does not determine already
//! resolved.

use super::cells::STRIP;
use super::certify::uncertified;
use super::clip::{self, ClipShape, Paths, columns, fill_polygon};
use super::convex::Convex;
use super::curve::{self, Mono, Quadratic};
use super::edges::{self, Bounds, Line, Polygon, StripList, line_work, rows, strips};
use super::scratch::{Kept, ShapeScratch};
use super::union::{Meters, Piece, Resolver, room};
use super::{Adjustment, Area, DeviceRect, Prep, ShapeOptions};
use crate::geometry::{greater, lesser};
use crate::stroke::Tiling;
use crate::{FillRule, Path, Stroke, Transform, features};
use pagura_core::{Error, Meter, Result};

/// The message of every preparation work budget error.
pub(crate) const RASTER_WORK: &str = "raster work budget exceeded";

/// The message of the scratch byte budget errors of preparation.
pub(crate) const RENDER_BYTES: &str = "render memory budget exceeded";

/// One mark's geometry in device space, restricted to an [`Area`] and binned
/// by band: a fill, a stroke outline (of pieces that meet without
/// overlapping where that is exact), or a glyph placement.
#[derive(Debug)]
pub struct Shape {
    pub(crate) area: Area,
    pub(crate) bounds: DeviceRect,
    work: usize,
    retained: usize,
    pub(crate) rule: FillRule,
    pub(crate) body: Body,
    pub(crate) clip: Option<ClipShape>,
    /// Whether the clip's rows restrict the shape's rows: not where the
    /// clip keeps every pixel the shape can touch.
    pub(crate) masked: bool,
    /// The rows resolved at preparation and their sides.
    pub(crate) resolved: Resolved,
}

/// Rows of a shape whose windings its signed area does not determine, or
/// where a clip path's boundary meets it: the sides of their covered set,
/// which replace the shape's edges there.
#[derive(Debug, Default)]
pub(crate) struct Resolved {
    /// Per resolved row, the end of its sides in `lines`.
    rows: Vec<(u32, u32)>,
    lines: Vec<Line>,
}

impl Resolved {
    /// Whether no row is resolved.
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The resolved rows within `rows` as a mask of bits from `rows.start`.
    pub(crate) fn mask(&self, rows: &std::ops::Range<u32>) -> u32 {
        let first = self.rows.partition_point(|&(row, _)| row < rows.start);
        let mut mask = 0_u32;
        for &(row, _) in self.rows.get(first..).unwrap_or(&[]) {
            if row >= rows.end {
                break;
            }
            mask |= 1_u32.checked_shl(row - rows.start).unwrap_or(0);
        }
        mask
    }

    /// The sides of the resolved rows within `rows`.
    pub(crate) fn lines(&self, rows: &std::ops::Range<u32>) -> &[Line] {
        let first = self.rows.partition_point(|&(row, _)| row < rows.start);
        let last = self.rows.partition_point(|&(row, _)| row < rows.end);
        let start = first
            .checked_sub(1)
            .and_then(|i| self.rows.get(i))
            .map_or(0, |&(_, end)| end as usize);
        let end = last
            .checked_sub(1)
            .and_then(|i| self.rows.get(i))
            .map_or(0, |&(_, end)| end as usize);
        self.lines.get(start..end.max(start)).unwrap_or(&[])
    }

    fn push(&mut self, y: u32, sides: &[Line], bytes: &mut Meter) -> Result<()> {
        // An empty resolved row still replaces the original edges.
        room(&mut self.lines, sides.len(), bytes)?;
        self.lines.extend_from_slice(sides);
        self.mark(y, bytes)
    }

    fn mark(&mut self, y: u32, bytes: &mut Meter) -> Result<()> {
        let end = u32::try_from(self.lines.len())
            .map_err(|_| Error::limit("resolved rows exceed u32"))?;
        room(&mut self.rows, 1, bytes)?;
        self.rows.push((y, end));
        Ok(())
    }

    fn len(&self) -> usize {
        self.lines.len()
    }

    /// The sides of every resolved row.
    pub(crate) fn sides(&self) -> &[Line] {
        &self.lines
    }

    fn retained_bytes(&self) -> usize {
        (self.rows.capacity() * size_of::<(u32, u32)>())
            .saturating_add(self.lines.capacity() * size_of::<Line>())
    }
}

/// How a shape's rows are produced.
#[derive(Debug)]
pub(crate) enum Body {
    /// Edges accumulated into the cells.
    Edges(EdgeBody),
    /// Shared glyph geometry, translated as its edges accumulate.
    Glyph(GlyphBody),
}

/// One translated placement of shared glyph geometry.
#[derive(Debug)]
pub(crate) struct GlyphBody {
    pub(crate) edges: std::sync::Arc<super::GlyphEdges>,
    pub(crate) origin: crate::Point,
    pub(crate) scan: (usize, usize),
    pub(crate) strips: Option<StripList>,
    pub(crate) geometry: GlyphGeometry,
    /// Boundary count for placement-error certification, before resolved
    /// sides, including any canonical point contour.
    boundaries: usize,
}

/// Which retained source this placement actually accumulates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlyphGeometry {
    Relative,
    Polygon,
}

/// Straight edges and monotone quadratics, listed by strip, and the
/// columns their cells can be touched in.
#[derive(Debug, Default)]
pub(crate) struct EdgeBody {
    pub(crate) lines: Vec<Line>,
    pub(crate) curves: Vec<Mono>,
    /// Items by strip; a shape within one strip has none and visits every
    /// item.
    pub(crate) strips: Option<StripList>,
    pub(crate) scan: (usize, usize),
}

/// What preparation knows about a shape's winding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Winding {
    /// 0 or one sign of magnitude one everywhere, by construction.
    Certified,
    /// To be certified, with chains at least this far apart.
    Check(f64),
    /// Possibly overlapping: cells are analysed.
    Unknown,
}

impl Shape {
    /// A fill of `path` (user space) under `transform` with `rule`, outside
    /// any clip; `None` when it covers no pixel of the area.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when flattening or rasterization would exceed
    /// `options.limits` and `Syntax` for arithmetic overflow.
    pub fn fill(
        path: &Path,
        transform: Transform,
        rule: FillRule,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        Self::fill_with_clip(path, transform, rule, None, options)
    }

    /// A fill painted in the clip state `clip`: its edges are clipped to the
    /// clip's rectangle and the pixels where its boundary meets the clip's
    /// are integrated with the clip's geometry. Rasterize it with that clip.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn fill_with_clip(
        path: &Path,
        transform: Transform,
        rule: FillRule,
        clip: Option<&ClipShape>,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        Self::fill_in(path, transform, rule, &Prep::new(options, clip))
    }

    /// [`Self::fill_with_clip`] with the buffers of `scratch`, which the
    /// render keeps between its shapes.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn fill_with_scratch(
        path: &Path,
        transform: Transform,
        rule: FillRule,
        (clip, scratch): (Option<&ClipShape>, &ShapeScratch),
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        let options = Prep::new(options, clip).lending(scratch);
        Self::fill_in(path, transform, rule, &options)
    }

    pub(crate) fn fill_in(
        path: &Path,
        transform: Transform,
        rule: FillRule,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        let mut bytes = Meter::new(options.limits.render_bytes, RENDER_BYTES);
        let (polygon, rule) = fill_polygon(path, transform, rule, options.limits, &mut bytes)?;
        Self::polygon(
            polygon,
            rule,
            Winding::Check(0.0),
            options,
            &mut bytes,
            None,
        )
    }

    /// A stroke of `path` (user space) with `stroke` whose line width and
    /// dashes are measured in pen space; `pen` maps user space to pen space
    /// (the inverse of the CTM the stroke was painted under, relative to
    /// user space), and `transform` user space to device space. Outside any
    /// clip; `None` when it covers no pixel of the area.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`], plus `Unsupported` for a singular pen.
    pub fn stroke(
        path: &Path,
        stroke: &Stroke,
        pen: Transform,
        transform: Transform,
        adjustment: Adjustment,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        Self::stroke_in(
            path,
            stroke,
            (pen, transform),
            adjustment,
            &Prep::new(options, None),
        )
    }

    /// A stroke painted in the clip state `clip`, as [`Self::fill_with_clip`].
    ///
    /// # Errors
    ///
    /// As [`Self::stroke`].
    pub fn stroke_with_clip(
        path: &Path,
        stroke: &Stroke,
        (pen, transform): (Transform, Transform),
        adjustment: Adjustment,
        clip: Option<&ClipShape>,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        Self::stroke_in(
            path,
            stroke,
            (pen, transform),
            adjustment,
            &Prep::new(options, clip),
        )
    }

    pub(crate) fn stroke_in(
        path: &Path,
        stroke: &Stroke,
        (pen, transform): (Transform, Transform),
        adjustment: Adjustment,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        let (polygon, tiling, mut bytes) =
            Self::stroke_pieces(path, stroke, (pen, transform), adjustment, options)?;
        Self::stroke_finish(polygon, tiling, options, &mut bytes)
    }

    /// The device polygon of a stroke's pieces, how far they tile, and the
    /// byte meter their preparation goes on with.
    pub(crate) fn stroke_pieces(
        path: &Path,
        stroke: &Stroke,
        (pen, transform): (Transform, Transform),
        adjustment: Adjustment,
        options: &Prep<'_>,
    ) -> Result<(Polygon, Tiling, Meter)> {
        let limits = options.limits;
        if path.segments().len() > limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        let to_user = pen.inverse().ok_or(Error::unsupported(
            features::SINGULAR_PEN,
            "singular stroke pen is not implemented",
        ))?;
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let path = path.transformed_with_bytes(pen, &mut bytes)?;
        let device = to_user.then(transform);
        let (polygon, tiling) = match adjustment {
            Adjustment::Adjusted => crate::stroke::adjusted_polygon_pieces(
                &path,
                device,
                stroke,
                limits,
                Tiling::Unproven,
                &mut bytes,
            )?,
            Adjustment::None => crate::stroke::polygon_pieces(
                &path,
                device,
                stroke,
                limits,
                (Tiling::Unproven, crate::flatten::TOLERANCE),
                &mut bytes,
            )?,
        };
        let released = path.retained_bytes();
        drop(path);
        bytes.refund(released);
        Ok((polygon, tiling, bytes))
    }

    /// The shape of a stroke's pieces, with the rows their windings leave in
    /// doubt resolved.
    pub(crate) fn stroke_finish(
        polygon: Polygon,
        tiling: Tiling,
        options: &Prep<'_>,
        bytes: &mut Meter,
    ) -> Result<Option<Self>> {
        let winding = match tiling {
            Tiling::Proven => Winding::Certified,
            // Only the slabs where the outline may overlap itself need the
            // union resolver.
            Tiling::Unproven => Winding::Check(0.0),
            Tiling::Components => Winding::Unknown,
        };
        Self::polygon(polygon, FillRule::NonZero, winding, options, bytes, None)
    }

    /// One glyph placement outside any clip: `edges` translated to the
    /// device-space `origin`; `None` when it covers no pixel of the area.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn glyph(
        edges: &std::sync::Arc<super::GlyphEdges>,
        origin: crate::Point,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        Self::glyph_with_clip(edges, origin, None, options)
    }

    /// A glyph placement in the clip state `clip`, as
    /// [`Self::fill_with_clip`].
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn glyph_with_clip(
        edges: &std::sync::Arc<super::GlyphEdges>,
        origin: crate::Point,
        clip: Option<&ClipShape>,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        super::glyph::place(edges, origin, &Prep::new(options, clip))
    }

    /// [`Self::glyph_with_clip`] with the buffers of `scratch`, as
    /// [`Self::fill_with_scratch`].
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn glyph_with_scratch(
        edges: &std::sync::Arc<super::GlyphEdges>,
        origin: crate::Point,
        (clip, scratch): (Option<&ClipShape>, &ShapeScratch),
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        super::glyph::place(edges, origin, &Prep::new(options, clip).lending(scratch))
    }

    #[cfg(test)]
    pub(crate) fn glyph_in(
        edges: &std::sync::Arc<super::GlyphEdges>,
        origin: crate::Point,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        super::glyph::place(edges, origin, options)
    }

    /// The pixels the shape can cover within the area.
    pub fn bounds(&self) -> DeviceRect {
        self.bounds
    }

    /// The bands the shape meets.
    pub fn bands(&self, area: &Area) -> std::ops::Range<u32> {
        Area {
            region: self.bounds,
            band_height: area.band_height,
        }
        .bands()
    }

    /// Work units its rasterization costs at most over all its bands, for
    /// the render-work reservation.
    pub fn work(&self) -> usize {
        self.work
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// Boundary pieces that translation and row resolution can perturb.
    pub(crate) fn boundaries(&self) -> usize {
        let own = match &self.body {
            Body::Edges(edges) => edges.lines.len() + edges.curves.len(),
            Body::Glyph(glyph) => glyph.boundaries,
        };
        own.saturating_add(self.resolved.len())
    }

    /// The most coverage one band of this shape can hold: its rows within a
    /// band, and every pixel of its width in them a partial cell and a span
    /// of its own.
    pub fn capacity(&self) -> super::Capacity {
        let rows = self.area.band_height.min(self.bounds.height()) as usize;
        let cells = rows.saturating_mul(self.bounds.width() as usize);
        super::Capacity {
            rows,
            spans: cells,
            cells,
        }
    }

    /// A shape of closed `polygon` with `rule`.
    pub(crate) fn polygon(
        polygon: Polygon,
        rule: FillRule,
        winding: Winding,
        options: &Prep<'_>,
        bytes: &mut Meter,
        shared: Option<(&std::sync::Arc<super::GlyphEdges>, crate::Point)>,
    ) -> Result<Option<Self>> {
        let mut work = Meter::new(options.limits.raster_work, RASTER_WORK);
        Self::polygon_metered(polygon, rule, winding, options, bytes, &mut work, shared)
    }

    /// A polygon replacement whose preparation shares the certificate's
    /// cumulative inspection/refinement work meter.
    pub(crate) fn polygon_metered(
        polygon: Polygon,
        rule: FillRule,
        winding: Winding,
        options: &Prep<'_>,
        bytes: &mut Meter,
        work: &mut Meter,
        shared: Option<(&std::sync::Arc<super::GlyphEdges>, crate::Point)>,
    ) -> Result<Option<Self>> {
        let Some(mut frame) = Frame::new(options) else {
            return Ok(None);
        };
        let Some(hull) = polygon.bounds() else {
            return Ok(None);
        };
        frame.fit(hull);
        if let Some(paths) = frame.paths {
            // A mark strictly inside every convex clipping path meets none
            // of their boundaries.
            work.charge(
                paths
                    .convex()
                    .map_or(0, |convex| convex.len().saturating_mul(polygon.vertices())),
            )?;
            if paths.contain(&polygon.points) {
                frame.paths = None;
            }
        }
        // Only the original, uncut stored polygon can supply these edges.
        let shared = shared.filter(|_| frame.rect.is_none() && frame.paths.is_none());
        // The heights the winding certificate leaves in doubt: none when
        // certified throughout, all when unknown.
        // A lone triangle bounds winding 0 or one sign of magnitude one,
        // whatever its vertices.
        let triangle =
            polygon.starts.len() == 1 && polygon.vertices() == 3 && !polygon.points_cells;
        let polygon_bytes = polygon.heap_bytes();
        let doubtful = match winding {
            Winding::Certified => Some(Vec::new()),
            Winding::Check(_) if triangle => Some(Vec::new()),
            Winding::Check(margin) => uncertified(&polygon, margin, bytes)?,
            Winding::Unknown => None,
        };
        let all_bytes = polygon.vertices().saturating_mul(size_of::<Line>());
        let all = edges::lines(&polygon, bytes)?;
        drop(polygon);
        bytes.refund(polygon_bytes);
        let lines = match frame.rect {
            Some(r) => {
                let mut clipped = Vec::new();
                for &line in &all {
                    room(&mut clipped, 3, bytes)?;
                    edges::clip_line(line, r, &mut clipped)?;
                }
                clipped.retain(|l| !l.is_horizontal());
                drop(all);
                bytes.refund(all_bytes);
                clipped
            }
            None => all,
        };
        // A mark crossing convex clipping paths is clipped to them as to
        // the rectangle, and then meets no clip row.
        let lines = match frame.paths.and_then(Paths::convex) {
            Some(convex) => {
                let hull = Bounds {
                    left: hull[0].x,
                    top: hull[0].y,
                    right: hull[1].x,
                    bottom: hull[1].y,
                };
                let clipped = clip_convex(lines, (convex, hull), work, bytes)?;
                frame.paths = None;
                clipped
            }
            None => lines,
        };
        let Some(bounds) = frame.bounds(lines.iter().flat_map(|l| [l.a, l.b])) else {
            return Ok(None);
        };
        let width = super::row::width(options.area.region.x1)?;
        let scan = clip::scan_columns(&lines, width);
        // Rows that meet a doubtful slab, or where a clip path's boundary
        // crosses the shape's columns.
        let doubtful_row = |y: u32| {
            doubtful.as_ref().is_none_or(|slabs| {
                let (top, bottom) = (f64::from(y), f64::from(y) + 1.0);
                slabs.iter().any(|&[lo, hi]| lo < bottom && hi > top)
            })
        };
        let joint = |y: u32| {
            frame
                .paths
                .is_some_and(|p| p.rows.crosses(y, (bounds.x0, bounds.x1)))
        };
        let mut chosen = Vec::new();
        for y in bounds.y0..bounds.y1 {
            let joint = joint(y);
            if joint || doubtful_row(y) {
                room(&mut chosen, 1, bytes)?;
                chosen.push((y, joint));
            }
        }
        let resolved = if chosen.is_empty() {
            Resolved::default()
        } else {
            resolve(&lines, rule, &frame, (bounds, &chosen), work, bytes)?
        };
        Self::edges(
            (lines, Vec::new()),
            rule,
            (bounds, scan, resolved),
            &frame,
            options,
            bytes,
            shared,
        )
        .map(Some)
    }

    /// A shape of certified lines and monotone quadratics in `frame`, which
    /// is fitted to them and has no clip rows left.
    pub(crate) fn pieces(
        lines: Vec<Line>,
        curves: Vec<Quadratic>,
        frame: &Frame<'_>,
        options: &Prep<'_>,
        bytes: &mut Meter,
    ) -> Result<Option<Self>> {
        debug_assert!(frame.paths.is_none(), "pieces meet no clip rows");
        let (mut lines, curves) = match frame.rect {
            Some(r) => {
                let mut clipped = Vec::new();
                let mut kept = Vec::new();
                let source_bytes = lines
                    .capacity()
                    .saturating_mul(size_of::<Line>())
                    .saturating_add(curves.capacity().saturating_mul(size_of::<Quadratic>()));
                for &line in &lines {
                    room(&mut clipped, 3, bytes)?;
                    edges::clip_line(line, r, &mut clipped)?;
                }
                for &q in &curves {
                    curve::clip(q, r, &mut clipped, &mut kept, bytes)?;
                }
                drop(lines);
                drop(curves);
                bytes.refund(source_bytes);
                (clipped, kept)
            }
            None => (lines, curves),
        };
        let mut monos = Vec::new();
        bytes.reserve(&mut monos, curves.len())?;
        monos.extend(curves.iter().map(|&q| Mono::new(q)));
        let points = lines
            .iter()
            .flat_map(|l| [l.a, l.b])
            .chain(curves.iter().flat_map(|q| [q.points[0], q.points[2]]));
        let Some(bounds) = frame.bounds(points) else {
            return Ok(None);
        };
        let width = super::row::width(options.area.region.x1)?;
        let scan = scan_columns(
            lines
                .iter()
                .flat_map(|l| [l.a.x, l.b.x])
                .chain(curves.iter().flat_map(|q| [q.points[0].x, q.points[2].x])),
            width,
        );
        lines.retain(|l| !l.is_horizontal());
        Self::edges(
            (lines, monos),
            FillRule::NonZero,
            (bounds, scan, Resolved::default()),
            frame,
            options,
            bytes,
            None,
        )
        .map(Some)
    }

    /// A placement of a certified glyph within `frame`, which keeps every
    /// pixel the glyph can touch: its relative edges and the origin, with
    /// no owned geometry and no allocation.
    pub(crate) fn translated(
        edges: &std::sync::Arc<super::GlyphEdges>,
        relative: &super::glyph::Relative,
        origin: crate::Point,
        [lo, hi]: [crate::Point; 2],
        frame: &Frame<'_>,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        let Some(bounds) = frame.bounds([lo, hi].into_iter()) else {
            return Ok(None);
        };
        let width = super::row::width(options.area.region.x1)?;
        let scan = columns(lo.x, hi.x, width);
        let columns = scan.1.saturating_sub(scan.0);
        let words = columns / 64 + 2;
        // Each window of a band visits every item once, at most as often as
        // the placement has rows.
        let points = edges.points();
        let items = relative
            .lines
            .len()
            .saturating_add(relative.curves.len())
            .saturating_add(points.len().saturating_mul(2));
        let rows = bounds.height() as usize;
        // Each point side visits one row. Use the same primitive/emission
        // bound as the relative body's edges; item visits include both
        // sides in every window, even when the point is outside it.
        let point_work = points
            .iter()
            .flat_map(|&point| super::glyph::point_sides(point + origin))
            .filter_map(|line| {
                line_rows(&line, bounds, options.area.region.x1)
                    .map(|rows| line_work(line, rows, width).saturating_mul(4))
            })
            .fold(0_usize, usize::saturating_add);
        let work = relative
            .work
            .saturating_add(point_work)
            .saturating_add(rows.saturating_mul(words + columns + 4))
            .saturating_add(rows.saturating_mul(items));
        Self::shared_glyph(
            edges,
            origin,
            (bounds, scan),
            (GlyphGeometry::Relative, items),
            work,
            frame,
            options,
        )
    }

    /// A short certified polygon with source-fixed bounds and work. Its
    /// raw sides use the same accumulation as any shared polygon body.
    pub(crate) fn polygon_glyph(
        edges: &std::sync::Arc<super::GlyphEdges>,
        summary: super::glyph::Summary,
        origin: crate::Point,
        [lo, hi]: [crate::Point; 2],
        frame: &Frame<'_>,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        let Some(bounds) = frame.bounds([lo, hi].into_iter()) else {
            return Ok(None);
        };
        let super::glyph::Kind::Polygon { polygon, .. } = &edges.kind else {
            unreachable!("canonical polygon source");
        };
        let width = super::row::width(options.area.region.x1)?;
        let scan = columns(lo.x, hi.x, width);
        let work = summary
            .work(
                polygon.vertices(),
                (bounds.height() as usize, scan.1.saturating_sub(scan.0)),
            )
            .ok_or(Error::limit(RASTER_WORK))?;
        Self::shared_glyph(
            edges,
            origin,
            (bounds, scan),
            (GlyphGeometry::Polygon, summary.boundaries()),
            work,
            frame,
            options,
        )
    }

    /// The common owner and limit check for allocation-free placements.
    fn shared_glyph(
        edges: &std::sync::Arc<super::GlyphEdges>,
        origin: crate::Point,
        (bounds, scan): (DeviceRect, (usize, usize)),
        (geometry, boundaries): (GlyphGeometry, usize),
        work: usize,
        frame: &Frame<'_>,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        if work > options.limits.raster_work {
            return Err(Error::limit(RASTER_WORK));
        }
        Ok(Some(Self {
            area: options.area,
            bounds,
            work,
            retained: size_of::<Self>(),
            rule: FillRule::NonZero,
            body: Body::Glyph(GlyphBody {
                edges: std::sync::Arc::clone(edges),
                origin,
                scan,
                strips: None,
                geometry,
                boundaries,
            }),
            clip: frame.clip.cloned(),
            masked: false,
            resolved: Resolved::default(),
        }))
    }

    pub(crate) fn edges(
        (lines, curves): (Vec<Line>, Vec<Mono>),
        rule: FillRule,
        (bounds, scan, resolved): (DeviceRect, (usize, usize), Resolved),
        frame: &Frame<'_>,
        options: &Prep<'_>,
        bytes: &mut Meter,
        shared: Option<(&std::sync::Arc<super::GlyphEdges>, crate::Point)>,
    ) -> Result<Self> {
        bytes.charge(size_of::<Self>())?;
        let area = options.area;
        let region = area.region;
        let line_extent = |l: &Line| line_rows(l, bounds, region.x1);
        let count = lines.len() + curves.len();
        let columns = scan.1.saturating_sub(scan.0);
        let shared = shared.and_then(|(edges, origin)| {
            let super::glyph::Kind::Polygon {
                polygon, points, ..
            } = &edges.kind
            else {
                return None;
            };
            let view = super::glyph::PolygonLines::new(polygon, points, origin);
            Some((edges, origin, view, view.keys()?))
        });
        // A shape within one strip visits every item in each window of a
        // band, at most once per row; a taller one lists its items per
        // strip, each visited at most once per row of the strip.
        let height = bounds.height() as usize;
        let (list, mut work, cells) = if strips((bounds.y0, bounds.y1 - 1)) == 1 {
            let cells = cell_work(&lines, &curves, &resolved, bounds, region.x1);
            (None, count.saturating_mul(height), cells)
        } else if let Some((_, _, view, keys)) = shared {
            let list = StripList::new(
                (bounds.y0, bounds.y1),
                keys,
                view.indexed().filter_map(|(key, line)| {
                    (!line.is_horizontal())
                        .then(|| line_extent(&line))
                        .flatten()
                        .map(|rows| (key, rows))
                }),
                bytes,
            )?;
            let visits = list_visits(&list);
            let cells = cell_work(&lines, &curves, &resolved, bounds, region.x1);
            (Some(list), visits, cells)
        } else {
            let (list, cells) = listed((&lines, &curves), &resolved, bounds, region.x1, bytes)?;
            let visits = list_visits(&list);
            (Some(list), visits, cells)
        };
        let rows = bounds.height() as usize;
        let words = columns / 64 + 2;
        let overhead = rows
            .saturating_mul(words + columns + 4)
            .saturating_add(resolved.len())
            .saturating_add(frame.segments(bounds));
        work = work
            .saturating_add(cells.saturating_mul(4))
            .saturating_add(overhead);
        if work > options.limits.raster_work {
            return Err(Error::limit(RASTER_WORK));
        }
        let shared = shared.filter(|(_, _, view, _)| {
            list.is_some()
                || view
                    .raw_count()
                    .and_then(|raw| shared_work(raw, rows, columns, cells, overhead))
                    .is_some_and(|bound| bound <= work)
        });
        let (body, payload) = Self::retain_edges(
            (lines, curves),
            list,
            scan,
            shared.map(|(edges, origin, _, _)| (edges, origin)),
            bytes,
        );
        let retained = size_of::<Self>()
            .saturating_add(payload)
            .saturating_add(resolved.retained_bytes());
        Ok(Self {
            area,
            bounds,
            work,
            retained,
            rule,
            body,
            clip: frame.clip.cloned(),
            masked: frame.paths.is_some(),
            resolved,
        })
    }

    /// The final owner of prepared sides: retain the index beside shared
    /// glyph geometry, and release only the copied edge vectors.
    fn retain_edges(
        (lines, curves): (Vec<Line>, Vec<Mono>),
        list: Option<StripList>,
        scan: (usize, usize),
        shared: Option<(&std::sync::Arc<super::GlyphEdges>, crate::Point)>,
        bytes: &mut Meter,
    ) -> (Body, usize) {
        let count = lines.len() + curves.len();
        let payload = lines.capacity() * size_of::<Line>() + curves.capacity() * size_of::<Mono>();
        let list_bytes = list.as_ref().map_or(0, StripList::retained_bytes);
        if let Some((edges, origin)) = shared {
            drop((lines, curves));
            bytes.refund(payload);
            (
                Body::Glyph(GlyphBody {
                    edges: std::sync::Arc::clone(edges),
                    origin,
                    scan,
                    strips: list,
                    geometry: GlyphGeometry::Polygon,
                    boundaries: count,
                }),
                list_bytes,
            )
        } else {
            (
                Body::Edges(EdgeBody {
                    lines,
                    curves,
                    strips: list,
                    scan,
                }),
                payload + list_bytes,
            )
        }
    }

    /// Adds `units` to the work bound.
    pub(crate) fn add_work(&mut self, units: usize) {
        self.work = self.work.saturating_add(units);
    }
}

fn line_rows(line: &Line, bounds: DeviceRect, right: u32) -> Option<(u32, u32)> {
    if line.a.x.min(line.b.x) >= f64::from(right) {
        None
    } else {
        rows_of(line, bounds)
    }
}

fn curve_rows(curve: &Mono, bounds: DeviceRect, right: u32) -> Option<(u32, u32)> {
    if curve.x0.min(curve.x1) >= f64::from(right) {
        None
    } else {
        rows(curve.y0, curve.y1, bounds)
    }
}

/// Each window of a band visits a listed item at most once per row of its
/// strip.
fn list_visits(list: &StripList) -> usize {
    list.len().saturating_mul(STRIP as usize)
}

/// The strip list of owned items and their cell bound, each item's rows
/// found once for both of the list's passes and the bound.
fn listed(
    (lines, curves): (&[Line], &[Mono]),
    resolved: &Resolved,
    bounds: DeviceRect,
    right: u32,
    bytes: &mut Meter,
) -> Result<(StripList, usize)> {
    let count = lines.len() + curves.len();
    let mut extents = Vec::new();
    bytes.reserve(&mut extents, count)?;
    extents.extend(lines.iter().map(|l| line_rows(l, bounds, right)));
    extents.extend(curves.iter().map(|m| curve_rows(m, bounds, right)));
    let list = StripList::new(
        (bounds.y0, bounds.y1),
        count,
        extents
            .iter()
            .enumerate()
            .filter_map(|(i, rows)| rows.map(|rows| (i, rows))),
        bytes,
    );
    let mut cells = resolved_cells(resolved, bounds, right);
    let items = lines.iter().copied().chain(curves.iter().map(chord));
    for (line, rows) in items.zip(&extents) {
        if let Some(r) = *rows {
            cells = cells.saturating_add(line_work(line, r, right as usize));
        }
    }
    let released = extents.capacity() * size_of::<Option<(u32, u32)>>();
    drop(extents);
    bytes.refund(released);
    Ok((list?, cells))
}

/// The same cell bound for owned and shared geometry, including every
/// absolute side that replaces a resolved row.
fn cell_work(
    lines: &[Line],
    curves: &[Mono],
    resolved: &Resolved,
    bounds: DeviceRect,
    right: u32,
) -> usize {
    let mut cells = 0_usize;
    for line in lines {
        if let Some(r) = line_rows(line, bounds, right) {
            cells = cells.saturating_add(line_work(*line, r, right as usize));
        }
    }
    for curve in curves {
        if let Some(r) = curve_rows(curve, bounds, right) {
            cells = cells.saturating_add(line_work(chord(curve), r, right as usize));
        }
    }
    cells.saturating_add(resolved_cells(resolved, bounds, right))
}

/// The cell bound of the sides that replace resolved rows.
fn resolved_cells(resolved: &Resolved, bounds: DeviceRect, right: u32) -> usize {
    let mut cells = 0_usize;
    for line in &resolved.lines {
        let r = rows_of(line, bounds).unwrap_or((bounds.y0, bounds.y0));
        cells = cells.saturating_add(line_work(*line, r, right as usize));
    }
    cells
}

/// A monotone quadratic's chord, which bounds the cells it touches.
fn chord(curve: &Mono) -> Line {
    Line {
        a: crate::Point::new(curve.x0, curve.y0),
        b: crate::Point::new(curve.x1, curve.y1),
    }
}

/// Borrowed polygons visit raw vertices even when a side is horizontal.
/// Line primitives cost at most `cells`; emission touches at most that
/// many cells and at most the scanned rectangle's area. Keep the original
/// work reservation only when it covers this complete traversal too.
fn shared_work(
    raw: usize,
    rows: usize,
    columns: usize,
    cells: usize,
    overhead: usize,
) -> Option<usize> {
    let touched = cells.min(rows.checked_mul(columns)?);
    raw.checked_mul(rows)?
        .checked_add(cells)?
        .checked_add(touched.checked_mul(3)?)?
        .checked_add(overhead)
}

/// The columns cells of edges with these x can be touched in.
fn scan_columns(xs: impl Iterator<Item = f64>, width: usize) -> (usize, usize) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for x in xs {
        lo = lesser(lo, x);
        hi = greater(hi, x);
    }
    columns(lo, hi, width)
}

/// The region and clip a shape is prepared in, and the buffers it borrows.
pub(crate) struct Frame<'a> {
    pub(crate) clip: Option<&'a ClipShape>,
    pub(crate) rect: Option<Bounds>,
    pub(crate) paths: Option<&'a Paths>,
    /// The pixels the clip keeps within the region.
    pub(crate) limit: DeviceRect,
    pub(crate) scratch: Option<&'a ShapeScratch>,
}

impl<'a> Frame<'a> {
    /// `None` when the clip keeps nothing of the region.
    pub(crate) fn new(options: &Prep<'a>) -> Option<Self> {
        let region = options.area.region;
        let clip = options.clip;
        let limit = clip.map_or(region, |c| c.bounds().intersect(&region));
        if limit.is_empty() {
            return None;
        }
        Some(Self {
            clip,
            rect: clip.and_then(ClipShape::rect),
            paths: clip.and_then(ClipShape::paths),
            limit,
            scratch: options.scratch,
        })
    }

    /// Drops what the clip cannot change for geometry within `hull`: a
    /// rectangle containing it needs no edge clipping, and rows inside
    /// across every pixel it can touch, or convex paths containing it, need
    /// no overlay or analysis. A mark within the page crop thus meets no
    /// clip at all.
    pub(crate) fn fit(&mut self, [lo, hi]: [crate::Point; 2]) {
        let hull = Bounds {
            left: lo.x,
            top: lo.y,
            right: hi.x,
            bottom: hi.y,
        };
        if self.rect.is_some_and(|r| r.contains(hull)) {
            self.rect = None;
        }
        let corners = [
            lo,
            crate::Point::new(hi.x, lo.y),
            hi,
            crate::Point::new(lo.x, hi.y),
        ];
        if self.paths.is_some_and(|p| {
            p.rows.inside(edges::pixels([lo, hi], self.limit, false)) || p.contain(&corners)
        }) {
            self.paths = None;
        }
    }

    /// The pixels geometry with these points can cover within the clip.
    pub(crate) fn bounds(&self, points: impl Iterator<Item = crate::Point>) -> Option<DeviceRect> {
        let mut lo = crate::Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = crate::Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in points {
            lo.x = lesser(lo.x, p.x);
            lo.y = lesser(lo.y, p.y);
            hi.x = greater(hi.x, p.x);
            hi.y = greater(hi.y, p.y);
        }
        if lo.x > hi.x || lo.y > hi.y {
            return None;
        }
        let bounds = edges::pixels([lo, hi], self.limit, false);
        (!bounds.is_empty()).then_some(bounds)
    }

    /// The clip segments of the rows of `bounds`.
    pub(crate) fn segments(&self, bounds: DeviceRect) -> usize {
        self.paths.map_or(0, |p| {
            (bounds.y0..bounds.y1)
                .map(|y| p.rows.row(y).len())
                .fold(0, usize::saturating_add)
        })
    }
}

/// The pieces of `lines` within every path of `convex`, each path clipping
/// the pieces the previous one left, and within the mark's `hull`.
fn clip_convex(
    mut lines: Vec<Line>,
    (convex, hull): (&[Convex], Bounds),
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<Vec<Line>> {
    for chains in convex {
        work.charge(lines.len())?;
        let mut clipped = Vec::new();
        room(&mut clipped, lines.len(), bytes)?;
        // Contour order: each line usually starts where the last one ended.
        let mut last = None;
        for &line in &lines {
            let start = match last {
                Some((end, inside)) if end == line.a => inside,
                _ => chains.contains(line.a),
            };
            let end = chains.contains(line.b);
            last = Some((line.b, end));
            // Both ends strictly inside: by convexity, so is the line.
            if start && end {
                room(&mut clipped, 1, bytes)?;
                clipped.push(line);
                continue;
            }
            let bound = chains.bound(line);
            work.charge(bound)?;
            room(&mut clipped, bound, bytes)?;
            chains.clip(line, &mut clipped);
        }
        let released = lines.capacity() * size_of::<Line>();
        drop(lines);
        bytes.refund(released);
        lines = clipped;
    }
    // Parts moved along a nearly horizontal chain side can reach far beside
    // the mark; its hull box clips them back, as the rectangle does.
    work.charge(lines.len())?;
    let beside = |l: &Line| l.a.x.min(l.b.x) < hull.left || l.a.x.max(l.b.x) > hull.right;
    if lines.iter().any(beside) {
        let mut kept = Vec::new();
        room(&mut kept, lines.len(), bytes)?;
        for &line in &lines {
            if beside(&line) {
                room(&mut kept, 3, bytes)?;
                edges::clip_line(line, hull, &mut kept)?;
            } else {
                room(&mut kept, 1, bytes)?;
                kept.push(line);
            }
        }
        kept.retain(|l| !l.is_horizontal());
        let released = lines.capacity() * size_of::<Line>();
        drop(lines);
        bytes.refund(released);
        lines = kept;
    }
    Ok(lines)
}

/// The rows of `bounds` a line has area in, as `(first, last)`.
fn rows_of(l: &Line, bounds: DeviceRect) -> Option<(u32, u32)> {
    rows(l.a.y.min(l.b.y), l.a.y.max(l.b.y), bounds)
}

/// Resolves the rows `chosen` of `bounds`: the sides of the covered set of
/// `lines` under `rule`, within the clip's paths in the rows marked joint.
///
/// Its work is charged to a meter of `raster_work`, and its scratch to one
/// of `render_bytes`: the resolver and row buffers the frame lends, whose
/// capacity is paid while they are kept, or its own, which it frees on
/// return.
fn resolve(
    lines: &[Line],
    rule: FillRule,
    frame: &Frame<'_>,
    rows: (DeviceRect, &[(u32, bool)]),
    work: &mut Meter,
    bytes: &mut Meter,
) -> Result<Resolved> {
    let mut kept = frame.scratch.map_or_else(Kept::default, ShapeScratch::take);
    let resolved = resolve_rows(lines, rule, frame, rows, &mut kept, Meters { work, bytes });
    if let Some(scratch) = frame.scratch {
        scratch.keep(kept);
    }
    resolved
}

/// [`resolve`] with the resolver and row buffers of `kept`.
fn resolve_rows(
    lines: &[Line],
    rule: FillRule,
    frame: &Frame<'_>,
    (bounds, chosen): (DeviceRect, &[(u32, bool)]),
    Kept {
        resolver,
        pieces,
        sides,
    }: &mut Kept,
    Meters { work, bytes }: Meters<'_>,
) -> Result<Resolved> {
    // The lines of each strip, then each row's pieces from its strip.
    let strip_rows =
        |l: &Line| rows_of(l, bounds).map(|(a, b)| (b / STRIP - a / STRIP) as usize + 1);
    let listed = lines
        .iter()
        .filter_map(strip_rows)
        .fold(0, usize::saturating_add);
    work.charge(listed.saturating_add(lines.len()))?;
    let list = StripList::new(
        (bounds.y0, bounds.y1),
        lines.len(),
        (0..lines.len()).filter_map(|i| rows_of(&lines[i], bounds).map(|rows| (i, rows))),
        bytes,
    )?;
    let mut rules = Vec::new();
    room(&mut rules, 1, bytes)?;
    rules.push(rule);
    if let Some(paths) = frame.paths {
        let count = paths.rules().count();
        room(&mut rules, count, bytes)?;
        rules.extend(paths.rules());
    }
    let mut resolved = Resolved::default();
    for &(y, joint) in chosen {
        let top = f64::from(y);
        pieces.clear();
        let listed = list.strip(y / STRIP);
        room(pieces, listed.len(), bytes)?;
        for &i in listed {
            if let Some(&line) = lines.get(i as usize) {
                pieces.extend(Piece::new(line, top, 0));
            }
        }
        let groups = match (joint, frame.paths) {
            (true, Some(paths)) => {
                paths.pieces(y, 1, (pieces, bytes), work)?;
                rules.len()
            }
            _ => 1,
        };
        resolve_pieces(
            pieces,
            &rules[..groups],
            y,
            (resolver, sides, &mut resolved),
            Meters { work, bytes },
        )?;
    }
    Ok(resolved)
}

/// Shared winding resolution and retained-row assembly for polygon and
/// joint clip preparation.
fn resolve_pieces(
    pieces: &[Piece],
    rules: &[FillRule],
    y: u32,
    (resolver, sides, resolved): (&mut Resolver, &mut Vec<Line>, &mut Resolved),
    Meters { work, bytes }: Meters<'_>,
) -> Result<()> {
    work.charge(pieces.len())?;
    sides.clear();
    resolver.row(pieces, rules, f64::from(y), (sides, Meters { work, bytes }))?;
    resolved.push(y, sides, bytes)
}

#[cfg(test)]
mod tests;
