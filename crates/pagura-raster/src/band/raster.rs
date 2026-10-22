//! The band rasterizer: one strip of cells, reused for every mark and band.

use super::cells::{Cells, STRIP};
use super::clip::{Rows, Segment};
use super::coverage::RowOut;
use super::curve::Mono;
use super::edges::{Line, StripList};
use super::row::Sink;
use super::shape::{Body, Resolved, Shape};
use super::{Area, BandCoverage, ClipShape, DeviceRect, MarkCoverage};
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

    fn put_row(&mut self, x: u32, values: &[u8]) {
        let (cells, skip) = self.span(x, values.len());
        let n = cells.len();
        if let Some(values) = values.get(skip..skip + n) {
            cells.copy_from_slice(values);
        }
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
                    resolved: &shape.resolved,
                };
                self.items(
                    &items,
                    (Target::of(shape), edges.scan, shape.rule),
                    rows,
                    out,
                );
            }
            Body::Glyph(glyph) => {
                if let Some(relative) = glyph.edges.relative() {
                    let items = Items {
                        lines: &relative.lines,
                        curves: &relative.curves,
                        origin: Some(glyph.origin),
                        strips: None,
                        resolved: &shape.resolved,
                    };
                    let target = Target::of(shape);
                    self.items(&items, (target, glyph.scan, FillRule::NonZero), rows, out);
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
    /// into the cells, then its rows leave for `target`.
    fn items(
        &mut self,
        items: &Items<'_>,
        (target, scan, rule): (Target<'_>, (usize, usize), FillRule),
        rows: std::ops::Range<u32>,
        out: &mut dyn RowOut,
    ) {
        let width = scan.1.saturating_sub(scan.0);
        let words = width / 64 + 2;
        let fit = self.cells.rows_fit(width);
        let mut y = rows.start;
        while y < rows.end {
            // Listed items come strip by strip; the others as many rows of
            // the band at once as fit.
            let strip = y / STRIP;
            let end = match items.strips {
                Some(_) => strip.saturating_add(1).saturating_mul(STRIP).min(rows.end),
                None => rows.end,
            };
            let window = y..end.min(y.saturating_add(fit));
            y = window.end;
            if !self.cells.open(scan, window.clone()) {
                continue;
            }
            let mut work = 0;
            // Resolved rows take their sides instead of the edges.
            let resolved = !items.resolved.is_empty();
            if resolved {
                self.cells.skip(items.resolved.mask(&window));
            }
            if let Some(list) = items.strips {
                let listed = list.strip(strip);
                work += listed.len();
                for &item in listed {
                    work += items.add(&mut self.cells, item as usize);
                }
            } else {
                work += items.all(&mut self.cells);
            }
            if resolved {
                self.cells.skip(0);
                let sides = items.resolved.lines(&window);
                work += sides.len();
                for &line in sides {
                    work += self.cells.line(line);
                }
            }
            for (r, row) in (0_usize..).zip(self.cells.rows()) {
                out.push_row(row);
                let mut sink = target.sink(out, row);
                let touched = self.cells.emit(r, rule, &mut sink);
                work = work
                    .saturating_add(touched.saturating_mul(3))
                    .saturating_add(words + 2)
                    .saturating_add(sink.extra());
            }
            self.work = self.work.saturating_add(work);
        }
    }

    /// Writes a clip's own coverage: its paths' classified rows, or its
    /// rectangle's sides through the cells.
    fn clip(&mut self, clip: &ClipShape, rows: std::ops::Range<u32>, out: &mut dyn RowOut) {
        let bounds = clip.bounds();
        let rows = rows.start.max(bounds.y0)..rows.end.min(bounds.y1).max(rows.start);
        if let Some(classified) = clip.rows() {
            for y in rows {
                out.push_row(y);
                let mut sink = Out {
                    coverage: out,
                    x0: bounds.x0 as usize,
                    x1: bounds.x1 as usize,
                };
                let segments = classified.row(y);
                for s in segments {
                    sink.run(s.x0 as usize, s.x1 as usize, s.value);
                }
                self.work = self.work.saturating_add(segments.len() + 2);
            }
            return;
        }
        let whole = [
            f64::from(bounds.x0),
            f64::from(bounds.y0),
            f64::from(bounds.x1),
            f64::from(bounds.y1),
        ];
        let [left, top, right, bottom] = clip
            .rect()
            .map_or(whole, |r| [r.left, r.top, r.right, r.bottom]);
        let sides = [
            Line {
                a: Point::new(left, top),
                b: Point::new(left, bottom),
            },
            Line {
                a: Point::new(right, bottom),
                b: Point::new(right, top),
            },
        ];
        let scan = super::clip::scan_columns(&sides, bounds.x1 as usize);
        let none = Resolved::default();
        let items = Items {
            lines: &sides,
            curves: &[],
            origin: None,
            strips: None,
            resolved: &none,
        };
        let target = Target { bounds, mask: None };
        self.items(&items, (target, scan, FillRule::NonZero), rows, out);
    }
}

/// A mark's items: its lines, then its monotone quadratics, moved by a
/// glyph's origin, and listed per strip where the mark has lists.
struct Items<'a> {
    lines: &'a [Line],
    curves: &'a [Mono],
    origin: Option<Point>,
    strips: Option<&'a StripList>,
    /// Rows whose sides replace the items.
    resolved: &'a Resolved,
}

impl Items<'_> {
    /// Adds every item to the open window, lines then curves. Returns the
    /// work spent, one unit per item visited.
    fn all(&self, cells: &mut Cells) -> usize {
        let origin = self.origin.unwrap_or_default();
        let mut work = self.lines.len() + self.curves.len();
        for l in self.lines {
            let line = Line {
                a: Point::new(l.a.x + origin.x, l.a.y + origin.y),
                b: Point::new(l.b.x + origin.x, l.b.y + origin.y),
            };
            if cells.reaches(line.a.x.min(line.b.x)) {
                work += cells.line(line);
            }
        }
        for m in self.curves {
            if cells.reaches(m.x0.min(m.x1) + origin.x) {
                work += cells.curve(m, origin);
            }
        }
        work
    }

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
                .map_or(0, |m| {
                    let origin = self.origin.unwrap_or_default();
                    if cells.reaches(m.x0.min(m.x1) + origin.x) {
                        cells.curve(m, origin)
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

    #[inline]
    fn cells(&mut self, x: usize, values: &[u8]) {
        let a = x.max(self.x0);
        let b = (x + values.len()).min(self.x1);
        if a < b
            && let Some(values) = values.get(a - x..b - x)
        {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "columns lie within the u32 region"
            )]
            self.coverage.put_row(a as u32, values);
        }
    }
}

/// Where a mark's rows go: its own columns, restricted to the inside and
/// boundary runs of the clip it is masked by.
#[derive(Debug, Clone, Copy)]
struct Target<'a> {
    bounds: DeviceRect,
    mask: Option<&'a Rows>,
}

impl<'a> Target<'a> {
    fn of(shape: &'a Shape) -> Self {
        Self {
            bounds: shape.bounds,
            mask: shape
                .clip
                .as_ref()
                .filter(|_| shape.masked)
                .and_then(ClipShape::rows),
        }
    }

    /// The sink of row `y`.
    fn sink(self, out: &'a mut dyn RowOut, y: u32) -> RowSink<'a> {
        let out = Out {
            coverage: out,
            x0: self.bounds.x0 as usize,
            x1: self.bounds.x1 as usize,
        };
        match self.mask {
            None => RowSink::Plain(out),
            Some(rows) => RowSink::Masked(Masked {
                out,
                segments: rows.row(y),
                next: 0,
                extra: 0,
            }),
        }
    }
}

/// A shape row's sink.
enum RowSink<'a> {
    /// Nothing between the shape and the output.
    Plain(Out<'a>),
    Masked(Masked<'a>),
}

impl RowSink<'_> {
    fn extra(&self) -> usize {
        match self {
            Self::Plain(_) => 0,
            Self::Masked(m) => m.extra,
        }
    }
}

impl Sink for RowSink<'_> {
    #[inline]
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        match self {
            Self::Plain(o) => o.run(x0, x1, value),
            Self::Masked(m) => m.run(x0, x1, value),
        }
    }

    #[inline]
    fn cells(&mut self, x: usize, values: &[u8]) {
        match self {
            Self::Plain(o) => o.cells(x, values),
            Self::Masked(m) => m.cells(x, values),
        }
    }
}

/// Runs of a row kept only where a clip's runs lie: outside them the clip
/// removes the pixels; on its boundary the row was resolved with it.
struct Masked<'a> {
    out: Out<'a>,
    segments: &'a [Segment],
    next: usize,
    extra: usize,
}

impl Sink for Masked<'_> {
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        let mut x = x0;
        while x < x1 {
            while self
                .segments
                .get(self.next)
                .is_some_and(|s| s.x1 as usize <= x)
            {
                self.next += 1;
                self.extra += 1;
            }
            let Some(s) = self.segments.get(self.next) else {
                return;
            };
            if s.x0 as usize >= x1 {
                return;
            }
            let start = x.max(s.x0 as usize);
            let end = x1.min(s.x1 as usize);
            self.out.run(start, end, value);
            x = end;
        }
    }

    fn cells(&mut self, x0: usize, values: &[u8]) {
        let x1 = x0 + values.len();
        let mut x = x0;
        while x < x1 {
            while self
                .segments
                .get(self.next)
                .is_some_and(|s| s.x1 as usize <= x)
            {
                self.next += 1;
                self.extra += 1;
            }
            let Some(s) = self.segments.get(self.next) else {
                return;
            };
            if s.x0 as usize >= x1 {
                return;
            }
            let start = x.max(s.x0 as usize);
            let end = x1.min(s.x1 as usize);
            if let Some(kept) = values.get(start - x0..end - x0) {
                self.out.cells(start, kept);
            }
            x = end;
        }
    }
}
