//! Marks prepared for the band rasterizer: fills, strokes and glyph
//! placements in device space, clipped by their clip state's rectangle,
//! with the cells their signed area does not determine already resolved.

use super::certify::uncertified;
use super::clip::{ClipShape, INSIDE, Rows, columns, fill_polygon};
use super::curve::{self, Quadratic};
use super::edges::{self, Bounds, Chained, Line, Polygon, RowList, line_work, rows};
use super::rect::rectangle;
use super::row::fold;
use super::scan::{RowIndex, Scanner, Stored, Walk, gap_winding, inside};
use super::{Adjustment, Area, DeviceRect, Prep, ShapeOptions};
use crate::stroke::Tiling;
use crate::{FillRule, Path, Stroke, Transform, features};
use pagura_core::{Error, Meter, Result};

/// The message of every preparation work budget error.
pub(crate) const RASTER_WORK: &str = "raster work budget exceeded";

/// One mark's geometry in device space, restricted to an [`Area`] and binned
/// by band: a fill, a stroke outline (as pieces that meet without
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
    /// Whether the clip's rows apply to the shape's rows: not where the
    /// clip keeps every pixel the shape can touch.
    pub(crate) overlay: bool,
    pub(crate) stored: Stored,
}

/// How a shape's rows are produced.
#[derive(Debug)]
pub(crate) enum Body {
    /// An axis-aligned rectangle in closed form, within the region.
    Rect(Bounds),
    /// Edges accumulated into the row.
    Edges(EdgeBody),
    /// A certified glyph's relative edges, translated as they accumulate.
    Glyph(GlyphBody),
}

/// One translated placement of a certified glyph.
#[derive(Debug)]
pub(crate) struct GlyphBody {
    pub(crate) edges: std::sync::Arc<super::GlyphEdges>,
    pub(crate) origin: crate::Point,
    /// The device row of the relative row list's first row.
    pub(crate) row0: i64,
    pub(crate) scan: (usize, usize),
}

/// Straight edges and monotone quadratics listed by band, and the columns
/// their cells can be touched in.
#[derive(Debug, Default)]
pub(crate) struct EdgeBody {
    pub(crate) lines: Vec<Line>,
    pub(crate) curves: Vec<Quadratic>,
    /// Items by row; a small, narrow shape with nothing to overlay has
    /// none and goes edge by edge into the rasterizer's grid.
    pub(crate) index: Option<RowList>,
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

    pub(crate) fn fill_in(
        path: &Path,
        transform: Transform,
        rule: FillRule,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        // A rectangle is filled in closed form.
        if let Some(rect) = rect_fill(path, transform, options.limits) {
            // The rectangle the polygon would be, without building it.
            let Some(mut frame) = Frame::new(options) else {
                return Ok(None);
            };
            frame.fit([
                crate::Point::new(rect.left, rect.top),
                crate::Point::new(rect.right, rect.bottom),
            ]);
            if frame.rows.is_none() {
                return Ok(Self::rect(rect, &frame, options));
            }
        }
        let (polygon, rule) = fill_polygon(path, transform, rule, options.limits)?;
        Self::polygon(polygon, rule, Winding::Check(0.0), options)
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
        let limits = options.limits;
        if path.segments().len() > limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        let to_user = pen.inverse().ok_or(Error::unsupported(
            features::SINGULAR_PEN,
            "singular stroke pen is not implemented",
        ))?;
        let path = path.transformed(pen);
        let device = to_user.then(transform);
        let (outline, outline_transform, tiling) = match adjustment {
            Adjustment::Adjusted => {
                let (outline, t) = crate::adjust::outline(&path, device, stroke, limits)?;
                (outline, t, Tiling::Components)
            }
            Adjustment::None => crate::stroke::pieces(
                &path,
                device,
                stroke,
                limits,
                (Tiling::Unproven, crate::flatten::TOLERANCE),
            )?,
        };
        let mut polygon = match Polygon::from_lines(&outline, outline_transform, limits)? {
            Some(polygon) => polygon,
            None => Polygon::from_path(&outline, outline_transform, limits)?,
        };
        let winding = match tiling {
            Tiling::Proven => Winding::Certified,
            // The union's boundary, where it relinks, may still certify.
            Tiling::Unproven if polygon.cancel_shared()? => Winding::Check(0.0),
            Tiling::Unproven | Tiling::Components => Winding::Unknown,
        };
        Self::polygon(polygon, FillRule::NonZero, winding, options)
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
    ) -> Result<Option<Self>> {
        let Some(mut frame) = Frame::new(options) else {
            return Ok(None);
        };
        let Some(hull) = polygon.bounds() else {
            return Ok(None);
        };
        frame.fit(hull);
        if frame.rows.is_none()
            && let Some(rect) = rectangle(&polygon)
        {
            return Ok(Self::rect(rect, &frame, options));
        }
        // The heights the winding certificate leaves in doubt: none when
        // certified throughout, all when unknown.
        // A lone triangle bounds winding 0 or one sign of magnitude one,
        // whatever its vertices.
        let triangle =
            polygon.starts.len() == 1 && polygon.vertices() == 3 && !polygon.points_cells;
        let doubtful = match winding {
            Winding::Certified => Some(Vec::new()),
            Winding::Check(_) if triangle => Some(Vec::new()),
            Winding::Check(margin) => uncertified(&polygon, margin),
            Winding::Unknown => None,
        };
        let certain = doubtful.as_ref().is_some_and(Vec::is_empty);
        let width = super::row::width(options.area.region.x1)?;
        if certain && frame.rows.is_none() {
            // Nothing to analyse: the edges alone, clipped to the rectangle.
            let lines = edges::lines(&polygon)?;
            drop(polygon);
            let lines = match frame.rect {
                Some(r) => {
                    let mut clipped = Vec::new();
                    clipped
                        .try_reserve(lines.len())
                        .map_err(|_| Error::limit("raster geometry allocation failed"))?;
                    for line in lines {
                        edges::clip_line(line, r, &mut clipped)?;
                    }
                    clipped
                }
                None => lines,
            };
            let Some(bounds) = frame.bounds(lines.iter().flat_map(|l| [l.a, l.b])) else {
                return Ok(None);
            };
            let scan = scan_columns(lines.iter().flat_map(|l| [l.a.x, l.b.x]), width);
            return Self::edges(
                lines,
                Vec::new(),
                rule,
                (bounds, scan, Stored::default()),
                &frame,
                options,
            )
            .map(Some);
        }
        let mut chained = Vec::new();
        edges::chained(&polygon, 0, &mut 0, &mut chained)?;
        drop(polygon);
        let chained = frame.clip(chained)?;
        let Some(bounds) = frame.bounds(chained.iter().flat_map(|e| [e.line.a, e.line.b])) else {
            return Ok(None);
        };
        let scan = super::clip::scan_range(&chained, width);
        let stored = if !certain || frame.crossed(bounds) {
            // Rows that meet no doubtful slab keep their signed area.
            let row_certain = |y: u32| {
                doubtful.as_ref().is_some_and(|slabs| {
                    let (top, bottom) = (f64::from(y), f64::from(y) + 1.0);
                    !slabs.iter().any(|&[lo, hi]| lo < bottom && hi > top)
                })
            };
            analyse(
                &chained,
                rule,
                &row_certain,
                &frame,
                (bounds, scan),
                options,
            )?
        } else {
            Stored::default()
        };
        let lines = chained
            .iter()
            .filter(|e| !e.line.is_horizontal())
            .map(|e| e.line)
            .collect();
        Self::edges(
            lines,
            Vec::new(),
            rule,
            (bounds, scan, stored),
            &frame,
            options,
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
    ) -> Result<Option<Self>> {
        debug_assert!(frame.rows.is_none(), "pieces meet no clip rows");
        let (lines, curves) = match frame.rect {
            Some(r) => {
                let mut clipped = Vec::new();
                let mut kept = Vec::new();
                clipped
                    .try_reserve(lines.len() + curves.len())
                    .and_then(|()| kept.try_reserve(curves.len()))
                    .map_err(|_| Error::limit("raster geometry allocation failed"))?;
                for line in lines {
                    edges::clip_line(line, r, &mut clipped)?;
                }
                for q in curves {
                    curve::clip(q, r, &mut clipped, &mut kept)?;
                }
                (clipped, kept)
            }
            None => (lines, curves),
        };
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
        let lines = lines.into_iter().filter(|l| !l.is_horizontal()).collect();
        Self::edges(
            lines,
            curves,
            FillRule::NonZero,
            (bounds, scan, Stored::default()),
            frame,
            options,
        )
        .map(Some)
    }

    /// A placement of a certified glyph within `frame`, which keeps every
    /// pixel the glyph can touch: its relative edges and the origin, with
    /// no geometry work and no allocation.
    pub(crate) fn translated(
        edges: &std::sync::Arc<super::GlyphEdges>,
        relative: &super::glyph::Relative,
        origin: crate::Point,
        frame: &Frame<'_>,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        let o = origin;
        if let Some(r) = relative.rect {
            let rect = Bounds {
                left: r.left + o.x,
                top: r.top + o.y,
                right: r.right + o.x,
                bottom: r.bottom + o.y,
            };
            return Ok(Self::rect(rect, frame, options));
        }
        let [lo, hi] = relative.hull;
        let (lo, hi) = (
            crate::Point::new(lo.x + o.x, lo.y + o.y),
            crate::Point::new(hi.x + o.x, hi.y + o.y),
        );
        let Some(bounds) = frame.bounds([lo, hi].into_iter()) else {
            return Ok(None);
        };
        let width = super::row::width(options.area.region.x1)?;
        let scan = columns(lo.x, hi.x, width);
        // Rows scanned through the bitmap or whole in the grid.
        let columns = scan.1.saturating_sub(scan.0);
        let words = columns / 64 + 2;
        // In the grid each band it meets visits every edge once, at most
        // as often as it has rows.
        let items = relative.lines.len() + relative.curves.len();
        let rows = bounds.height() as usize;
        let work = relative
            .work
            .saturating_add(rows.saturating_mul(words + columns + 4))
            .saturating_add(rows.saturating_mul(items));
        if work > options.limits.raster_work {
            return Err(Error::limit(RASTER_WORK));
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "bounded origins lie within i64"
        )]
        let row0 = o.y.floor() as i64 + relative.first;
        Ok(Some(Self {
            area: options.area,
            bounds,
            work,
            retained: size_of::<Self>(),
            rule: FillRule::NonZero,
            body: Body::Glyph(GlyphBody {
                edges: std::sync::Arc::clone(edges),
                origin,
                row0,
                scan,
            }),
            clip: frame.clip.cloned(),
            overlay: false,
            stored: Stored::default(),
        }))
    }

    fn rect(rect: Bounds, frame: &Frame<'_>, options: &Prep<'_>) -> Option<Self> {
        let region = options.area.region;
        let mut rect = rect.intersect(Bounds {
            left: f64::from(region.x0),
            top: f64::from(region.y0),
            right: f64::from(region.x1),
            bottom: f64::from(region.y1),
        });
        if let Some(r) = frame.rect {
            rect = rect.intersect(r);
        }
        if rect.is_empty() {
            return None;
        }
        let bounds = rect.pixels(region).intersect(&frame.limit);
        if bounds.is_empty() {
            return None;
        }
        Some(Self {
            area: options.area,
            bounds,
            work: (bounds.height() as usize).saturating_mul(8),
            retained: size_of::<Self>(),
            rule: FillRule::NonZero,
            body: Body::Rect(rect),
            clip: frame.clip.cloned(),
            overlay: false,
            stored: Stored::default(),
        })
    }

    fn edges(
        lines: Vec<Line>,
        curves: Vec<Quadratic>,
        rule: FillRule,
        (bounds, scan, stored): (DeviceRect, (usize, usize), Stored),
        frame: &Frame<'_>,
        options: &Prep<'_>,
    ) -> Result<Self> {
        let area = options.area;
        let region = area.region;
        let right = f64::from(region.x1);
        let extent = |lo: f64, hi: f64| rows(lo, hi, bounds);
        let line_rows = |l: &Line| {
            if l.a.x.min(l.b.x) >= right {
                return None;
            }
            extent(l.a.y.min(l.b.y), l.a.y.max(l.b.y))
        };
        let curve_rows = |q: &Quadratic| {
            let [a, _, b] = q.points;
            if a.x.min(b.x) >= right {
                return None;
            }
            extent(a.y.min(b.y), a.y.max(b.y))
        };
        let count = lines.len() + curves.len();
        let item_rows = |i: usize| match lines.get(i) {
            Some(l) => line_rows(l),
            None => curves.get(i - lines.len()).and_then(curve_rows),
        };
        let columns = scan.1.saturating_sub(scan.0);
        // A mark of at most 16 rows and a bitmap word's width, with nothing
        // to overlay, goes into the grid: each band it meets visits every
        // item once, at most as often as it has rows, and each row it
        // crosses.
        let height = bounds.height() as usize;
        let grid = stored.len() == 0 && frame.rows.is_none() && columns <= 64 && height <= 16;
        let (index, mut work) = if grid {
            let visits = (0..count)
                .filter_map(item_rows)
                .fold(count.saturating_mul(height), |n, (a, b)| {
                    n.saturating_add((b - a) as usize + 1)
                });
            (None, visits)
        } else {
            let index = RowList::new((bounds.y0, bounds.y1), count, item_rows)?;
            let visits = index.len();
            (Some(index), visits)
        };
        let mut cells = 0_usize;
        for l in &lines {
            if let Some(r) = line_rows(l) {
                cells = cells.saturating_add(line_work(*l, r, region.x1 as usize));
            }
        }
        for q in &curves {
            if let Some(r) = curve_rows(q) {
                let [a, _, b] = q.points;
                cells = cells.saturating_add(line_work(Line { a, b }, r, region.x1 as usize));
            }
        }
        let rows = bounds.height() as usize;
        let words = (scan.1.saturating_sub(scan.0)) / 64 + 2;
        work = work
            .saturating_add(cells.saturating_mul(4))
            .saturating_add(rows.saturating_mul(words + columns + 4))
            .saturating_add(stored.len().saturating_mul(2))
            .saturating_add(frame.segments(bounds));
        if work > options.limits.raster_work {
            return Err(Error::limit(RASTER_WORK));
        }
        let body = EdgeBody {
            lines,
            curves,
            index,
            scan,
        };
        let retained = size_of::<Self>()
            .saturating_add(body.lines.capacity() * size_of::<Line>())
            .saturating_add(body.curves.capacity() * size_of::<Quadratic>())
            .saturating_add(body.index.as_ref().map_or(0, RowList::retained_bytes))
            .saturating_add(stored.retained_bytes());
        Ok(Self {
            area,
            bounds,
            work,
            retained,
            rule,
            body: Body::Edges(body),
            clip: frame.clip.cloned(),
            overlay: frame.rows.is_some(),
            stored,
        })
    }

    /// Adds `units` to the work bound.
    pub(crate) fn add_work(&mut self, units: usize) {
        self.work = self.work.saturating_add(units);
    }
}

/// The device rectangle a fill of `path` is when the path is one subpath of
/// four corners that `transform` keeps axis-aligned: the corners as the
/// flattened polygon holds them, so the fill's bytes do not change. `None`
/// for anything else, which goes through the polygon.
fn rect_fill(path: &Path, transform: Transform, limits: &pagura_core::Limits) -> Option<Bounds> {
    use crate::Segment;
    let segments = path.segments();
    if !transform.is_finite() || segments.len() > limits.path_segments {
        return None;
    }
    let mut corners = [crate::Point::default(); 5];
    let mut n = 0;
    for (i, segment) in segments.iter().enumerate() {
        match (i, *segment) {
            (0, Segment::MoveTo(p)) => {
                corners[0] = p;
                n = 1;
            }
            (1..=4, Segment::LineTo(p)) if n == i => {
                corners[n] = p;
                n += 1;
            }
            (_, Segment::Close) if i + 1 == segments.len() && i == n => {}
            _ => return None,
        }
    }
    // An explicit return to the first corner closes as a close does.
    if n == 5 && corners[4] == corners[0] {
        n = 4;
    }
    if n != 4 {
        return None;
    }
    let mut device = [crate::Point::default(); 4];
    for (d, &p) in device.iter_mut().zip(&corners[..4]) {
        *d = p * transform;
        crate::flatten::bounded(*d).ok()?;
    }
    let [a, b, c, d] = device;
    let axis = (a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
        || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x);
    (axis && a != b && b != c && c != d && d != a).then_some(Bounds {
        left: a.x.min(c.x),
        top: a.y.min(c.y),
        right: a.x.max(c.x),
        bottom: a.y.max(c.y),
    })
}

/// The columns cells of edges with these x can be touched in.
fn scan_columns(xs: impl Iterator<Item = f64>, width: usize) -> (usize, usize) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for x in xs {
        lo = lo.min(x);
        hi = hi.max(x);
    }
    columns(lo, hi, width)
}

/// The region and clip a shape is prepared in.
pub(crate) struct Frame<'a> {
    pub(crate) clip: Option<&'a ClipShape>,
    pub(crate) rect: Option<Bounds>,
    pub(crate) rows: Option<&'a Rows>,
    /// The pixels the clip keeps within the region.
    pub(crate) limit: DeviceRect,
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
            rows: clip.and_then(ClipShape::rows),
            limit,
        })
    }

    /// Drops what the clip cannot change for geometry within `hull`: a
    /// rectangle containing it needs no edge clipping, and rows inside
    /// across every pixel it can touch need no overlay or analysis. A mark
    /// within the page crop thus meets no clip at all.
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
        if self
            .rows
            .is_some_and(|rows| rows.inside(edges::pixels([lo, hi], self.limit, false)))
        {
            self.rows = None;
        }
    }

    /// The edges clipped to the clip's rectangle.
    pub(crate) fn clip(&self, chained: Vec<Chained>) -> Result<Vec<Chained>> {
        let Some(r) = self.rect else {
            return Ok(chained);
        };
        let mut clipped = Vec::new();
        clipped
            .try_reserve(chained.len())
            .map_err(|_| Error::limit("raster geometry allocation failed"))?;
        for edge in chained {
            edges::clip(edge, r, &mut clipped)?;
        }
        Ok(clipped)
    }

    /// The pixels geometry with these points can cover within the clip.
    pub(crate) fn bounds(&self, points: impl Iterator<Item = crate::Point>) -> Option<DeviceRect> {
        let mut lo = crate::Point::new(f64::INFINITY, f64::INFINITY);
        let mut hi = crate::Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in points {
            lo.x = lo.x.min(p.x);
            lo.y = lo.y.min(p.y);
            hi.x = hi.x.max(p.x);
            hi.y = hi.y.max(p.y);
        }
        if lo.x > hi.x || lo.y > hi.y {
            return None;
        }
        let bounds = edges::pixels([lo, hi], self.limit, false);
        (!bounds.is_empty()).then_some(bounds)
    }

    /// Whether a clip boundary cell lies within `bounds`.
    fn crossed(&self, bounds: DeviceRect) -> bool {
        self.rows.is_some_and(|rows| {
            (bounds.y0..bounds.y1).any(|y| {
                rows.row(y)
                    .iter()
                    .any(|s| s.cell != INSIDE && s.x0 < bounds.x1 && s.x1 > bounds.x0)
            })
        })
    }

    /// The clip segments of the rows of `bounds`.
    pub(crate) fn segments(&self, bounds: DeviceRect) -> usize {
        self.rows.map_or(0, |rows| {
            (bounds.y0..bounds.y1)
                .map(|y| rows.row(y).len())
                .fold(0, usize::saturating_add)
        })
    }
}

/// The heights of row `y` within `rect`, from the row's top.
pub(crate) fn span(rect: Option<Bounds>, y: u32) -> (f64, f64) {
    rect.map_or((0.0, 1.0), |r| {
        let top = f64::from(y);
        let lo = (r.top - top).clamp(0.0, 1.0);
        let hi = (r.bottom - top).clamp(0.0, 1.0);
        (lo, hi.max(lo))
    })
}

/// Resolves the cells of `chained` that its signed area does not
/// determine: cells where two chains meet in an uncertified shape, cells of
/// rows the clip's rectangle cuts, and cells where it and a clip boundary
/// meet.
fn analyse(
    chained: &[Chained],
    rule: FillRule,
    certain: &dyn Fn(u32) -> bool,
    frame: &Frame<'_>,
    (bounds, scan): (DeviceRect, (usize, usize)),
    options: &Prep<'_>,
) -> Result<Stored> {
    let mut meter = Meter::new(options.limits.raster_work, RASTER_WORK);
    let mut scanner = Scanner::new(scan, &mut meter)?;
    let index = RowIndex::new(chained, (bounds.y0, bounds.y1 - 1), &mut meter)?;
    let mut stored = Stored::default();
    for y in bounds.y0..bounds.y1 {
        let segments = frame.rows.map(|rows| rows.row(y));
        let joint = segments.is_some_and(|s| {
            s.iter()
                .any(|s| s.cell != INSIDE && s.x0 < bounds.x1 && s.x1 > bounds.x0)
        });
        let certain = certain(y);
        if certain && !joint {
            continue;
        }
        let context = Context {
            rule,
            certain,
            bounds,
            rows: frame.rows,
            segments,
            y,
        };
        let result = scanner
            .load(
                chained,
                (index.row(y), y, span(frame.rect, y)),
                scan,
                &mut meter,
            )
            .and_then(|()| analyse_row(&mut scanner, &context, &mut stored, &mut meter));
        scanner.unload();
        if let Err(error) = result {
            scanner.recover();
            return Err(error);
        }
    }
    Ok(stored)
}

/// What one row's analysis needs besides the scanner.
struct Context<'a> {
    rule: FillRule,
    certain: bool,
    bounds: DeviceRect,
    rows: Option<&'a Rows>,
    segments: Option<&'a [super::clip::Segment]>,
    y: u32,
}

impl Context<'_> {
    /// The clip's state at columns `x..end`: the run it is in (inside, a
    /// boundary cell, or `None` outside) and where that run ends.
    fn clip(&self, x: u32, end: u32, cursor: &mut usize) -> (Option<u32>, u32) {
        let Some(segments) = self.segments else {
            return (Some(INSIDE), end);
        };
        while segments.get(*cursor).is_some_and(|s| s.x1 <= x) {
            *cursor += 1;
        }
        match segments.get(*cursor) {
            None => (None, end),
            Some(s) if s.x0 > x => (None, end.min(s.x0)),
            Some(s) => (Some(s.cell), end.min(s.x1)),
        }
    }
}

fn analyse_row(
    scanner: &mut Scanner,
    context: &Context<'_>,
    stored: &mut Stored,
    meter: &mut Meter,
) -> Result<()> {
    let bounds = context.bounds;
    let mut cursor = 0;
    let mut cover = 0.0;
    let mut position = bounds.x0;
    for index in 0..scanner.cells.len() {
        let cell = scanner.cells[index];
        if cell.x >= bounds.x1 {
            break;
        }
        if cell.x > position {
            gap(
                scanner,
                context,
                (position, cell.x, cover),
                &mut cursor,
                stored,
                meter,
            )?;
        }
        if cell.x >= bounds.x0 {
            if cell.listed {
                listed(scanner, context, &cell, &mut cursor, stored, meter)?;
            } else {
                gap(
                    scanner,
                    context,
                    (cell.x, cell.x + 1, cell.cover),
                    &mut cursor,
                    stored,
                    meter,
                )?;
            }
        }
        cover = cell.cover;
        position = position.max(cell.x + 1);
    }
    if position < bounds.x1 {
        gap(
            scanner,
            context,
            (position, bounds.x1, cover),
            &mut cursor,
            stored,
            meter,
        )?;
    }
    Ok(())
}

/// Cells `x0..x1` that no piece crosses, into which `cover` is carried:
/// their winding is constant over the row's span.
fn gap(
    scanner: &mut Scanner,
    context: &Context<'_>,
    (x0, x1, cover): (u32, u32, f64),
    cursor: &mut usize,
    stored: &mut Stored,
    meter: &mut Meter,
) -> Result<()> {
    let span = scanner.span;
    let winding = gap_winding(cover, span);
    let covered = inside(winding, context.rule);
    let exact = if covered {
        crate::quantize_unit(span.1 - span.0)
    } else {
        0
    };
    let folded = fold(cover, context.rule);
    let partial = covered && span != (0.0, 1.0);
    let mut x = x0;
    while x < x1 {
        let (state, end) = context.clip(x, x1, cursor);
        meter.charge(1)?;
        match state {
            Some(INSIDE) => {
                if exact != folded {
                    for column in x..end {
                        stored.push(context.y, column, exact)?;
                    }
                }
            }
            Some(boundary) if partial => {
                let (_, edges, rules) = context
                    .rows
                    .and_then(|r| r.cell(boundary))
                    .ok_or(Error::invalid_argument("clip cell out of range"))?;
                let value =
                    scanner.resolve_joint((None, cover), context.rule, (edges, rules), meter)?;
                stored.push(context.y, x, value)?;
            }
            None | Some(_) => {}
        }
        x = end.max(x + 1);
    }
    Ok(())
}

/// A cell that pieces cross.
fn listed(
    scanner: &mut Scanner,
    context: &Context<'_>,
    cell: &super::scan::Cell,
    cursor: &mut usize,
    stored: &mut Stored,
    meter: &mut Meter,
) -> Result<()> {
    let (state, _) = context.clip(cell.x, cell.x + 1, cursor);
    let value = match state {
        None => return Ok(()),
        Some(INSIDE) => {
            if context.certain || cell.certain {
                return Ok(());
            }
            match scanner.walk(cell, context.rule, meter)? {
                Walk::Fold => return Ok(()),
                Walk::Value(value) => value,
                Walk::Unknown => scanner.resolve(cell, context.rule, meter)?,
            }
        }
        Some(boundary) => {
            let (_, edges, rules) = context
                .rows
                .and_then(|r| r.cell(boundary))
                .ok_or(Error::invalid_argument("clip cell out of range"))?;
            scanner.resolve_joint((Some(cell), 0.0), context.rule, (edges, rules), meter)?
        }
    };
    stored.push(context.y, cell.x, value)
}
