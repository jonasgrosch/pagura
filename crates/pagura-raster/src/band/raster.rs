//! The band rasterizer: one strip of cells, reused for every mark and band.

use super::cells::{Cells, STRIP};
use super::clip::{Rows, Segment};
use super::coverage::RowOut;
use super::curve::Mono;
use super::edges::{Line, StripList};
use super::glyph::{Kind, PolygonLines, point_sides};
use super::row::Sink;
use super::shape::{Body, GlyphBody, Resolved, Shape};
use super::{Area, BandCoverage, ClipShape, DeviceRect, MarkCoverage};
use crate::{FillRule, Point};
use pagura_core::{Error, Meter, Result};

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
                    lines: Lines::Slice {
                        lines: &edges.lines,
                        point: None,
                    },
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
                let items = Items::glyph(glyph, &shape.resolved);
                let target = Target::of(shape);
                self.items(&items, (target, glyph.scan, FillRule::NonZero), rows, out);
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
            let mut work = items.accumulate::<false>(&mut self.cells, &window);
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
            lines: Lines::Slice {
                lines: &sides,
                point: None,
            },
            curves: &[],
            origin: None,
            strips: None,
            resolved: &none,
        };
        let target = Target { bounds, mask: None };
        self.items(&items, (target, scan, FillRule::NonZero), rows, out);
    }
}

/// A mark's straight sides, owned by the placement or borrowed from its
/// source, with an optional canonical point contour. Strip indexes keep a
/// polygon's source order, with keys that recover the same placed side.
enum Lines<'a> {
    Slice {
        lines: &'a [Line],
        point: Option<&'a GlyphBody>,
    },
    Polygon(PolygonLines<'a>),
}

/// A mark's items: its lines, then its monotone quadratics, moved by a
/// glyph's origin, and listed per strip where the mark has lists.
struct Items<'a> {
    lines: Lines<'a>,
    curves: &'a [Mono],
    origin: Option<Point>,
    strips: Option<&'a StripList>,
    /// Rows whose sides replace the items.
    resolved: &'a Resolved,
}

impl Items<'_> {
    fn glyph<'a>(glyph: &'a GlyphBody, resolved: &'a Resolved) -> Items<'a> {
        let (lines, curves) = match glyph.geometry {
            super::shape::GlyphGeometry::Relative => {
                let Some(relative) = glyph.edges.relative() else {
                    unreachable!("a relative glyph source");
                };
                debug_assert!(glyph.strips.is_none());
                let lines = Lines::Slice {
                    lines: &relative.lines,
                    point: (!glyph.edges.points().is_empty()).then_some(glyph),
                };
                (lines, relative.curves.as_slice())
            }
            super::shape::GlyphGeometry::Polygon => {
                let Kind::Polygon {
                    polygon, points, ..
                } = &glyph.edges.kind
                else {
                    unreachable!("a canonical polygon glyph source");
                };
                (
                    Lines::Polygon(PolygonLines::new(polygon, points, glyph.origin)),
                    &[][..],
                )
            }
        };
        Items {
            lines,
            curves,
            origin: Some(glyph.origin),
            strips: glyph.strips.as_ref(),
            resolved,
        }
    }

    /// The common accumulation for execution and preparation inspection.
    fn accumulate<const MEASURE: bool>(
        &self,
        cells: &mut Cells,
        window: &std::ops::Range<u32>,
    ) -> usize {
        let resolved = !self.resolved.is_empty();
        if resolved {
            cells.skip(self.resolved.mask(window));
        }
        let mut work = if let Some(list) = self.strips {
            let listed = list.strip(window.start / STRIP);
            listed.len()
                + listed
                    .iter()
                    .map(|&item| self.add::<MEASURE>(cells, item as usize))
                    .sum::<usize>()
        } else {
            self.all::<MEASURE>(cells)
        };
        if resolved {
            cells.skip(0);
            let sides = self.resolved.lines(window);
            work += sides.len();
            for &line in sides {
                work += if MEASURE {
                    cells.line_measured(line)
                } else {
                    cells.line(line)
                };
            }
        }
        work
    }

    /// Adds every item to the open window: lines, curves, then canonical
    /// point contours. Returns the work spent, one unit per item visited.
    fn all<const MEASURE: bool>(&self, cells: &mut Cells) -> usize {
        let origin = self.origin.unwrap_or_default();
        let at = |p: Point| Point::new(p.x + origin.x, p.y + origin.y);
        let mut work = self.curves.len();
        match &self.lines {
            Lines::Slice { lines, .. } => {
                work += lines.len();
                for l in *lines {
                    work += Self::line::<MEASURE>(
                        cells,
                        Line {
                            a: at(l.a),
                            b: at(l.b),
                        },
                    );
                }
            }
            Lines::Polygon(view) => {
                // Count every visited side, including those whose placed
                // endpoints collapse to a horizontal line.
                work = work.saturating_add(view.raw_count().unwrap_or(usize::MAX));
                view.for_each_unindexed(|line| {
                    if !line.is_horizontal() {
                        work += Self::line::<MEASURE>(cells, line);
                    }
                });
            }
        }
        for m in self.curves {
            if cells.reaches(m.x0.min(m.x1) + origin.x) {
                work += if MEASURE {
                    cells.curve_measured(m, origin)
                } else {
                    cells.curve(m, origin)
                };
            }
        }
        if let Lines::Slice {
            point: Some(glyph), ..
        } = &self.lines
        {
            for &point in glyph.edges.points() {
                work += 2;
                let [right, left] = point_sides(point + glyph.origin);
                let rows = cells.rows();
                let scan = f64::from(u32::try_from(glyph.scan.0).unwrap_or(u32::MAX));
                // A whole cell outside the global scan contributes no carry.
                // Within that scan, keep both sides even left of the output
                // region: its prefix arithmetic is shared with a full render.
                if right.a.y >= f64::from(rows.start)
                    && right.a.y < f64::from(rows.end)
                    && right.a.x > scan
                    && cells.reaches(left.a.x)
                {
                    for line in [right, left] {
                        // These are absolute canonical endpoints. Adding an
                        // origin or a default zero again changes their bits.
                        work += Self::line::<MEASURE>(cells, line);
                    }
                }
            }
        }
        work
    }

    #[inline]
    fn line<const MEASURE: bool>(cells: &mut Cells, line: Line) -> usize {
        if !cells.reaches(line.a.x.min(line.b.x)) {
            0
        } else if MEASURE {
            cells.line_measured(line)
        } else {
            cells.line(line)
        }
    }

    /// Adds item `item` to the open window: each point plus the origin, as
    /// translating the vertices first would give. Returns the work spent.
    #[inline]
    fn add<const MEASURE: bool>(&self, cells: &mut Cells, item: usize) -> usize {
        let lines = match &self.lines {
            Lines::Slice { lines, point } => {
                debug_assert!(point.is_none());
                lines
            }
            Lines::Polygon(view) => {
                return view
                    .line(item)
                    .map_or(0, |line| Self::line::<MEASURE>(cells, line));
            }
        };
        let at = |p: Point| match self.origin {
            Some(o) => Point::new(p.x + o.x, p.y + o.y),
            None => p,
        };
        // Items right of the window change none of its cells.
        match lines.get(item) {
            Some(l) => {
                let line = Line {
                    a: at(l.a),
                    b: at(l.b),
                };
                if cells.reaches(line.a.x.min(line.b.x)) {
                    if MEASURE {
                        cells.line_measured(line)
                    } else {
                        cells.line(line)
                    }
                } else {
                    0
                }
            }
            None => self
                .curves
                .get(item.wrapping_sub(lines.len()))
                .map_or(0, |m| {
                    let origin = self.origin.unwrap_or_default();
                    if cells.reaches(m.x0.min(m.x1) + origin.x) {
                        if MEASURE {
                            cells.curve_measured(m, origin)
                        } else {
                            cells.curve(m, origin)
                        }
                    } else {
                        0
                    }
                }),
        }
    }
}

/// Inspect the same accumulated areas execution emits, before its single
/// coverage quantization. Allocation and the execution work upper bound
/// are charged by the caller before inspection starts.
pub(crate) fn inspect(
    shape: &Shape,
    cells: &mut Cells,
    mut visit: impl FnMut(u32, usize, usize, f64, f64),
) -> Result<()> {
    let (items, scan) = match &shape.body {
        Body::Edges(edges) => (
            Items {
                lines: Lines::Slice {
                    lines: &edges.lines,
                    point: None,
                },
                curves: &edges.curves,
                origin: None,
                strips: edges.strips.as_ref(),
                resolved: &shape.resolved,
            },
            edges.scan,
        ),
        Body::Glyph(glyph) => (Items::glyph(glyph, &shape.resolved), glyph.scan),
    };
    let target = Target::of(shape);
    let fit = cells.rows_fit(scan.1.saturating_sub(scan.0));
    let mut y = shape.bounds.y0;
    while y < shape.bounds.y1 {
        let end = match items.strips {
            Some(_) => (y / STRIP + 1).saturating_mul(STRIP).min(shape.bounds.y1),
            None => shape.bounds.y1,
        };
        let window = y..end.min(y.saturating_add(fit));
        y = window.end;
        if !cells.open(scan, window.clone()) {
            return Err(Error::limit("glyph inspection cells exceed allocation"));
        }
        items.accumulate::<true>(cells, &window);
        let rounding = cells.arithmetic_error();
        for (r, row) in (0_usize..).zip(cells.rows()) {
            let mut next = 0;
            cells.inspect(r, |x0, x1, area| {
                let x0 = x0.max(target.bounds.x0 as usize);
                let x1 = x1.min(target.bounds.x1 as usize);
                if let Some(mask) = target.mask {
                    masked_ranges(mask.row(row), &mut next, (x0, x1), |a, b| {
                        visit(row, a, b, area.abs(), rounding);
                    });
                } else if x0 < x1 {
                    visit(row, x0, x1, area.abs(), rounding);
                }
            });
        }
    }
    Ok(())
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
        self.extra += masked_ranges(self.segments, &mut self.next, (x0, x1), |a, b| {
            self.out.run(a, b, value);
        });
    }

    fn cells(&mut self, x0: usize, values: &[u8]) {
        let x1 = x0 + values.len();
        self.extra += masked_ranges(self.segments, &mut self.next, (x0, x1), |a, b| {
            if let Some(kept) = values.get(a - x0..b - x0) {
                self.out.cells(a, kept);
            }
        });
    }
}

/// One monotone clip-row cursor for execution and unquantized inspection.
/// Each segment advances at most once per row, regardless of touched cells.
fn masked_ranges(
    segments: &[Segment],
    next: &mut usize,
    (mut x, x1): (usize, usize),
    mut keep: impl FnMut(usize, usize),
) -> usize {
    let mut advanced = 0;
    while x < x1 {
        while segments.get(*next).is_some_and(|s| s.x1 as usize <= x) {
            *next += 1;
            advanced += 1;
        }
        let Some(segment) = segments.get(*next) else {
            break;
        };
        if segment.x0 as usize >= x1 {
            break;
        }
        let a = x.max(segment.x0 as usize);
        let b = x1.min(segment.x1 as usize);
        keep(a, b);
        x = b;
    }
    advanced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_source_does_not_grow_the_item_view() {
        #[expect(dead_code, reason = "only the prior view layout is compared")]
        enum OldLines<'a> {
            Slice(&'a [Line]),
            Polygon(PolygonLines<'a>),
        }
        assert_eq!(size_of::<Lines<'_>>(), size_of::<OldLines<'_>>());
    }

    #[test]
    fn inspection_and_emission_advance_each_mask_segment_once() {
        let segments: Vec<_> = (0..256)
            .map(|i| Segment {
                x0: i * 2,
                x1: i * 2 + 1,
                value: 255,
                inside: true,
            })
            .collect();
        let (mut next, mut advanced, mut visits) = (0, 0, 0);
        for x in 0..512 {
            advanced += masked_ranges(&segments, &mut next, (x, x + 1), |a, b| {
                assert_eq!((a, b), (x, x + 1));
                visits += 1;
            });
        }
        assert_eq!(visits, segments.len());
        assert_eq!(advanced, segments.len());
    }
}
