//! The band rasterizer: one strip of cells, reused for every mark and band.

use super::cells::{Cells, STRIP};
use super::clip::{INSIDE, Rows, Segment};
use super::coverage::RowOut;
use super::curve::Quadratic;
use super::edges::{Bounds, Line, StripList};
use super::rect;
use super::row::Sink;
use super::shape::{Body, Shape};
use super::{Area, BandCoverage, ClipShape, MarkCoverage};
use crate::{FillRule, Point};
use pagura_core::{Meter, Result};

/// The band rasterizer: strip-sized cells with a touched bitmap per row,
/// reused for every mark of a render.
#[derive(Debug)]
pub struct BandRasterizer {
    area: Area,
    cells: Cells,
    run: Run,
    work: usize,
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
}

impl BandRasterizer {
    /// Scratch storage for `area`, charged to `bytes` (`render_bytes`):
    /// one strip of cells up to the region's right edge and its bitmaps,
    /// and the storage of glyph runs. The charge depends on the region's
    /// right edge only, never on the band height or the region's rows.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when the storage exceeds `bytes`.
    pub fn new(area: Area, bytes: &mut Meter) -> Result<Self> {
        let width = super::row::width(area.region.x1)?;
        Ok(Self {
            area,
            cells: Cells::new(width, bytes)?,
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
        Cells::charge(width, bytes)?;
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
        self.cells
            .retained_bytes()
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
                debug_assert!(
                    match (&shape.clip, clip) {
                        (Some(a), Some(b)) => a.same(b),
                        (prepared, clip) => prepared.is_none() && clip.is_none(),
                    },
                    "a shape is rasterized with the clip it was prepared against"
                );
                let rows = Self::shape_rows(shape, rows);
                self.shape(shape, (band, rows), out);
            }
        }
        out.finish();
    }

    /// The rows of `band` (`rows`) that `shape` can cover.
    fn shape_rows(shape: &Shape, rows: std::ops::Range<u32>) -> std::ops::Range<u32> {
        let bounds = shape.bounds;
        rows.start.max(bounds.y0)..rows.end.min(bounds.y1).max(rows.start.max(bounds.y0))
    }

    /// Writes `shape`'s `rows` of `band`, within its bounds, to `out`.
    fn shape(
        &mut self,
        shape: &Shape,
        (band, rows): (u32, std::ops::Range<u32>),
        out: &mut dyn RowOut,
    ) {
        debug_assert_eq!(shape.area, self.area, "a shape of another area");
        if rows.is_empty() {
            return;
        }
        let _ = band;
        match &shape.body {
            Body::Edges(edges) => {
                let items = Items {
                    lines: &edges.lines,
                    curves: &edges.curves,
                    origin: None,
                    strips: edges.strips.as_ref(),
                };
                self.items(&items, (shape, edges.scan, shape.rule), rows, out);
            }
            Body::Glyph(glyph) => {
                if let Some(relative) = glyph.edges.relative() {
                    let items = Items {
                        lines: &relative.lines,
                        curves: &relative.curves,
                        origin: Some(glyph.origin),
                        strips: None,
                    };
                    self.items(&items, (shape, glyph.scan, FillRule::NonZero), rows, out);
                }
            }
        }
    }

    /// Writes the coverage within `band` of glyph placements of one run, all
    /// prepared in the clip state `clip`, to `out`, replacing its contents:
    /// each glyph's rows as [`Self::rasterize`] writes them alone, merged
    /// into the band's rows. The glyphs must share no pixel; they come in
    /// content order. Rasterizes the leading glyphs that fit the run
    /// storage, at least one, and returns how many it took; the caller
    /// passes the rest again.
    pub fn rasterize_run(
        &mut self,
        glyphs: &[&Shape],
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
        for &glyph in glyphs {
            let own = Self::shape_rows(glyph, rows.clone());
            let bounds = glyph.bounds;
            let width = bounds.width() as usize;
            let cells = own.len().saturating_mul(width);
            if run.slots.len() == RUN_SHAPES || used.saturating_add(cells) > RUN_CELLS {
                if taken == 0 {
                    // Too large to share the run storage: alone.
                    self.run = run;
                    let shape = Some(glyph);
                    self.rasterize(MarkCoverage { shape, clip }, band, out);
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
            self.shape(glyphs[slot.shape], (band, slot.rows.clone()), &mut target);
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

    /// Writes `items`' `rows` strip by strip: each strip's items accumulate
    /// into the cells, then its rows leave through `shape`'s sink.
    fn items(
        &mut self,
        items: &Items<'_>,
        (shape, scan, rule): (&Shape, (usize, usize), FillRule),
        rows: std::ops::Range<u32>,
        out: &mut dyn RowOut,
    ) {
        let width = scan.1.saturating_sub(scan.0);
        let words = width / 64 + 2;
        let fit = self.cells.rows_fit(width);
        let mut y = rows.start;
        while y < rows.end {
            let strip = y / STRIP;
            let end = strip.saturating_add(1).saturating_mul(STRIP).min(rows.end);
            let window = y..end.min(y.saturating_add(fit));
            y = window.end;
            if !self.cells.open(scan, window.clone()) {
                continue;
            }
            let mut work = 0;
            if let Some(list) = items.strips {
                let listed = list.strip(strip);
                work += listed.len();
                for &item in listed {
                    work += items.add(&mut self.cells, item as usize);
                }
            } else {
                let count = items.lines.len() + items.curves.len();
                work += count;
                for item in 0..count {
                    work += items.add(&mut self.cells, item);
                }
            }
            for (r, row) in (0_usize..).zip(self.cells.rows()) {
                out.push_row(row);
                let mut sink = sink(out, shape, row);
                let touched = self.cells.emit(r, rule, &mut sink);
                work = work
                    .saturating_add(touched.saturating_mul(3))
                    .saturating_add(words + 2)
                    .saturating_add(sink.extra());
            }
            self.work = self.work.saturating_add(work);
        }
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

/// A mark's items: its lines, then its monotone quadratics, moved by a
/// glyph's origin, and listed per strip where the mark has lists.
struct Items<'a> {
    lines: &'a [Line],
    curves: &'a [Quadratic],
    origin: Option<Point>,
    strips: Option<&'a StripList>,
}

impl Items<'_> {
    /// Adds item `item` to the open window: each point plus the origin, as
    /// translating the vertices first would give. Returns the work spent.
    #[inline]
    fn add(&self, cells: &mut Cells, item: usize) -> usize {
        let at = |p: Point| match self.origin {
            Some(o) => Point::new(p.x + o.x, p.y + o.y),
            None => p,
        };
        // Items right of the window change none of its cells.
        match self.lines.get(item) {
            Some(l) => {
                let line = Line {
                    a: at(l.a),
                    b: at(l.b),
                };
                if cells.reaches(line.a.x.min(line.b.x)) {
                    cells.line(line)
                } else {
                    0
                }
            }
            None => self
                .curves
                .get(item.wrapping_sub(self.lines.len()))
                .map_or(0, |q| {
                    let curve = Quadratic {
                        points: q.points.map(at),
                    };
                    if cells.reaches(curve.points[0].x.min(curve.points[2].x)) {
                        cells.curve(curve)
                    } else {
                        0
                    }
                }),
        }
    }
}

/// Writes runs to the current row of a coverage, within columns `x0..x1`.
struct Out<'a> {
    coverage: &'a mut dyn RowOut,
    x0: usize,
    x1: usize,
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
/// its clip's rows, where it has them, over the output.
fn sink<'a>(out: &'a mut dyn RowOut, shape: &'a Shape, y: u32) -> RowSink<'a> {
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
enum RowSink<'a> {
    /// Nothing between the shape and the output.
    Plain(Out<'a>),
    Overlay(Overlay<'a>),
}

impl RowSink<'_> {
    fn extra(&self) -> usize {
        match self {
            Self::Plain(_) => 0,
            Self::Overlay(o) => o.extra(),
        }
    }
}

impl Sink for RowSink<'_> {
    #[inline]
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        match self {
            Self::Plain(o) => o.run(x0, x1, value),
            Self::Overlay(o) => o.run(x0, x1, value),
        }
    }
}

/// Runs of a shape's row with its resolved cells substituted and its clip's
/// rows applied: inside runs keep the shape's value, a boundary cell takes
/// the clip's value where the shape covers it fully, outside is dropped.
struct Overlay<'a> {
    out: Out<'a>,
    stored: &'a [(u32, u8)],
    next: usize,
    clip: Option<(&'a Rows, &'a [Segment])>,
    segment: usize,
    extra: usize,
}

impl Overlay<'_> {
    /// Work units spent on resolved cells and clip runs.
    fn extra(&self) -> usize {
        self.extra
    }
}

impl Sink for Overlay<'_> {
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
