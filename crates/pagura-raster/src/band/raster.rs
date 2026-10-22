//! The band rasterizer: one dense cell row, reused for every mark and band.

use super::approximate::{self, Multiplied};
use super::binned;
use super::clip::{INSIDE, Rows, Segment};
use super::curve::Quadratic;
use super::edges::Bounds;
use super::edges::Line;
use super::rect;
use super::row::{Accumulate, DenseRow, Row, Sink, fold};
use super::shape::{Body, EdgeBody, GlyphBody, Shape};
use super::{Area, BandCoverage, ClipShape, MarkCoverage};
use crate::FillRule;
use pagura_core::{Meter, Result};

/// The band rasterizer: a dense band-width cell row with a touched-cell
/// bitmap, reused for every mark of a render.
#[derive(Debug)]
pub struct BandRasterizer {
    area: Area,
    row: Row,
    cells: approximate::Scratch,
    grid: Grid,
    work: usize,
}

/// The cells of a small mark's rows within a band, accumulated edge by edge
/// and scanned whole, and the bytes of one of its rows.
#[derive(Debug, Default)]
struct Grid {
    area: Vec<f64>,
    cover: Vec<f64>,
    bytes: Vec<u8>,
}

/// The cells of the largest mark the grid holds.
const GRID_CELLS: usize = 1 << 14;

impl Grid {
    fn new(bytes: &mut Meter) -> Result<Self> {
        let mut grid = Self::default();
        bytes.reserve(&mut grid.area, GRID_CELLS)?;
        bytes.reserve(&mut grid.cover, GRID_CELLS)?;
        bytes.reserve(&mut grid.bytes, GRID_CELLS)?;
        grid.area.resize(GRID_CELLS, 0.0);
        grid.cover.resize(GRID_CELLS, 0.0);
        grid.bytes.resize(GRID_CELLS, 0);
        Ok(grid)
    }

    fn retained_bytes(&self) -> usize {
        (self.area.capacity() + self.cover.capacity())
            .saturating_mul(size_of::<f64>())
            .saturating_add(self.bytes.capacity())
    }

    /// Row `k` of a grid `width` cells wide from device column `base`.
    #[inline]
    fn row(&mut self, k: usize, width: usize, base: i64) -> DenseRow<'_> {
        let cells = k * width..(k + 1) * width;
        DenseRow {
            base,
            area: &mut self.area[cells.clone()],
            cover: &mut self.cover[cells],
        }
    }
}

impl BandRasterizer {
    /// Scratch storage for `area`, charged to `bytes` (`render_bytes`):
    /// one row of cells up to the region's right edge and its bitmap, one
    /// row of approximate cells, and a fixed grid for small marks. The
    /// charge depends on the region's right edge only, never on the band
    /// height or the region's rows.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when the storage exceeds `bytes`.
    pub fn new(area: Area, bytes: &mut Meter) -> Result<Self> {
        let width = super::row::width(area.region.x1)?;
        Ok(Self {
            area,
            row: Row::new(width, bytes)?,
            cells: approximate::Scratch::new(width, bytes)?,
            grid: Grid::new(bytes)?,
            work: 0,
        })
    }

    /// The area.
    pub fn area(&self) -> Area {
        self.area
    }

    /// Work units spent by every [`Self::rasterize`] call so far, as
    /// [`Shape::work`] and [`MarkCoverage::work`] bound them.
    pub fn work_done(&self) -> usize {
        self.work
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.row
            .retained_bytes()
            .saturating_add(self.cells.retained_bytes())
            .saturating_add(self.grid.retained_bytes())
    }

    /// Writes the coverage of `mark` within `band` to `out`, replacing its
    /// contents.
    pub fn rasterize(&mut self, mark: MarkCoverage<'_>, band: u32, out: &mut BandCoverage) {
        out.clear();
        let rows = self.area.band_rows(band);
        let region = self.area.region;
        match (mark.shape, mark.clip) {
            (None, None) => {
                for y in rows {
                    out.push_row(y);
                    out.put_run(region.x0, region.width(), u8::MAX);
                    self.work = self.work.saturating_add(2);
                }
            }
            (None, Some(clip)) => self.clip(clip, rows, out),
            (Some(shape), clip) => {
                let prepared = shape.clip.as_ref().or(shape.multiplied.as_ref());
                debug_assert!(
                    match (prepared, clip) {
                        (Some(a), Some(b)) => a.same(b),
                        (Some(_), None) => false,
                        (None, _) => true,
                    },
                    "a shape is rasterized with the clip it was prepared against"
                );
                debug_assert_eq!(shape.area, self.area, "a shape of another area");
                // A shape prepared outside the mark's clip has the clip's
                // coverage multiplied in: exact wherever either is empty or
                // full, a product where both are partial. Preparing the
                // shape in its clip state keeps every pixel exact.
                let late = if prepared.is_none() { clip } else { None };
                let mut bounds = shape.bounds;
                if let Some(clip) = late {
                    bounds = bounds.intersect(&clip.bounds());
                }
                let rows = rows.start.max(bounds.y0)..rows.end.min(bounds.y1);
                if rows.is_empty() {
                    out.finish();
                    return;
                }
                let shape = (shape, late);
                match &shape.0.body {
                    Body::Rect(r) => self.rect(*r, shape, rows, out),
                    Body::Edges(edges) => self.edges(edges, shape, (band, rows), out),
                    Body::Glyph(glyph) => self.glyph(glyph, shape, rows, out),
                    Body::Cells(cells) => {
                        self.work = self.work.saturating_add(self.cells.rasterize(
                            cells,
                            shape,
                            (band, rows),
                            out,
                        ));
                    }
                    Body::Binned(placement) => {
                        self.work = self.work.saturating_add(binned::rasterize(
                            placement,
                            shape,
                            rows,
                            (out, self.cells.row_buffer()),
                        ));
                    }
                }
            }
        }
        out.finish();
    }

    fn rect(
        &mut self,
        r: Bounds,
        shape: (&Shape, Option<&ClipShape>),
        rows: std::ops::Range<u32>,
        out: &mut BandCoverage,
    ) {
        for y in rows {
            out.push_row(y);
            let mut sink = sink(out, shape, y, self.cells.row_buffer());
            let runs = rect::emit_row(r, y, &mut sink);
            self.work = self
                .work
                .saturating_add(runs + 2)
                .saturating_add(sink.extra());
        }
    }

    fn edges(
        &mut self,
        edges: &EdgeBody,
        shape: (&Shape, Option<&ClipShape>),
        (band, rows): (u32, std::ops::Range<u32>),
        out: &mut BandCoverage,
    ) {
        let _ = band;
        let count = edges.lines.len();
        for y in rows {
            let top = f64::from(y);
            let mut cells = 0;
            let items = edges.index.row(y);
            for &item in items {
                let item = item as usize;
                cells += match edges.lines.get(item) {
                    Some(&line) => self.row.line(line, top),
                    None => edges
                        .curves
                        .get(item - count)
                        .map_or(0, |&q| self.row.curve(q, top)),
                };
            }
            out.push_row(y);
            let mut sink = sink(out, shape, y, self.cells.row_buffer());
            let touched = self.row.emit(edges.scan, shape.0.rule, &mut sink);
            let words = (edges.scan.1.saturating_sub(edges.scan.0)) / 64 + 2;
            self.work = self
                .work
                .saturating_add(items.len())
                .saturating_add(cells)
                .saturating_add(touched.saturating_mul(3))
                .saturating_add(words + 2)
                .saturating_add(sink.extra());
        }
    }

    /// Accumulates a translated glyph: each listed edge plus the origin, as
    /// translating the vertices first would give.
    fn glyph(
        &mut self,
        glyph: &GlyphBody,
        shape: (&Shape, Option<&ClipShape>),
        rows: std::ops::Range<u32>,
        out: &mut BandCoverage,
    ) {
        let Some(relative) = glyph.edges.relative() else {
            return;
        };
        let (plain, late) = shape;
        let width = glyph.scan.1.saturating_sub(glyph.scan.0);
        let height = rows.len();
        if late.is_none()
            && plain.multiplied.is_none()
            && !plain.overlay
            && width > 0
            && width.saturating_mul(height) <= GRID_CELLS
        {
            self.glyph_grid((glyph, plain.bounds), relative, rows, out);
            return;
        }
        let o = glyph.origin;
        let at = |p: crate::Point| crate::Point::new(p.x + o.x, p.y + o.y);
        let count = relative.lines.len();
        let words = (glyph.scan.1.saturating_sub(glyph.scan.0)) / 64 + 2;
        // An item's first and last listed rows are reached only by some
        // fractional origins; others skip them, with a margin far above the
        // rounding of the translation.
        let f = o.y - o.y.floor();
        let (low, high) = (f - 1e-9, f + 1e-9);
        for y in rows {
            let top = f64::from(y);
            let local = u32::try_from(i64::from(y) - glyph.row0).ok();
            let items = local.map_or(&[][..], |r| relative.index.row(r));
            let mut cells = 0;
            for &item in items {
                let item = item as usize;
                let [first, last] = relative.ends[item];
                let [below, above] = relative.reach[item];
                if (local == Some(first) && low >= below) || (local == Some(last) && high <= above)
                {
                    continue;
                }
                cells += match relative.lines.get(item) {
                    Some(l) => self.row.line(
                        Line {
                            a: at(l.a),
                            b: at(l.b),
                        },
                        top,
                    ),
                    None => relative.curves.get(item - count).map_or(0, |q| {
                        self.row.curve(
                            Quadratic {
                                points: q.points.map(at),
                            },
                            top,
                        )
                    }),
                };
            }
            out.push_row(y);
            let mut sink = sink(out, shape, y, self.cells.row_buffer());
            let touched = self.row.emit(glyph.scan, FillRule::NonZero, &mut sink);
            self.work = self
                .work
                .saturating_add(items.len())
                .saturating_add(cells)
                .saturating_add(touched.saturating_mul(3))
                .saturating_add(words + 2)
                .saturating_add(sink.extra());
        }
    }

    /// A glyph's rows accumulated edge by edge into the grid, each edge
    /// visiting only the rows it crosses, then scanned whole: per cell the
    /// edges add in the same order and arithmetic as row by row.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded device rows are clamped to the band's u32 rows"
    )]
    fn glyph_grid(
        &mut self,
        (glyph, bounds): (&GlyphBody, super::DeviceRect),
        relative: &super::glyph::Relative,
        rows: std::ops::Range<u32>,
        out: &mut BandCoverage,
    ) {
        let o = glyph.origin;
        let at = |p: crate::Point| crate::Point::new(p.x + o.x, p.y + o.y);
        let (from, to) = glyph.scan;
        let width = to - from;
        let base = from as i64;
        let (first, end) = (f64::from(rows.start), f64::from(rows.end));
        let grid = &mut self.grid;
        let mut work = 0_usize;
        // The rows an edge from height `ya` to `yb` crosses in the band.
        let span = |ya: f64, yb: f64| {
            let (lo, hi) = (ya.min(yb).floor().max(first), ya.max(yb).ceil().min(end));
            (lo < hi).then(|| lo as u32..hi as u32)
        };
        for l in &relative.lines {
            let line = Line {
                a: at(l.a),
                b: at(l.b),
            };
            work += 1;
            for y in span(line.a.y, line.b.y).into_iter().flatten() {
                work += 1 + grid
                    .row((y - rows.start) as usize, width, base)
                    .line(line, f64::from(y));
            }
        }
        for q in &relative.curves {
            let q = Quadratic {
                points: q.points.map(at),
            };
            work += 1;
            for y in span(q.points[0].y, q.points[2].y).into_iter().flatten() {
                work += 1 + grid
                    .row((y - rows.start) as usize, width, base)
                    .curve(q, f64::from(y));
            }
        }
        for (k, y) in rows.enumerate() {
            let cells = k * width..(k + 1) * width;
            let mut cover = 0.0;
            for ((a, c), byte) in grid.area[cells.clone()]
                .iter_mut()
                .zip(&mut grid.cover[cells])
                .zip(&mut grid.bytes[..width])
            {
                cover += std::mem::take(c);
                *byte = fold(std::mem::take(a) + cover, FillRule::NonZero);
            }
            // The shape's own columns: the scan may reach past the area.
            let x0 = (bounds.x0 as usize).clamp(from, to);
            let x1 = (bounds.x1 as usize).clamp(x0, to);
            out.push_row(y);
            out.put_row(x0 as u32, &grid.bytes[x0 - from..x1 - from]);
            work += width + 2;
        }
        self.work = self.work.saturating_add(work);
    }

    fn clip(&mut self, clip: &ClipShape, rows: std::ops::Range<u32>, out: &mut BandCoverage) {
        let bounds = clip.bounds();
        let rows = rows.start.max(bounds.y0)..rows.end.min(bounds.y1);
        for y in rows {
            out.push_row(y);
            let mut sink = Out {
                coverage: out,
                x0: bounds.x0 as usize,
                x1: bounds.x1 as usize,
            };
            if let Some(product) = clip.product() {
                let runs = product.emit_row(
                    y,
                    clip.rect(),
                    (bounds.x0 as usize, bounds.x1 as usize),
                    &mut sink,
                );
                self.work = self.work.saturating_add(runs + 2);
                continue;
            }
            if let Some(rows) = clip.rows() {
                let segments = rows.row(y);
                for s in segments {
                    let value = if s.cell == INSIDE {
                        u8::MAX
                    } else {
                        rows.cell(s.cell).map_or(0, |(c, _, _)| c.value)
                    };
                    sink.run(s.x0 as usize, s.x1 as usize, value);
                }
                self.work = self.work.saturating_add(segments.len() + 2);
                continue;
            }
            let whole = Bounds {
                left: f64::from(bounds.x0),
                top: f64::from(bounds.y0),
                right: f64::from(bounds.x1),
                bottom: f64::from(bounds.y1),
            };
            let rect = clip.rect().map_or(whole, |r| r.intersect(whole));
            let runs = rect::emit_row(rect, y, &mut sink);
            self.work = self.work.saturating_add(runs + 2);
        }
    }
}

/// Writes runs to the current row of a coverage, within columns `x0..x1`.
pub(crate) struct Out<'a> {
    pub(crate) coverage: &'a mut BandCoverage,
    pub(crate) x0: usize,
    pub(crate) x1: usize,
}

impl Sink for Out<'_> {
    #[inline]
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        let a = x0.max(self.x0);
        let b = x1.min(self.x1);
        if a < b {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "columns lie within the u32 region"
            )]
            self.coverage.put_run(a as u32, (b - a) as u32, value);
        }
    }
}

/// The sink of one row of `shape`: its cells resolved at preparation and
/// its clip's rows, where it has them, over the output, or its multiplied
/// clip; a clip it was not prepared in (`late`) is multiplied in after its
/// resolved cells.
pub(crate) fn sink<'a>(
    out: &'a mut BandCoverage,
    (shape, late): (&'a Shape, Option<&'a ClipShape>),
    y: u32,
    buffer: &'a mut Vec<u8>,
) -> RowSink<'a> {
    if let Some(clip) = shape.multiplied.as_ref() {
        return RowSink::Multiplied(Multiplied::new(out, shape, Some(clip), y, buffer));
    }
    if let Some(clip) = late {
        return RowSink::Late(Overlay {
            out: Multiplied::new(out, shape, Some(clip), y, buffer),
            stored: shape.stored.row(y),
            next: 0,
            clip: None,
            segment: 0,
            extra: 0,
        });
    }
    let rows = shape
        .clip
        .as_ref()
        .filter(|_| shape.overlay)
        .and_then(ClipShape::rows);
    RowSink::Overlay(Overlay {
        out: Out {
            coverage: out,
            x0: shape.bounds.x0 as usize,
            x1: shape.bounds.x1 as usize,
        },
        stored: shape.stored.row(y),
        next: 0,
        clip: rows.map(|rows| (rows, rows.row(y))),
        segment: 0,
        extra: 0,
    })
}

/// A shape row's sink.
pub(crate) enum RowSink<'a> {
    Overlay(Overlay<'a>),
    Multiplied(Multiplied<'a>),
    Late(Overlay<'a, Multiplied<'a>>),
}

impl RowSink<'_> {
    pub(crate) fn extra(&self) -> usize {
        match self {
            Self::Overlay(o) => o.extra(),
            Self::Multiplied(m) => m.work(),
            Self::Late(o) => o.extra().saturating_add(o.out.work()),
        }
    }
}

impl Sink for RowSink<'_> {
    #[inline]
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        match self {
            Self::Overlay(o) => o.run(x0, x1, value),
            Self::Multiplied(m) => m.run(x0, x1, value),
            Self::Late(o) => o.run(x0, x1, value),
        }
    }
}

/// Runs of a shape's row with its resolved cells substituted and its clip's
/// rows applied: inside runs keep the shape's value, a boundary cell takes
/// the clip's value where the shape covers it fully, outside is dropped.
pub(crate) struct Overlay<'a, S = Out<'a>> {
    out: S,
    stored: &'a [(u32, u8)],
    next: usize,
    clip: Option<(&'a Rows, &'a [Segment])>,
    segment: usize,
    extra: usize,
}

impl<S> Overlay<'_, S> {
    /// Work units spent on resolved cells and clip runs.
    pub(crate) fn extra(&self) -> usize {
        self.extra
    }
}

impl<S: Sink> Sink for Overlay<'_, S> {
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        if self.stored.is_empty() && self.clip.is_none() {
            self.out.run(x0, x1, value);
            return;
        }
        let mut x = x0;
        while x < x1 {
            while self
                .stored
                .get(self.next)
                .is_some_and(|&(s, _)| (s as usize) < x)
            {
                self.next += 1;
                self.extra += 1;
            }
            let stored = self.stored.get(self.next).copied();
            if let Some((s, v)) = stored
                && s as usize == x
            {
                self.out.run(x, x + 1, v);
                self.next += 1;
                self.extra += 1;
                x += 1;
                continue;
            }
            let limit = stored.map_or(x1, |(s, _)| (s as usize).min(x1));
            let Some((rows, segments)) = self.clip else {
                self.out.run(x, limit, value);
                x = limit;
                continue;
            };
            while segments
                .get(self.segment)
                .is_some_and(|s| s.x1 as usize <= x)
            {
                self.segment += 1;
                self.extra += 1;
            }
            match segments.get(self.segment) {
                None => x = limit,
                Some(s) if s.x0 as usize > x => x = limit.min(s.x0 as usize),
                Some(s) if s.cell == INSIDE => {
                    let end = limit.min(s.x1 as usize);
                    self.out.run(x, end, value);
                    x = end;
                }
                Some(s) => {
                    let clip = rows.cell(s.cell).map_or(0, |(c, _, _)| c.value);
                    let v = if value == u8::MAX { clip } else { 0 };
                    self.out.run(x, x + 1, v);
                    x += 1;
                }
            }
        }
    }
}
