//! Marks prepared for the band rasterizer: fills, strokes and glyph
//! placements in device space, clipped by their clip state's rectangle,
//! with the cells their signed area does not determine already resolved.

use super::approximate::Cells;
use super::binned::Placement;
use super::certify::certified;
use super::clip::{ClipShape, INSIDE, Rows, columns, fill_polygon};
use super::curve::{self, Quadratic};
use super::edges::{self, Bounds, Chained, Line, Polygon, RowList, line_work, rows};
use super::rect::rectangle;
use super::scan::{RowIndex, Scanner, Stored, Walk, gap_winding, inside};
use super::row::fold;
use super::{Adjustment, Area, DeviceRect, Precision, Prep, ShapeOptions};
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
    pub(crate) stored: Stored,
    /// A clip multiplied into the coverage (approximate precision).
    pub(crate) multiplied: Option<ClipShape>,
}

/// How a shape's rows are produced.
#[derive(Debug)]
pub(crate) enum Body {
    /// An axis-aligned rectangle in closed form, within the region.
    Rect(Bounds),
    /// Edges accumulated into the row.
    Edges(EdgeBody),
    /// Approximate signed cells.
    Cells(Cells),
    /// A glyph mask reused per origin bin.
    Binned(Placement),
}

/// Straight edges and monotone quadratics listed by band, and the columns
/// their cells can be touched in.
#[derive(Debug, Default)]
pub(crate) struct EdgeBody {
    pub(crate) lines: Vec<Line>,
    pub(crate) curves: Vec<Quadratic>,
    pub(crate) index: RowList,
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
        if options.precision == Precision::Approximate {
            return super::approximate::fill(path, transform, rule, options);
        }
        let (polygon, rule) = fill_polygon(path, transform, rule, None, options.limits)?;
        Self::polygon(polygon, rule, Winding::Check(0.0), options)
    }

    /// A stroke of `path` (user space) with `stroke` whose line width and
    /// dashes are measured in pen space; `pen` maps pen space to user space
    /// (the CTM when the stroke was painted), and `transform` user space to
    /// device space. Outside any clip; `None` when it covers no pixel of the
    /// area.
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
        Self::stroke_in(path, stroke, (pen, transform), adjustment, &Prep::new(options, None))
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
        Self::stroke_in(path, stroke, (pen, transform), adjustment, &Prep::new(options, clip))
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
        let inverse = pen.inverse().ok_or(Error::unsupported(
            features::SINGULAR_PEN,
            "singular stroke pen is not implemented",
        ))?;
        let path = path.transformed(inverse);
        let device = pen.then(transform);
        let (outline, outline_transform, pieces) = match adjustment {
            Adjustment::Adjusted => {
                let (outline, t) = crate::adjust::outline(&path, device, stroke, limits)?;
                (outline, t, false)
            }
            Adjustment::None => crate::stroke::pieces(&path, device, stroke, limits)?,
        };
        if options.precision == Precision::Approximate {
            return super::approximate::stroke(&outline, outline_transform, pieces, options);
        }
        let polygon = Polygon::from_path(&outline, outline_transform, None, limits)?;
        let winding = if pieces {
            Winding::Certified
        } else {
            Winding::Unknown
        };
        Self::polygon(polygon, FillRule::NonZero, winding, options)
    }

    /// One glyph placement outside any clip: `edges` translated to the
    /// device-space `origin`; `None` when it covers no pixel of the area.
    /// `origins` asks for origin bins, which need edges from
    /// [`GlyphEdges::binned`](super::GlyphEdges::binned); edges from
    /// [`GlyphEdges::new`](super::GlyphEdges::new) always place exactly, and
    /// binned edges always in bins.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn glyph(
        edges: &std::sync::Arc<super::GlyphEdges>,
        origin: crate::Point,
        origins: super::GlyphOrigins,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        Self::glyph_with_clip(edges, origin, origins, None, options)
    }

    /// A glyph placement in the clip state `clip`, as
    /// [`Self::fill_with_clip`]. Binned placements multiply the clip's
    /// coverage in.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn glyph_with_clip(
        edges: &std::sync::Arc<super::GlyphEdges>,
        origin: crate::Point,
        origins: super::GlyphOrigins,
        clip: Option<&ClipShape>,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        super::glyph::place(edges, origin, origins, &Prep::new(options, clip))
    }

    #[cfg(test)]
    pub(crate) fn glyph_in(
        edges: &std::sync::Arc<super::GlyphEdges>,
        origin: crate::Point,
        origins: super::GlyphOrigins,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        super::glyph::place(edges, origin, origins, options)
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
        let Some(frame) = Frame::new(options) else {
            return Ok(None);
        };
        if frame.rows.is_none()
            && let Some(rect) = rectangle(&polygon)
        {
            return Ok(Self::rect(rect, &frame, options));
        }
        let certain = match winding {
            Winding::Certified => true,
            Winding::Check(margin) => certified(&polygon, margin),
            Winding::Unknown => false,
        };
        let width = super::row::width(options.area.region.x1)?;
        if certain && frame.rows.is_none() {
            // Nothing to analyse: the edges alone, clipped to the rectangle.
            let lines = edges::lines(&polygon)?;
            drop(polygon);
            let lines = match frame.rect {
                Some(r) => {
                    let mut clipped = Vec::new();
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
            return Self::edges(lines, Vec::new(), rule, (bounds, scan, Stored::default()), &frame, options)
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
            analyse(&chained, rule, certain, &frame, (bounds, scan), options)?
        } else {
            Stored::default()
        };
        let lines = chained
            .iter()
            .filter(|e| !e.line.is_horizontal())
            .map(|e| e.line)
            .collect();
        Self::edges(lines, Vec::new(), rule, (bounds, scan, stored), &frame, options).map(Some)
    }

    /// A shape of certified lines and monotone quadratics, whose clip has no
    /// paths other than rectangles.
    pub(crate) fn pieces(
        lines: Vec<Line>,
        curves: Vec<Quadratic>,
        options: &Prep<'_>,
    ) -> Result<Option<Self>> {
        let Some(frame) = Frame::new(options) else {
            return Ok(None);
        };
        let (lines, curves) = match frame.rect {
            Some(r) => {
                let mut clipped = Vec::new();
                let mut kept = Vec::new();
                for line in lines {
                    let edge = Chained {
                        line,
                        chain: 0,
                        group: 0,
                        enclosed: false,
                        place: edges::Place::default(),
                    };
                    let mut out = Vec::new();
                    edges::clip(edge, r, &mut out)?;
                    clipped.extend(out.into_iter().map(|e| e.line));
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
            &frame,
            options,
        )
        .map(Some)
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
            stored: Stored::default(),
            multiplied: None,
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
        let index = RowList::new((bounds.y0, bounds.y1), count, |i| match lines.get(i) {
            Some(l) => line_rows(l),
            None => curves.get(i - lines.len()).and_then(curve_rows),
        })?;
        let mut work = index.len();
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
            .saturating_add(rows.saturating_mul(words + 2))
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
            .saturating_add(body.index.retained_bytes())
            .saturating_add(stored.retained_bytes());
        Ok(Self {
            area,
            bounds,
            work,
            retained,
            rule,
            body: Body::Edges(body),
            clip: frame.clip.cloned(),
            stored,
            multiplied: None,
        })
    }

    /// A shape with a ready body, for the approximate and binned producers.
    pub(crate) fn with_body(
        body: Body,
        bounds: DeviceRect,
        (rule, work, retained): (FillRule, usize, usize),
        options: &Prep<'_>,
    ) -> Self {
        Self {
            area: options.area,
            bounds,
            work,
            retained: size_of::<Self>().saturating_add(retained),
            rule,
            body,
            clip: options.clip.cloned(),
            stored: Stored::default(),
            multiplied: None,
        }
    }

    /// Adds `units` to the work bound.
    pub(crate) fn add_work(&mut self, units: usize) {
        self.work = self.work.saturating_add(units);
    }
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
                rows.row(y).iter().any(|s| {
                    s.cell != INSIDE && s.x0 < bounds.x1 && s.x1 > bounds.x0
                })
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
    certain: bool,
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
            .load(chained, (index.row(y), y, span(frame.rect, y)), scan, &mut meter)
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
            gap(scanner, context, (position, cell.x, cover), &mut cursor, stored, meter)?;
        }
        if cell.x >= bounds.x0 {
            if cell.listed {
                listed(scanner, context, &cell, &mut cursor, stored, meter)?;
            } else {
                gap(scanner, context, (cell.x, cell.x + 1, cell.cover), &mut cursor, stored, meter)?;
            }
        }
        cover = cell.cover;
        position = position.max(cell.x + 1);
    }
    if position < bounds.x1 {
        gap(scanner, context, (position, bounds.x1, cover), &mut cursor, stored, meter)?;
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
            None => {}
            Some(INSIDE) => {
                if exact != folded {
                    for column in x..end {
                        stored.push(context.y, column, exact)?;
                    }
                }
            }
            Some(boundary) => {
                if partial {
                    let (_, edges, rules) = context
                        .rows
                        .and_then(|r| r.cell(boundary))
                        .ok_or(Error::invalid_argument("clip cell out of range"))?;
                    let value =
                        scanner.resolve_joint((None, cover), context.rule, (edges, rules), meter)?;
                    stored.push(context.y, x, value)?;
                }
            }
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
