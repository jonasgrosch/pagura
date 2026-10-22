//! Clip states (8.5.4) prepared once per render.
//!
//! The axis-aligned rectangles among a state's paths intersect into one
//! rectangle, which clips every mark's edges. The other paths keep their
//! edges, listed per strip, and their rows classified into inside runs and
//! boundary runs with their coverage: rows a single certified path crosses
//! take its signed area, the others go through the exact row resolver. A
//! mark whose row meets a boundary run is resolved there together with the
//! clip's paths, so that their joint geometry is integrated rather than
//! multiplied.

use super::cells::{Cells, STRIP};
use super::certify::uncertified;
use super::edges::{self, Bounds, Line, Polygon, StripList};
use super::rect::rectangle;
use super::row::Sink;
use super::shape::RENDER_BYTES;
use super::union::{Meters, Piece, Resolver, room};
use super::{DeviceRect, Prep, ShapeOptions};
use crate::{FillRule, Path, Transform};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

/// The message of every clip preparation budget error.
const CLIP_WORK: &str = "clip raster work budget exceeded";

/// A clip state prepared once per render: the intersection of a chain of
/// clipping paths, classified per row into outside, inside and boundary
/// runs. A rectangular clip becomes edge clipping. Cloning shares the
/// prepared state.
#[derive(Debug, Clone)]
pub struct ClipShape {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    bounds: DeviceRect,
    rect: Option<Bounds>,
    paths: Option<Paths>,
    work: usize,
    retained: usize,
}

/// The clip's paths other than rectangles: their edges, clipped to the
/// rectangle, and their classified rows.
#[derive(Debug)]
pub(crate) struct Paths {
    /// Non-horizontal edges, path by path.
    lines: Vec<Line>,
    /// Per path: the end of its edges in `lines`, and its rule.
    ends: Vec<(u32, FillRule)>,
    /// Intersection of canonical path bounds, prepared once for local
    /// curve interval proofs. Includes all rows, not only a strip's edges.
    proof: Option<Bounds>,
    strips: StripList,
    pub(crate) rows: Rows,
}

impl Paths {
    /// Outward rectangles containing each canonical path's covered set,
    /// intersected. A curve can affect the joint area only inside them.
    pub(crate) fn proof_bounds(&self) -> Option<Bounds> {
        self.proof
    }

    fn calculate_bounds(&self) -> Option<Bounds> {
        let mut first = 0;
        self.ends
            .iter()
            .filter_map(|&(end, _)| {
                let lines = self.lines.get(first..end as usize).unwrap_or(&[]);
                first = end as usize;
                let mut points = lines.iter().flat_map(|line| [line.a, line.b]);
                let p = points.next()?;
                Some(points.fold(
                    Bounds {
                        left: p.x,
                        top: p.y,
                        right: p.x,
                        bottom: p.y,
                    },
                    |b, p| Bounds {
                        left: b.left.min(p.x),
                        top: b.top.min(p.y),
                        right: b.right.max(p.x),
                        bottom: b.bottom.max(p.y),
                    },
                ))
            })
            .reduce(Bounds::intersect)
    }

    /// The rules of the paths, in order.
    pub(crate) fn rules(&self) -> impl Iterator<Item = FillRule> + '_ {
        self.ends.iter().map(|&(_, rule)| rule)
    }

    /// Appends the pieces of row `y` of every path, as groups `first`,
    /// `first + 1`, …, charging the room they take to `bytes`.
    pub(crate) fn pieces(
        &self,
        y: u32,
        first: u8,
        (out, bytes): (&mut Vec<Piece>, &mut Meter),
    ) -> Result<()> {
        let top = f64::from(y);
        let listed = self.strips.strip(y / STRIP);
        room(out, listed.len(), bytes)?;
        for &item in listed {
            let Some(&line) = self.lines.get(item as usize) else {
                continue;
            };
            let path = self.ends.partition_point(|&(end, _)| end <= item);
            let group = u8::try_from(path)
                .ok()
                .and_then(|g| g.checked_add(first))
                .ok_or(Error::limit("clip path count exceeds u8"))?;
            out.extend(Piece::new(line, top, group));
        }
        Ok(())
    }

    fn retained_bytes(&self) -> usize {
        (self.lines.capacity() * size_of::<Line>())
            .saturating_add(self.ends.capacity() * size_of::<(u32, FillRule)>())
            .saturating_add(self.strips.retained_bytes())
            .saturating_add(self.rows.retained_bytes())
    }
}

/// A run of one clip row: pixels inside, or boundary pixels with their
/// coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Segment {
    pub(crate) x0: u32,
    pub(crate) x1: u32,
    /// The coverage of each pixel of the run.
    pub(crate) value: u8,
    /// Whether the run lies wholly inside: a mark keeps its own coverage
    /// there; elsewhere the clip's boundary crosses or cuts every pixel.
    pub(crate) inside: bool,
}

/// The classified rows of a clip's paths.
#[derive(Debug, Default)]
pub(crate) struct Rows {
    index: Vec<(u32, u32)>,
    segments: Vec<Segment>,
}

impl Rows {
    /// Whether every pixel of `pixels` lies in an inside run.
    pub(crate) fn inside(&self, pixels: DeviceRect) -> bool {
        (pixels.y0..pixels.y1).all(|y| {
            self.row(y)
                .iter()
                .any(|s| s.inside && s.x0 <= pixels.x0 && s.x1 >= pixels.x1)
        })
    }

    /// Whether a boundary run of row `y` meets columns `x0..x1`.
    pub(crate) fn crosses(&self, y: u32, (x0, x1): (u32, u32)) -> bool {
        self.row(y)
            .iter()
            .any(|s| !s.inside && s.x0 < x1 && s.x1 > x0)
    }

    /// The runs of row `y`, left to right; pixels between them are outside.
    pub(crate) fn row(&self, y: u32) -> &[Segment] {
        let index = self.index.partition_point(|&(row, _)| row < y);
        match self.index.get(index) {
            Some(&(row, end)) if row == y => {
                let start = index.checked_sub(1).map_or(0, |i| self.index[i].1);
                self.segments
                    .get(start as usize..end as usize)
                    .unwrap_or(&[])
            }
            _ => &[],
        }
    }

    /// Segments over all rows.
    pub(crate) fn len(&self) -> usize {
        self.segments.len()
    }

    fn retained_bytes(&self) -> usize {
        (self.index.capacity() * size_of::<(u32, u32)>())
            .saturating_add(self.segments.capacity() * size_of::<Segment>())
    }

    fn push(&mut self, y: u32, segment: Segment, bytes: &mut Meter) -> Result<()> {
        if let Some(last) = self.segments.last_mut()
            && self.index.last().is_some_and(|&(row, _)| row == y)
            && last.x1 == segment.x0
            && last.value == segment.value
            && last.inside == segment.inside
        {
            last.x1 = segment.x1;
            return Ok(());
        }
        room(&mut self.segments, 1, bytes)?;
        room(&mut self.index, 1, bytes)?;
        self.segments.push(segment);
        let end =
            u32::try_from(self.segments.len()).map_err(|_| Error::limit("clip rows exceed u32"))?;
        match self.index.last_mut() {
            Some(last) if last.0 == y => last.1 = end,
            _ => self.index.push((y, end)),
        }
        Ok(())
    }
}

/// The fill a clipping path of `path` under `transform` and `rule` paints:
/// a certified empty region becomes its hairline outline (10.7.4).
pub(crate) fn fill_polygon(
    path: &Path,
    transform: Transform,
    rule: FillRule,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<(Polygon, FillRule)> {
    if path.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    if !transform.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    if let Some(outline) =
        crate::empty_fill::classified_outline(path, transform, rule, limits, bytes)?
    {
        let polygon = Polygon::from_path(&outline, Transform::IDENTITY, limits, bytes)?;
        let released = outline.retained_bytes();
        drop(outline);
        bytes.refund(released);
        return Ok((polygon, FillRule::NonZero));
    }
    let polygon = match Polygon::from_lines(path, transform, limits, bytes)? {
        Some(polygon) => polygon,
        None => Polygon::from_path(path, transform, limits, bytes)?,
    };
    Ok((polygon, rule))
}

impl ClipShape {
    /// The intersection of `paths` (user space, outermost first) under
    /// `transform`.
    ///
    /// # Errors
    ///
    /// As [`Shape::fill`](super::Shape::fill), plus a `Limit` error beyond
    /// `mask_sources`.
    pub fn new(
        paths: &[(&Path, FillRule)],
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Self> {
        let options = &Prep::new(options, None);
        let limits = options.limits;
        if paths.len() > limits.mask_sources {
            return Err(Error::limit("mask source budget exceeded"));
        }
        let region = options.area.region;
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let mut rect: Option<Bounds> = None;
        let mut groups: Vec<(Polygon, FillRule)> = Vec::new();
        for &(path, rule) in paths {
            let (polygon, rule) = fill_polygon(path, transform, rule, limits, &mut bytes)?;
            if let Some(r) = rectangle(&polygon) {
                rect = Some(rect.map_or(r, |b| b.intersect(r)));
                let released = polygon.heap_bytes();
                drop(polygon);
                bytes.refund(released);
            } else {
                bytes.reserve_capacity(&mut groups, 1)?;
                groups.push((polygon, rule));
            }
        }
        let mut bounds = region;
        if let Some(r) = rect {
            bounds = bounds.intersect(&r.pixels(region));
        }
        for (polygon, _) in &groups {
            let pixels = polygon
                .bounds()
                .map_or_else(DeviceRect::default, |b| edges::pixels(b, region, false));
            bounds = bounds.intersect(&pixels);
        }
        // Alone, a rectangle's two sides go through the cells: per row at
        // most two visits, six cells, four touched and the bitmap words.
        let words = bounds.x1 as usize / 64 + 2;
        bytes.charge(size_of::<Inner>())?;
        let mut inner = Inner {
            bounds,
            rect,
            paths: None,
            work: (bounds.height() as usize).saturating_mul(words + 24),
            retained: size_of::<Inner>(),
        };
        if !groups.is_empty() && !bounds.is_empty() {
            let paths = classify(&groups, rect, bounds, options, &mut bytes)?;
            inner.work = paths
                .rows
                .len()
                .saturating_add(bounds.height() as usize * 2);
            inner.retained = inner.retained.saturating_add(paths.retained_bytes());
            inner.paths = Some(paths);
        }
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// The pixels the clip can keep within the area; empty when it removes
    /// everything.
    pub fn bounds(&self) -> DeviceRect {
        self.inner.bounds
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.inner.retained
    }

    /// Work units rasterizing the clip alone over every band costs at most.
    pub fn work(&self) -> usize {
        self.inner.work
    }

    pub(crate) fn rect(&self) -> Option<Bounds> {
        self.inner.rect
    }

    pub(crate) fn paths(&self) -> Option<&Paths> {
        self.inner.paths.as_ref()
    }

    pub(crate) fn rows(&self) -> Option<&Rows> {
        self.inner.paths.as_ref().map(|p| &p.rows)
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

/// Straight boundaries and uncertain slabs before the row classification.
struct Boundaries {
    lines: Vec<Line>,
    ends: Vec<(u32, FillRule)>,
    doubtful: Option<Vec<[f64; 2]>>,
}

/// Flattens the clip's groups into directed boundaries, accounting for the
/// original polygons, temporary line copies and certificate scratch.
fn boundaries(
    groups: &[(Polygon, FillRule)],
    rect: Option<Bounds>,
    meter: &mut Meter,
    bytes: &mut Meter,
) -> Result<Boundaries> {
    let mut lines: Vec<Line> = Vec::new();
    let mut ends = Vec::new();
    bytes.reserve(&mut ends, groups.len())?;
    // Rows a single certified path settles by its signed area.
    let mut doubtful: Option<Vec<[f64; 2]>> = None;
    for (polygon, rule) in groups {
        meter.charge(polygon.vertices())?;
        let source_bytes = polygon.vertices().saturating_mul(size_of::<Line>());
        let source = edges::lines(polygon, bytes)?;
        for &line in &source {
            if let Some(r) = rect {
                room(&mut lines, 3, bytes)?;
                edges::clip_line(line, r, &mut lines)?;
            } else {
                room(&mut lines, 1, bytes)?;
                lines.push(line);
            }
        }
        drop(source);
        bytes.refund(source_bytes);
        lines.retain(|l| !l.is_horizontal());
        let end = u32::try_from(lines.len()).map_err(|_| Error::limit("clip edges exceed u32"))?;
        ends.push((end, *rule));
        if groups.len() == 1 {
            doubtful = uncertified(polygon, 0.0, bytes)?;
        }
    }
    Ok(Boundaries {
        lines,
        ends,
        doubtful,
    })
}

/// Classifies the rows of `bounds` for the non-rectangular `groups`.
fn classify(
    groups: &[(Polygon, FillRule)],
    rect: Option<Bounds>,
    bounds: DeviceRect,
    options: &Prep<'_>,
    bytes: &mut Meter,
) -> Result<Paths> {
    let limits = options.limits;
    let mut meter = Meter::new(limits.raster_work, CLIP_WORK);
    let Boundaries {
        lines,
        ends,
        doubtful,
    } = boundaries(groups, rect, &mut meter, bytes)?;
    let rows_of = |l: &Line| edges::rows(l.a.y.min(l.b.y), l.a.y.max(l.b.y), bounds);
    let strips_list = StripList::new(
        (bounds.y0, bounds.y1),
        lines.len(),
        |i| lines.get(i).and_then(rows_of),
        bytes,
    )?;
    meter.charge(strips_list.len())?;
    let mut paths = Paths {
        lines,
        ends,
        proof: None,
        strips: strips_list,
        rows: Rows::default(),
    };
    meter.charge(paths.lines.len().saturating_add(paths.ends.len()))?;
    paths.proof = paths.calculate_bounds();
    let mut rules = Vec::new();
    bytes.reserve(&mut rules, paths.ends.len())?;
    rules.extend(paths.rules());
    let width = super::row::width(bounds.x1)?;
    let scan = scan_columns(&paths.lines, width);
    let mut cells = Cells::new(scan.1.saturating_sub(scan.0).max(1), bytes)?;
    let mut resolver = Resolver::default();
    let (mut pieces, mut sides) = (Vec::new(), Vec::new());
    let mut rows = Rows::default();
    for y in bounds.y0..bounds.y1 {
        let top = f64::from(y);
        let settled = doubtful
            .as_ref()
            .is_some_and(|slabs| !slabs.iter().any(|&[lo, hi]| lo < top + 1.0 && hi > top));
        pieces.clear();
        paths.pieces(y, 0, (&mut pieces, bytes))?;
        meter.charge(pieces.len().saturating_add(2))?;
        if !cells.open(scan, y..y + 1) {
            return Err(Error::limit(CLIP_WORK));
        }
        if settled {
            for p in &pieces {
                meter.charge(cells.line(p.line))?;
            }
        } else {
            sides.clear();
            let meters = Meters {
                work: &mut meter,
                bytes,
            };
            resolver.row(&pieces, &rules, top, (&mut sides, meters))?;
            for &line in &sides {
                meter.charge(cells.line(line))?;
            }
        }
        let mut sink = Classify {
            rows: &mut rows,
            bytes,
            y,
            bounds: (bounds.x0, bounds.x1),
            error: None,
        };
        let touched = cells.emit(0, FillRule::NonZero, &mut sink);
        if let Some(error) = sink.error {
            return Err(error);
        }
        meter.charge(touched.saturating_add(scan.1.saturating_sub(scan.0) / 64 + 2))?;
    }
    paths.rows = rows;
    Ok(paths)
}

/// Builds a clip row's runs from its emitted cells: touched cells and
/// partial runs are boundary, full runs inside, empty ones outside.
struct Classify<'a> {
    rows: &'a mut Rows,
    bytes: &'a mut Meter,
    y: u32,
    bounds: (u32, u32),
    error: Option<Error>,
}

impl Classify<'_> {
    fn push(&mut self, (x0, x1): (usize, usize), value: u8, inside: bool) {
        let a = u32::try_from(x0).unwrap_or(u32::MAX).max(self.bounds.0);
        let b = u32::try_from(x1).unwrap_or(u32::MAX).min(self.bounds.1);
        if a >= b || value == 0 || self.error.is_some() {
            return;
        }
        let segment = Segment {
            x0: a,
            x1: b,
            value,
            inside,
        };
        if let Err(error) = self.rows.push(self.y, segment, self.bytes) {
            self.error = Some(error);
        }
    }
}

impl Sink for Classify<'_> {
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        self.push((x0, x1), value, value == u8::MAX);
    }

    fn cells(&mut self, x: usize, values: &[u8]) {
        for (k, &value) in values.iter().enumerate() {
            self.push((x + k, x + k + 1), value, false);
        }
    }
}

/// The columns cells of `lines` can be touched in: from the leftmost (or
/// the page's left side) to one past the rightmost.
pub(crate) fn scan_columns(lines: &[Line], width: usize) -> (usize, usize) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for l in lines {
        lo = lo.min(l.a.x).min(l.b.x);
        hi = hi.max(l.a.x).max(l.b.x);
    }
    columns(lo, hi, width)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the row width before conversion"
)]
pub(crate) fn columns(lo: f64, hi: f64, width: usize) -> (usize, usize) {
    if lo > hi {
        return (0, 0);
    }
    let w = width as f64;
    let from = lo.floor().clamp(0.0, w) as usize;
    let to = (hi.ceil() + 1.0).clamp(0.0, w) as usize;
    (from, to.max(from))
}
