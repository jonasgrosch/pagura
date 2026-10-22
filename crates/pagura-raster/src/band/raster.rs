//! The band rasterizer: one dense cell row, reused for every mark and band.

use super::approximate::{self, Multiplied};
use super::binned;
use super::clip::{INSIDE, Rows, Segment};
use super::coverage::RowOut;
use super::curve::Quadratic;
use super::edges::Bounds;
use super::edges::Line;
use super::rect;
use super::row::{Accumulate, DenseRow, Row, Sink, fold};
use super::shape::{Body, EdgeBody, GlyphBody, Shape};
use super::{Area, BandCoverage, ClipShape, GlyphMask, MarkCoverage};
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
    run: Run,
    work: usize,
}

/// One glyph of a run: a shape, or a mask that stores as it is.
#[derive(Debug, Clone, Copy)]
pub enum RunGlyph<'a> {
    /// A prepared shape.
    Shape(&'a Shape),
    /// A binned mask its clip state keeps whole.
    Mask(GlyphMask<'a>),
}

impl RunGlyph<'_> {
    fn bounds(&self) -> super::DeviceRect {
        match self {
            Self::Shape(shape) => shape.bounds,
            Self::Mask(mask) => mask.bounds(),
        }
    }
}

/// The shapes of a run at once, at most.
const RUN_SHAPES: usize = 256;

/// The cells a run's shapes share, at most.
const RUN_CELLS: usize = 1 << 16;

/// Storage for one run: each shape's rows in its own cells, then merged
/// into a band-width row.
#[derive(Debug, Default)]
struct Run {
    slots: Vec<Slot>,
    cells: Vec<u8>,
    row: Vec<u8>,
}

impl Run {
    fn new(width: usize, bytes: &mut Meter) -> Result<Self> {
        let mut run = Self::default();
        bytes.reserve(&mut run.slots, RUN_SHAPES)?;
        bytes.reserve(&mut run.cells, RUN_CELLS)?;
        bytes.reserve(&mut run.row, width)?;
        run.cells.resize(RUN_CELLS, 0);
        run.row.resize(width, 0);
        Ok(run)
    }

    fn retained_bytes(&self) -> usize {
        (self.slots.capacity() * size_of::<Slot>())
            .saturating_add(self.cells.capacity())
            .saturating_add(self.row.capacity())
    }
}

/// One shape of a run and its cells.
#[derive(Debug)]
struct Slot {
    shape: usize,
    start: usize,
    x0: u32,
    width: usize,
    rows: std::ops::Range<u32>,
}

/// A run shape's own cells, its bounds' columns by its rows.
struct SlotOut<'a> {
    cells: &'a mut [u8],
    x0: u32,
    width: usize,
    y0: u32,
    row: usize,
}

impl SlotOut<'_> {
    /// The cells of columns `x .. x + len` in the current row, clamped.
    fn span(&mut self, x: u32, len: usize) -> (&mut [u8], usize) {
        let from = (x.saturating_sub(self.x0) as usize).min(self.width);
        let skip = (self.x0.saturating_sub(x) as usize).min(len);
        let to = (from + len - skip).min(self.width);
        let start = self.row * self.width;
        (&mut self.cells[start + from..start + to], skip)
    }
}

impl RowOut for SlotOut<'_> {
    fn push_row(&mut self, y: u32) {
        self.row = y.saturating_sub(self.y0) as usize;
    }

    fn put_run(&mut self, x: u32, len: u32, value: u8) {
        self.span(x, len as usize).0.fill(value);
    }

    fn put_row(&mut self, x: u32, values: &[u8]) {
        let (cells, skip) = self.span(x, values.len());
        let n = cells.len();
        cells.copy_from_slice(&values[skip..skip + n]);
    }
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
pub(crate) const GRID_CELLS: usize = 1 << 14;

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
            run: Run::new(width, bytes)?,
            work: 0,
        })
    }

    /// Charges `bytes` with the storage [`Self::new`] reserves for `area`,
    /// without allocating it: preparation reserves a rasterizer that a
    /// render makes or reuses.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn charge(area: Area, bytes: &mut Meter) -> Result<()> {
        let width = super::row::width(area.region.x1)?;
        let words = width.div_ceil(64) + 1;
        // The row: area, cover and its bitmap.
        bytes.charge_items::<f64>(width)?;
        bytes.charge_items::<f64>(width)?;
        bytes.charge_items::<u64>(words)?;
        // The approximate row, its bitmap and product, and its grid.
        bytes.charge_items::<f32>(width)?;
        bytes.charge_items::<u64>(words)?;
        bytes.charge_items::<u8>(width)?;
        bytes.charge_items::<f32>(GRID_CELLS)?;
        bytes.charge_items::<u8>(GRID_CELLS)?;
        // The grid.
        bytes.charge_items::<f64>(GRID_CELLS)?;
        bytes.charge_items::<f64>(GRID_CELLS)?;
        bytes.charge_items::<u8>(GRID_CELLS)?;
        // The run.
        bytes.charge_items::<Slot>(RUN_SHAPES)?;
        bytes.charge_items::<u8>(RUN_CELLS)?;
        bytes.charge_items::<u8>(width)
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
            .saturating_add(self.run.retained_bytes())
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
                let rows = Self::shape_rows(shape, clip, rows);
                self.shape(shape, clip, (band, rows), out);
            }
        }
        out.finish();
    }

    /// The rows of `band` (`rows`) that `shape` within `clip` can cover.
    fn shape_rows(
        shape: &Shape,
        clip: Option<&ClipShape>,
        rows: std::ops::Range<u32>,
    ) -> std::ops::Range<u32> {
        let mut bounds = shape.bounds;
        if let Some(clip) = Self::late(shape, clip) {
            bounds = bounds.intersect(&clip.bounds());
        }
        rows.start.max(bounds.y0)..rows.end.min(bounds.y1).max(rows.start.max(bounds.y0))
    }

    /// A clip `shape` was not prepared in: its coverage is multiplied in,
    /// exact wherever either is empty or full, a product where both are
    /// partial. Preparing the shape in its clip state keeps every pixel
    /// exact.
    fn late<'c>(shape: &Shape, clip: Option<&'c ClipShape>) -> Option<&'c ClipShape> {
        let prepared = shape.clip.as_ref().or(shape.multiplied.as_ref());
        debug_assert!(
            match (prepared, clip) {
                (Some(a), Some(b)) => a.same(b),
                (Some(_), None) => false,
                (None, _) => true,
            },
            "a shape is rasterized with the clip it was prepared against"
        );
        if prepared.is_none() { clip } else { None }
    }

    /// Writes `shape`'s `rows` of `band`, within its bounds, to `out`.
    fn shape(
        &mut self,
        shape: &Shape,
        clip: Option<&ClipShape>,
        (band, rows): (u32, std::ops::Range<u32>),
        out: &mut dyn RowOut,
    ) {
        debug_assert_eq!(shape.area, self.area, "a shape of another area");
        if rows.is_empty() {
            return;
        }
        let shape = (shape, Self::late(shape, clip));
        match &shape.0.body {
            Body::Rect(r) => self.rect(*r, shape, rows, out),
            Body::Edges(edges) => self.edges(edges, shape, (band, rows), out),
            Body::Glyph(glyph) => self.glyph(glyph, shape, rows, out),
            Body::Cells(cells) => {
                self.work =
                    self.work
                        .saturating_add(self.cells.rasterize(cells, shape, (band, rows), out));
            }
            Body::Binned(mask) => {
                self.work = self.work.saturating_add(binned::rasterize(
                    mask,
                    shape,
                    rows,
                    (out, self.cells.row_buffer()),
                ));
            }
        }
    }

    /// Writes the coverage of `mask` within `band` to `out`, replacing its
    /// contents: its rows as they are. The clip state it was prepared in
    /// keeps all of it.
    pub fn rasterize_mask(&mut self, mask: GlyphMask<'_>, band: u32, out: &mut BandCoverage) {
        out.clear();
        let rows = Self::mask_rows(mask, self.area.band_rows(band));
        self.work = self.work.saturating_add(binned::emit(mask, rows, out));
        out.finish();
    }

    /// The rows of `band` (`rows`) that `mask` can cover.
    fn mask_rows(mask: GlyphMask<'_>, rows: std::ops::Range<u32>) -> std::ops::Range<u32> {
        let b = mask.bounds();
        let start = rows.start.max(b.y0);
        start..rows.end.min(b.y1).max(start)
    }

    /// Writes the coverage within `band` of glyphs of one run, all in the
    /// clip state `clip`, to `out`, replacing its contents: each glyph's
    /// rows as [`Self::rasterize`] and [`Self::rasterize_mask`] write them
    /// alone, merged into the band's rows. The glyphs must share no pixel;
    /// they come in content order. Rasterizes the leading glyphs that fit
    /// the run storage, at least one, and returns how many it took; the
    /// caller passes the rest again.
    pub fn rasterize_run(
        &mut self,
        glyphs: &[RunGlyph<'_>],
        clip: Option<&ClipShape>,
        band: u32,
        out: &mut BandCoverage,
    ) -> usize {
        out.clear();
        let rows = self.area.band_rows(band);
        let mut run = std::mem::take(&mut self.run);
        run.slots.clear();
        let mut used = 0_usize;
        let mut taken = 0;
        for glyph in glyphs {
            let own = match glyph {
                RunGlyph::Shape(shape) => Self::shape_rows(shape, clip, rows.clone()),
                RunGlyph::Mask(mask) => Self::mask_rows(*mask, rows.clone()),
            };
            let bounds = glyph.bounds();
            let width = bounds.width() as usize;
            let cells = own.len().saturating_mul(width);
            if run.slots.len() == RUN_SHAPES || used.saturating_add(cells) > RUN_CELLS {
                if taken == 0 {
                    // Too large to share the run storage: alone.
                    self.run = run;
                    match *glyph {
                        RunGlyph::Shape(shape) => self.rasterize(
                            MarkCoverage {
                                shape: Some(shape),
                                clip,
                            },
                            band,
                            out,
                        ),
                        RunGlyph::Mask(mask) => self.rasterize_mask(mask, band, out),
                    }
                    return 1;
                }
                break;
            }
            taken += 1;
            if cells > 0 {
                run.slots.push(Slot {
                    shape: taken - 1,
                    start: used,
                    x0: bounds.x0,
                    width,
                    rows: own,
                });
                used += cells;
            }
        }
        for slot in &run.slots {
            let cells = &mut run.cells[slot.start..slot.start + slot.rows.len() * slot.width];
            cells.fill(0);
            let mut target = SlotOut {
                cells,
                x0: slot.x0,
                width: slot.width,
                y0: slot.rows.start,
                row: 0,
            };
            match glyphs[slot.shape] {
                RunGlyph::Shape(shape) => {
                    self.shape(shape, clip, (band, slot.rows.clone()), &mut target);
                }
                RunGlyph::Mask(mask) => {
                    let work = binned::emit(mask, slot.rows.clone(), &mut target);
                    self.work = self.work.saturating_add(work);
                }
            }
        }
        // Rows merged left to right; shapes whose columns meet go out as one
        // stretch, each pixel from the one shape that covers it.
        run.slots.sort_unstable_by_key(|s| s.x0);
        let mut work = run.slots.len();
        for y in rows {
            out.push_row(y);
            let mut stretch: Option<(u32, u32)> = None;
            for slot in run.slots.iter().filter(|s| s.rows.contains(&y)) {
                let x1 = slot.x0 + u32::try_from(slot.width).unwrap_or(u32::MAX);
                if let Some((a, b)) = stretch
                    && slot.x0 >= b
                {
                    out.put_row(a, &run.row[a as usize..b as usize]);
                    run.row[a as usize..b as usize].fill(0);
                    stretch = None;
                }
                let k = (y - slot.rows.start) as usize * slot.width + slot.start;
                let own = &run.cells[k..k + slot.width];
                for (merged, &value) in run.row[slot.x0 as usize..x1 as usize].iter_mut().zip(own) {
                    *merged = (*merged).max(value);
                }
                work += 2 * slot.width + 2;
                stretch = Some(stretch.map_or((slot.x0, x1), |(a, b)| (a, b.max(x1))));
            }
            if let Some((a, b)) = stretch {
                out.put_row(a, &run.row[a as usize..b as usize]);
                run.row[a as usize..b as usize].fill(0);
            }
        }
        self.work = self.work.saturating_add(work);
        self.run = run;
        out.finish();
        taken
    }

    fn rect(
        &mut self,
        r: Bounds,
        shape: (&Shape, Option<&ClipShape>),
        rows: std::ops::Range<u32>,
        out: &mut dyn RowOut,
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
        out: &mut dyn RowOut,
    ) {
        let _ = band;
        let Some(index) = &edges.index else {
            let at = |p: crate::Point| p;
            self.grid(
                (&edges.lines, &edges.curves, &at),
                (edges.scan, shape.0.rule),
                shape,
                rows,
                out,
            );
            return;
        };
        let count = edges.lines.len();
        for y in rows {
            let top = f64::from(y);
            let mut cells = 0;
            let items = index.row(y);
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
        out: &mut dyn RowOut,
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
            let o = glyph.origin;
            let at = |p: crate::Point| crate::Point::new(p.x + o.x, p.y + o.y);
            self.grid(
                (&relative.lines, &relative.curves, &at),
                (glyph.scan, FillRule::NonZero),
                shape,
                rows,
                out,
            );
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

    /// A small mark's rows accumulated edge by edge into the grid, each
    /// edge (moved by `at`) visiting only the rows it crosses, then scanned
    /// whole: per cell the edges add in the same order and arithmetic as
    /// row by row. Its rows go out at once, or through the shape's sink
    /// where a clip is multiplied in.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded device rows are clamped to the band's u32 rows"
    )]
    fn grid(
        &mut self,
        (lines, curves, at): (&[Line], &[Quadratic], &dyn Fn(crate::Point) -> crate::Point),
        (scan, rule): ((usize, usize), FillRule),
        shape: (&Shape, Option<&ClipShape>),
        rows: std::ops::Range<u32>,
        out: &mut dyn RowOut,
    ) {
        let (from, to) = scan;
        let width = to - from;
        let base = i64::try_from(from).unwrap_or(i64::MAX);
        let (first, end) = (f64::from(rows.start), f64::from(rows.end));
        // Edges right of the area change no cell of it.
        let right = f64::from(self.area.region.x1);
        let grid = &mut self.grid;
        let mut work = 0_usize;
        // The rows an edge from height `ya` to `yb` crosses in the band.
        let span = |ya: f64, yb: f64| {
            let (lo, hi) = (ya.min(yb).floor().max(first), ya.max(yb).ceil().min(end));
            // Within the band's u32 rows.
            #[expect(clippy::cast_sign_loss, reason = "clamped to the band's rows")]
            (lo < hi).then_some(lo as u32..hi as u32)
        };
        for l in lines {
            let line = Line {
                a: at(l.a),
                b: at(l.b),
            };
            work += 1;
            if line.a.x.min(line.b.x) >= right {
                continue;
            }
            for y in span(line.a.y, line.b.y).into_iter().flatten() {
                work += 1 + grid
                    .row((y - rows.start) as usize, width, base)
                    .line(line, f64::from(y));
            }
        }
        for q in curves {
            let q = Quadratic {
                points: q.points.map(at),
            };
            work += 1;
            if q.points[0].x.min(q.points[2].x) >= right {
                continue;
            }
            for y in span(q.points[0].y, q.points[2].y).into_iter().flatten() {
                work += 1 + grid
                    .row((y - rows.start) as usize, width, base)
                    .curve(q, f64::from(y));
            }
        }
        let plain = shape.0.multiplied.is_none() && shape.1.is_none();
        let bounds = shape.0.bounds;
        for (k, y) in rows.enumerate() {
            let cells = k * width..(k + 1) * width;
            let mut cover = 0.0;
            for ((a, c), byte) in grid.area[cells.clone()]
                .iter_mut()
                .zip(&mut grid.cover[cells])
                .zip(&mut grid.bytes[..width])
            {
                cover += std::mem::take(c);
                *byte = fold(std::mem::take(a) + cover, rule);
            }
            out.push_row(y);
            work += width + 2;
            if plain {
                // The shape's own columns: the scan may reach past the area.
                let x0 = (bounds.x0 as usize).clamp(from, to);
                let x1 = (bounds.x1 as usize).clamp(x0, to);
                out.put_row(x0 as u32, &grid.bytes[x0 - from..x1 - from]);
                continue;
            }
            let mut sink = sink(out, shape, y, self.cells.row_buffer());
            for (x, &value) in (from..).zip(&grid.bytes[..width]) {
                sink.run(x, x + 1, value);
            }
            work += sink.extra();
        }
        self.work = self.work.saturating_add(work);
    }

    fn clip(&mut self, clip: &ClipShape, rows: std::ops::Range<u32>, out: &mut dyn RowOut) {
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
    pub(crate) coverage: &'a mut dyn RowOut,
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
    out: &'a mut dyn RowOut,
    (shape, late): (&'a Shape, Option<&'a ClipShape>),
    y: u32,
    buffer: &'a mut Vec<u8>,
) -> RowSink<'a> {
    // A multiplied clip scales the shape's own coverage, its resolved
    // cells included.
    if let Some(clip) = shape.multiplied.as_ref().or(late) {
        let stored = shape.stored.row(y);
        let multiplied = Multiplied::new(out, shape, Some(clip), y, buffer);
        if stored.is_empty() {
            return RowSink::Multiplied(multiplied);
        }
        return RowSink::Late(Overlay {
            out: multiplied,
            stored,
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
    let stored = shape.stored.row(y);
    let out = Out {
        coverage: out,
        x0: shape.bounds.x0 as usize,
        x1: shape.bounds.x1 as usize,
    };
    if stored.is_empty() && rows.is_none() {
        return RowSink::Plain(out);
    }
    RowSink::Overlay(Overlay {
        out,
        stored,
        next: 0,
        clip: rows.map(|rows| (rows, rows.row(y))),
        segment: 0,
        extra: 0,
    })
}

/// A shape row's sink.
pub(crate) enum RowSink<'a> {
    /// Nothing between the shape and the output.
    Plain(Out<'a>),
    Overlay(Overlay<'a>),
    Multiplied(Multiplied<'a>),
    Late(Overlay<'a, Multiplied<'a>>),
}

impl RowSink<'_> {
    pub(crate) fn extra(&self) -> usize {
        match self {
            Self::Plain(_) => 0,
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
            Self::Plain(o) => o.run(x0, x1, value),
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
