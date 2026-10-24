//! The band executor (ADR 0034 §4): bands of the global grid top to bottom;
//! in each band the page group's active items in source order, groups and
//! soft masks in band-local buffers on stacks, every output pixel quantized
//! once. Execution reads only what preparation prepared and reserved, so it
//! cannot fail.

mod frame;
mod glyphs;
mod group;
mod mask;
mod output;
mod schedule;
mod transfer;
mod workspace;

pub(crate) use frame::{Backdrop, Buffer, Frame, Read, Tops};
pub(crate) use mask::transfer as mask_transfer;
pub(crate) use schedule::Active;
pub(crate) use transfer::validate;
pub use workspace::Scratch;
pub(crate) use workspace::{
    Extents, MaskCache, PageBuffer, Sizes, Usage, Workspace, coverage_capacity,
};

use crate::kernel::{self, Colors, Mode, Sources};
use crate::plan::{Boundary, Compositing, Item, ItemKind, Mark, Plan, area};
use crate::prepare::Completion;
use crate::prepare::Shapes;
use crate::source::{Pixels, Source, SpanColors};
use pagura_color::Painted;
use pagura_core::convert::u32_to_usize;
use pagura_raster::{BandCoverage, BandRasterizer, DeviceRect, RowOut, Span};
use std::ops::Range;

#[cfg(test)]
mod tests;

/// Coverage stored directly: an ordinary mark or a keyed glyph whose
/// rows can be kept for the next band.
#[derive(Debug, Clone, Copy)]
enum DirectCoverage {
    Mark(crate::plan::Cover),
    Glyph(u32),
}

/// A prepared scene with scratch storage sized for it: what every band's
/// execution works with. Paint execution cannot fail; an output consumer
/// can return its own error.
#[derive(Debug)]
pub struct Rendering<'a> {
    pub(crate) plan: &'a Plan,
    pub(crate) shapes: &'a Shapes,
    pub(crate) workspace: &'a mut Workspace,
    pub(crate) rasterizer: &'a mut BandRasterizer,
}

impl Rendering<'_> {
    /// The maximum premultiplied RGBA8 bytes in one output band. Reserving
    /// this capacity before [`Self::render_bands`] keeps execution from
    /// allocating output storage.
    ///
    /// # Errors
    ///
    /// Returns `Limit` when the band's byte count does not fit `usize`.
    pub fn band_bytes(&self) -> pagura_core::Result<usize> {
        output_band_bytes(self.plan.area)
    }

    /// Renders every band, appending each row of the region to `rows` as
    /// premultiplied RGBA8, `4 × width` bytes, top to bottom. Room for the
    /// rows should be reserved: appending beyond it allocates.
    pub fn render(&mut self, rows: &mut Vec<u8>) {
        let _ = self.render_until(rows, |_| false);
    }

    /// Renders bands as [`Self::render`] does, asking `stop` with the next
    /// band's first row before each band.
    pub fn render_until(
        &mut self,
        rows: &mut Vec<u8>,
        stop: impl FnMut(u32) -> bool,
    ) -> Completion {
        match self.render_each(rows, stop, |_, _| Ok::<(), std::convert::Infallible>(())) {
            Ok(completion) => completion,
            Err(never) => match never {},
        }
    }

    /// Renders top to bottom into a reusable `band`, passing its device-row
    /// range and premultiplied RGBA8 bytes to `consume`. The bytes are valid
    /// only during that call. `band` is cleared before rendering and after
    /// each successful call; reserve [`Self::band_bytes`] first to avoid
    /// allocation during execution.
    ///
    /// As [`Self::render_until`], `stop` is asked before each band. Every
    /// call starts again at the region's first row, including after a stop
    /// or consumer error.
    ///
    /// # Errors
    ///
    /// Returns the first consumer error without rendering another band.
    pub fn render_bands<E>(
        &mut self,
        band: &mut Vec<u8>,
        stop: impl FnMut(u32) -> bool,
        mut consume: impl FnMut(Range<u32>, &[u8]) -> Result<(), E>,
    ) -> Result<Completion, E> {
        band.clear();
        self.render_each(band, stop, |rows, band| {
            consume(rows, band)?;
            band.clear();
            Ok(())
        })
    }

    /// The shared driver: collectors keep appended rows; consumers release
    /// each band's output for reuse. Execution and quantization are shared.
    fn render_each<E>(
        &mut self,
        output: &mut Vec<u8>,
        mut stop: impl FnMut(u32) -> bool,
        mut consume: impl FnMut(Range<u32>, &mut Vec<u8>) -> Result<(), E>,
    ) -> Result<Completion, E> {
        let area = self.plan.area;
        self.workspace.restart();
        self.rasterizer.restart();
        for band in area.bands() {
            let rows = area.band_rows(band);
            let row = rows.start;
            if stop(row) {
                return Ok(Completion::Stopped { row });
            }
            Executor::band(self, band, output);
            consume(rows, output)?;
        }
        Ok(Completion::Complete)
    }
}

/// The largest actual band: clipped edge bands can both be shorter than
/// the grid height. Three or more bands include an unclipped middle band.
fn output_band_bytes(area: pagura_raster::Area) -> pagura_core::Result<usize> {
    let bands = area.bands();
    let rows = if bands.end.saturating_sub(bands.start) > 2 {
        u32_to_usize(area.band_height)
    } else {
        area.band_rows(bands.start)
            .len()
            .max(area.band_rows(bands.end.saturating_sub(1)).len())
    };
    u32_to_usize(area.region.width())
        .checked_mul(rows)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(pagura_core::Error::limit("output band byte size overflows"))
}

/// One band's execution.
pub(crate) struct Executor<'a> {
    plan: &'a Plan,
    ws: &'a mut Workspace,
    shapes: &'a Shapes,
    rasterizer: &'a mut BandRasterizer,
    band: u32,
    rows: Range<u32>,
    first: u32,
}

impl<'a> Executor<'a> {
    /// Executes `band` of `rendering` and appends its rows to `out`.
    fn band(rendering: &'a mut Rendering<'_>, band: u32, out: &mut Vec<u8>) {
        let plan = rendering.plan;
        let rows = plan.area.band_rows(band);
        if rows.is_empty() {
            return;
        }
        let first = plan.area.bands().start;
        let mut executor = Executor {
            plan,
            ws: rendering.workspace,
            shapes: rendering.shapes,
            rasterizer: rendering.rasterizer,
            band,
            rows,
            first,
        };
        executor.page();
        output::write(executor.plan, executor.ws, &executor.rows, out);
    }

    fn page(&mut self) {
        let plan = self.plan;
        let region = plan.area.region;
        let rect = DeviceRect {
            x0: region.x0,
            y0: self.rows.start,
            x1: region.x1,
            y1: self.rows.end,
        };
        let ws = &mut *self.ws;
        ws.page.reset(
            &mut ws.extents,
            (area(rect), u32_to_usize(rect.width())),
            plan.output.background,
            plan.output.layout,
        );
        self.ws.selection.fill(0);
        let Some(page) = plan.groups.first() else {
            return;
        };
        let frame = Frame {
            group: 0,
            buffer: Buffer::Page,
            rect,
            layout: page.layout,
            knockout: page.knockout,
            initial: Backdrop::Transparent,
        };
        self.run(frame, Tops::default());
    }

    /// Runs the active items of `frame`'s group; buffers of nested groups
    /// go above `tops`.
    fn run(&mut self, frame: Frame, tops: Tops) {
        let plan = self.plan;
        let Some(group) = plan.groups.get(u32_to_usize(frame.group)) else {
            return;
        };
        let Some(active) = self.ws.active.get_mut(u32_to_usize(frame.group)) else {
            return;
        };
        active.advance(group, &plan.items, self.band);
        let mut next = 0;
        while let Some(position) = self
            .ws
            .active
            .get(u32_to_usize(frame.group))
            .and_then(|active| active.get(next))
        {
            next += 1;
            let Some(item) = group
                .items
                .get(u32_to_usize(position))
                .and_then(|&id| plan.items.get(u32_to_usize(id)))
            else {
                continue;
            };
            match &item.kind {
                ItemKind::Mark(mark) => self.mark(item, mark, frame, tops),
                ItemKind::Run(run) => self.glyph_run(item, run, frame, tops),
                ItemKind::Group(used) => self.group(item, used, frame, tops),
            }
        }
    }

    /// Counts the rasterization work of `item` once per render.
    fn count_shape(&mut self, item: &Item) {
        if self.band == item.bands[0].max(self.first) {
            self.ws.usage.work = self.ws.usage.work.saturating_add(item.work);
        }
    }

    fn mark(&mut self, item: &Item, mark: &Mark, frame: Frame, tops: Tops) {
        if let Some(mask) = mark.compositing.mask {
            self.ensure_mask(mask, tops);
        }
        self.count_shape(item);
        match mark.boundary {
            Boundary::Shape(boundary) => {
                let outline = crate::plan::Cover {
                    shape: Some(boundary),
                    clip: None,
                };
                self.shapes
                    .rasterize(self.rasterizer, outline, self.band, &mut self.ws.boundary.0);
            }
            Boundary::Empty => self.ws.boundary.0.clear(),
            Boundary::Absent => {}
        }
        if let Some(words) = self.opaque(mark, frame) {
            self.store_direct(DirectCoverage::Mark(mark.cover), frame, words);
            return;
        }
        let mut coverage = std::mem::take(&mut self.ws.coverage);
        self.shapes
            .rasterize(self.rasterizer, mark.cover, self.band, &mut coverage);
        self.store_coverage(mark, frame, &coverage);
        self.ws.coverage = coverage;
    }

    /// Rasterizes `cover` straight into the N16 page as the opaque constant
    /// colour `words`, row by row as the rasterizer emits it: one rounding
    /// per store from each pixel's coverage, nothing else to evaluate.
    fn store_direct(&mut self, cover: DirectCoverage, frame: Frame, words: [u16; 4]) {
        let ws = &mut *self.ws;
        let PageBuffer::N16(page) = &mut ws.page else {
            return;
        };
        let rect = frame.rect;
        let mut out = StoreOut {
            page,
            extents: &mut ws.extents,
            rect,
            width: u32_to_usize(rect.width()),
            words,
            row: None,
            stored: 0,
        };
        match cover {
            DirectCoverage::Mark(cover) => {
                self.shapes
                    .rasterize_into(self.rasterizer, cover, self.band, &mut out);
            }
            DirectCoverage::Glyph(shape) => {
                self.shapes
                    .rasterize_glyph_into(self.rasterizer, shape, self.band, &mut out);
            }
        }
        let stored = out.stored;
        ws.usage.work = ws.usage.work.saturating_add(stored);
    }

    /// Stores every span of `coverage` within `frame`.
    fn store_coverage(&mut self, mark: &Mark, frame: Frame, coverage: &BandCoverage) {
        each_span(coverage, frame.rect, |at, cover| {
            self.mark_span(mark, frame, at, cover);
        });
    }

    /// The words `mark` stores straight from coverage into `frame`: an
    /// opaque constant colour into the N16 page.
    pub(super) fn opaque(&self, mark: &Mark, frame: Frame) -> Option<[u16; 4]> {
        if mark.records || frame.buffer != Buffer::Page {
            return None;
        }
        match &self.ws.page {
            PageBuffer::N16(_) => mark.words,
            PageBuffer::F64(_) | PageBuffer::F32(_) => None,
        }
    }

    fn mark_span(&mut self, mark: &Mark, frame: Frame, (x, y): (u32, u32), cover: Cover<'_>) {
        let plan = self.plan;
        let Some(paint) = plan.paints.get(u32_to_usize(mark.paint)) else {
            return;
        };
        let len = cover.len();
        let first_row = self.rows.start;
        let ws = &mut *self.ws;
        let compositing = &mark.compositing;
        let constant = paint.constant();
        if constant.is_none()
            && (paint_words(ws, (mark, paint), frame, (x, y), cover)
                || !fill_colors(ws, (mark, paint), (x, y), len))
        {
            return;
        }
        let mask = match compositing.mask {
            Some(m) => mask::row(plan, &ws.masks, &mut ws.mask, m, (x, y), len),
            None => None,
        };
        let paint_shape = (constant.is_none() && !paint.shape_is_alpha())
            .then(|| ws.texel_shape.get(..len))
            .flatten();
        let span = match constant {
            Some((color, painted)) => Some((Colors::Constant(color), None, painted)),
            None => filled(&ws.colors, len),
        }
        .filter(|(colors, ..)| colors.holds(len, frame.layout.channels()));
        debug_assert!(
            span.is_some(),
            "paint fills whole spans of the buffer's channels"
        );
        let Some((colors, paint_alpha, painted)) = span else {
            return;
        };
        if mark.records && !ws.selection.is_empty() {
            output::record(
                plan,
                &mut ws.selection,
                (x, y),
                first_row,
                (cover, mark.select),
            );
        }
        let preserve = compositing.overprint & !painted.0 & frame.layout.mask();
        // An opaque source over a whole span at full opacity, without a soft
        // mask, overwrites a narrow page: there are no opacities to form.
        if cover.coverage().is_none()
            && paint_alpha.is_none_or(kernel::full)
            && compositing.alpha == 1.0
            && mask.is_none()
            && compositing.blend == pagura_interpret::BlendMode::Normal
            && preserve == 0
            && let Colors::Strided { values, stride: 3 } = colors
            && overwrite(
                (&mut ws.page, &mut ws.extents),
                frame,
                Pixels::new(x, y, len),
                |pixels| {
                    kernel::overwrite_n16(pixels, values.get(..len * 3).unwrap_or(&[]));
                    true
                },
            )
        {
            ws.usage.work = ws.usage.work.saturating_add(len);
            return;
        }
        let (Some(alpha), Some(shape)) = (ws.alpha.get_mut(..len), ws.shape.get_mut(..len)) else {
            return;
        };
        opacity((paint_alpha, paint_shape), cover.coverage(), alpha, shape);
        apply(compositing, mask, alpha, shape);
        ws.usage.work = ws.usage.work.saturating_add(len);
        let mode = Mode {
            layout: frame.layout,
            blend: compositing.blend,
            knockout: frame.knockout,
            preserve,
        };
        let sources = Sources {
            colors,
            alpha: &ws.alpha[..len],
            shape: &ws.shape[..len],
        };
        let kind = Store::Mark(mark.words.map(|words| (words, cover.coverage())));
        store(
            &mut Target {
                page: &mut ws.page,
                extents: &mut ws.extents,
                wide: &mut ws.wide,
                levels: &mut ws.levels,
                initial: &mut ws.initial,
            },
            frame,
            (x, y),
            (&sources, &mode),
            kind,
        );
    }
}

/// Fills the workspace's span colours (and texel shapes where the paint's
/// shape is not its alpha) with `paint`'s pixels `x .. x + len` of row `y`,
/// under `boundary`; false where the boundary's coverage does not fit.
fn fill_colors(
    ws: &mut Workspace,
    (mark, paint): (&Mark, &Source),
    (x, y): (u32, u32),
    len: usize,
) -> bool {
    let outline = match mark.boundary {
        Boundary::Shape(_) | Boundary::Empty => {
            let (boundary, dense) = &mut ws.boundary;
            let Some(dense) = dense.get_mut(..len) else {
                return false;
            };
            densify(boundary, (x, y), dense);
            Some(&*dense)
        }
        Boundary::Absent => None,
    };
    let pixels = Pixels {
        x,
        y,
        len,
        boundary: outline,
    };
    let Some(scratch) = ws.paint_scratch.get_mut(u32_to_usize(mark.paint)) else {
        return false;
    };
    paint.fill(pixels, scratch, &mut ws.colors, &mut ws.texel_shape);
    true
}

/// The colours, alpha and painted channels of the first `len` pixels a
/// paint filled; `None` where it filled a shorter alpha span.
fn filled(colors: &SpanColors, len: usize) -> Option<(Colors<'_>, Option<&[f64]>, Painted)> {
    let alpha = match colors.alpha.as_slice() {
        [] => None,
        alpha => Some(alpha.get(..len)?),
    };
    let values = Colors::Strided {
        values: &colors.colors,
        stride: colors.channels,
    };
    Some((values, alpha, colors.painted))
}

/// Lets an opaque paint over a whole span at full opacity, without a soft
/// mask, write a narrow page's words itself; false where it does not.
fn paint_words(
    ws: &mut Workspace,
    (mark, paint): (&Mark, &Source),
    frame: Frame,
    (x, y): (u32, u32),
    cover: Cover<'_>,
) -> bool {
    let compositing = &mark.compositing;
    let len = cover.len();
    let Some(scratch) = ws.paint_scratch.get_mut(u32_to_usize(mark.paint)) else {
        return false;
    };
    let written = cover.coverage().is_none()
        && !mark.records
        && mark.boundary == Boundary::Absent
        && compositing.alpha == 1.0
        && compositing.mask.is_none()
        && compositing.blend == pagura_interpret::BlendMode::Normal
        && compositing.overprint == 0
        && overwrite(
            (&mut ws.page, &mut ws.extents),
            frame,
            Pixels::new(x, y, len),
            |pixels| paint.words(Pixels::new(x, y, len), scratch, pixels),
        );
    if written {
        ws.usage.work = ws.usage.work.saturating_add(len);
    }
    written
}

/// Overwrites `len` pixels of `frame`'s narrow page from pixel (`x`, `y`)
/// on with the words `write` gives, as a store at full opacity does; false,
/// with nothing stored, where the frame is not a narrow page or `write`
/// gives none.
fn overwrite(
    (page, extents): (&mut PageBuffer, &mut Extents),
    frame: Frame,
    pixels: Pixels<'_>,
    write: impl FnOnce(&mut [[u16; 4]]) -> bool,
) -> bool {
    let Pixels { x, y, len, .. } = pixels;
    let Some(start) = frame.index(x, y).filter(|_| frame.buffer == Buffer::Page) else {
        return false;
    };
    let stored = match page {
        PageBuffer::N16(page) => page
            .get_mut(start..start + len)
            .is_some_and(write)
            .then_some(()),
        PageBuffer::F64(_) | PageBuffer::F32(_) => None,
    };
    if stored.is_some() {
        let column = x - frame.rect.x0;
        let end = column.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
        extents.mark(u32_to_usize(y - frame.rect.y0), column, end);
    }
    stored.is_some()
}

/// Coverage rows stored as they are emitted: an opaque constant colour's
/// N16 words at each pixel's coverage, within the frame's `rect`.
struct StoreOut<'a> {
    page: &'a mut [[u16; 4]],
    extents: &'a mut Extents,
    rect: DeviceRect,
    width: usize,
    words: [u16; 4],
    /// The current row within `rect`, if it lies there.
    row: Option<usize>,
    stored: usize,
}

/// Columns of the current row within the frame.
struct Columns {
    row: usize,
    /// The first column, relative to the frame.
    column: u32,
    count: usize,
    /// How many of the requested columns lie left of the first.
    skip: usize,
}

impl StoreOut<'_> {
    /// Columns `x .. x + len` of the current row within `rect`, if any.
    fn columns(&self, x: u32, len: usize) -> Option<Columns> {
        let row = self.row?;
        let end = x.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
        let (start, stop) = (x.max(self.rect.x0), end.min(self.rect.x1));
        (start < stop).then(|| Columns {
            row,
            column: start - self.rect.x0,
            count: u32_to_usize(stop - start),
            skip: u32_to_usize(start - x),
        })
    }

    /// The page pixels of `columns`.
    fn pixels(&mut self, columns: &Columns) -> Option<&mut [[u16; 4]]> {
        let first = columns.row * self.width + u32_to_usize(columns.column);
        self.page.get_mut(first..first + columns.count)
    }

    /// Records stores at `lo .. hi` of `columns`.
    fn stored(&mut self, columns: &Columns, (lo, hi): (usize, usize)) {
        let at = |k: usize| columns.column + u32::try_from(k).unwrap_or(u32::MAX);
        self.extents.mark(columns.row, at(lo), at(hi));
        self.stored += hi - lo;
    }
}

impl RowOut for StoreOut<'_> {
    fn push_row(&mut self, y: u32) {
        self.row = (self.rect.y0..self.rect.y1)
            .contains(&y)
            .then(|| u32_to_usize(y - self.rect.y0));
    }

    fn put_run(&mut self, x: u32, len: u32, value: u8) {
        let words = self.words;
        let Some(columns) = self.columns(x, u32_to_usize(len)).filter(|_| value != 0) else {
            return;
        };
        if let Some(pixels) = self.pixels(&columns) {
            if value == u8::MAX {
                pixels.fill(words);
            } else {
                kernel::store_n16_uniform(pixels, words, value);
            }
        }
        self.stored(&columns, (0, columns.count));
    }

    fn put_row(&mut self, x: u32, values: &[u8]) {
        let words = self.words;
        let Some(columns) = self.columns(x, values.len()) else {
            return;
        };
        let Some(values) = values.get(columns.skip..columns.skip + columns.count) else {
            return;
        };
        // Zero coverage leaves a pixel as it is: only covered pixels count.
        let covered = self
            .pixels(&columns)
            .and_then(|pixels| kernel::store_n16_runs(pixels, words, values));
        if let Some(stretch) = covered {
            self.stored(&columns, stretch);
        }
    }
}

/// The coverage of `coverage` at the pixels from `(x, y)`, one byte each.
fn densify(coverage: &BandCoverage, (x, y): (u32, u32), out: &mut [u8]) {
    out.fill(0);
    let end = x.saturating_add(u32::try_from(out.len()).unwrap_or(u32::MAX));
    for row in coverage.rows().filter(|row| row.y == y) {
        for span in row.spans() {
            let (start, len) = match span {
                Span::Full { x, len } => (x, len),
                Span::Partial { x, coverage } => (x, u32::try_from(coverage.len()).unwrap_or(0)),
            };
            for column in start.max(x)..start.saturating_add(len).min(end) {
                let value = match span {
                    Span::Full { .. } => 255,
                    Span::Partial { coverage, .. } => coverage
                        .get(u32_to_usize(column - start))
                        .copied()
                        .unwrap_or(0),
                };
                if let Some(slot) = out.get_mut(u32_to_usize(column - x)) {
                    *slot = value;
                }
            }
        }
    }
}

/// The pixels of a span within a frame.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Cover<'a> {
    /// Fully covered pixels.
    Full(usize),
    /// Coverage bytes.
    Partial(&'a [u8]),
}

impl<'a> Cover<'a> {
    /// Pixels.
    pub(crate) fn len(&self) -> usize {
        match *self {
            Self::Full(len) => len,
            Self::Partial(coverage) => coverage.len(),
        }
    }

    /// The coverage bytes; `None` when full.
    pub(crate) fn coverage(&self) -> Option<&'a [u8]> {
        match *self {
            Self::Full(_) => None,
            Self::Partial(coverage) => Some(coverage),
        }
    }

    /// The coverage of pixel `i`.
    pub(crate) fn at(&self, i: usize) -> u8 {
        match *self {
            Self::Full(_) => 255,
            Self::Partial(coverage) => coverage.get(i).copied().unwrap_or(0),
        }
    }

    /// Pixels `start .. end`.
    pub(crate) fn slice(&self, start: usize, end: usize) -> Self {
        match *self {
            Self::Full(_) => Self::Full(end.saturating_sub(start)),
            Self::Partial(coverage) => Self::Partial(coverage.get(start..end).unwrap_or_default()),
        }
    }
}

/// Calls `store` with every span of `coverage` within `rect` and its first
/// pixel.
fn each_span(
    coverage: &BandCoverage,
    rect: DeviceRect,
    mut store: impl FnMut((u32, u32), Cover<'_>),
) {
    for row in coverage.rows() {
        if row.y < rect.y0 || row.y >= rect.y1 {
            continue;
        }
        for span in row.spans() {
            if let Some((x, cover)) = clamp(span, rect) {
                store((x, row.y), cover);
            }
        }
    }
}

/// `span` restricted to the columns of `rect`, with its first column.
pub(crate) fn clamp(span: Span<'_>, rect: DeviceRect) -> Option<(u32, Cover<'_>)> {
    let (x, len) = match span {
        Span::Full { x, len } => (x, len),
        Span::Partial { x, coverage } => (x, u32::try_from(coverage.len()).ok()?),
    };
    let start = x.max(rect.x0);
    let end = x.saturating_add(len).min(rect.x1);
    if start >= end {
        return None;
    }
    let count = u32_to_usize(end - start);
    let cover = match span {
        Span::Full { .. } => Cover::Full(count),
        Span::Partial { coverage, .. } => {
            let skip = u32_to_usize(start - x);
            Cover::Partial(coverage.get(skip..skip + count)?)
        }
    };
    Some((start, cover))
}

/// Source alpha and shape of a mark's pixels: the paint's alpha and shape
/// (its alpha unless given) times coverage, multiplied in that order.
fn opacity(
    (paint, paint_shape): (Option<&[f64]>, Option<&[f64]>),
    coverage: Option<&[u8]>,
    alpha: &mut [f64],
    shape: &mut [f64],
) {
    if let Some(values) = paint {
        alpha.copy_from_slice(values);
    } else {
        alpha.fill(1.0);
    }
    match paint_shape {
        Some(values) => shape.copy_from_slice(values),
        None => shape.copy_from_slice(alpha),
    }
    if let Some(coverage) = coverage {
        for ((a, s), &c) in alpha.iter_mut().zip(shape.iter_mut()).zip(coverage) {
            let factor = UNIT[usize::from(c)];
            *a *= factor;
            *s *= factor;
        }
    }
}

/// `c / 255` for every coverage byte, as the division gives it.
const UNIT: [f64; 256] = {
    let mut unit = [0.0; 256];
    let mut c = 0;
    while c < 256 {
        unit[c] = c as f64 / 255.0;
        c += 1;
    }
    unit
};

/// Constant alpha and soft mask (11.6.4.3): alpha always, shape too under
/// `AIS` (11.6.4.4).
pub(crate) fn apply(
    compositing: &Compositing,
    mask: Option<&[f64]>,
    alpha: &mut [f64],
    shape: &mut [f64],
) {
    let constant = compositing.alpha;
    let shaped = compositing.alpha_is_shape;
    if mask.is_none() && constant == 1.0 {
        // Multiplying by one changes nothing.
        return;
    }
    match mask {
        Some(values) => {
            for ((a, s), &m) in alpha.iter_mut().zip(shape.iter_mut()).zip(values) {
                let factor = constant * m;
                *a *= factor;
                if shaped {
                    *s *= factor;
                }
            }
        }
        None => {
            // Without a soft mask the factor `constant × 1.0` is `constant`
            // exactly.
            for (a, s) in alpha.iter_mut().zip(shape.iter_mut()) {
                *a *= constant;
                if shaped {
                    *s *= constant;
                }
            }
        }
    }
}

/// The buffers a store may write.
pub(crate) struct Target<'a> {
    pub(crate) page: &'a mut PageBuffer,
    pub(crate) extents: &'a mut Extents,
    pub(crate) wide: &'a mut [f64],
    pub(crate) levels: &'a mut [[f32; 4]],
    pub(crate) initial: &'a mut Vec<f64>,
}

/// What a store is.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Store<'a> {
    /// A mark, with the words and coverage of an opaque constant store for
    /// N16.
    Mark(Option<([u16; 4], Option<&'a [u8]>)>),
    /// A child's result, rounded once into narrow storage.
    Result,
}

/// Stores one span of sources into `frame`'s buffer.
pub(crate) fn store(
    target: &mut Target<'_>,
    frame: Frame,
    (x, y): (u32, u32),
    (sources, mode): (&Sources<'_>, &Mode),
    kind: Store<'_>,
) {
    let len = sources.alpha.len();
    let Some(start) = frame.index(x, y) else {
        return;
    };
    let stride = frame.layout.stride();
    if mode.knockout {
        let Some(initial) = target.initial.get_mut(..len * stride) else {
            return;
        };
        let read = Read {
            page: &*target.page,
            wide: &*target.wide,
            levels: &*target.levels,
        };
        frame.initial.row(read, (x, y), frame.layout, initial);
    }
    let initial = if mode.knockout {
        target.initial.get(..len * stride).unwrap_or_default()
    } else {
        &[]
    };
    let colors = &sources.colors;
    match frame.buffer {
        Buffer::Wide(offset) => {
            let begin = offset + start * stride;
            if let Some(pixels) = target.wide.get_mut(begin..begin + len * stride) {
                kernel::composite(pixels, sources, initial, mode);
            }
        }
        Buffer::Levels(offset) => {
            if let Some(pixels) = target.levels.get_mut(offset + start..offset + start + len) {
                levels(pixels, colors, sources.alpha, kind);
            }
        }
        Buffer::Page => {
            let row = u32_to_usize(y - frame.rect.y0);
            let start_column = x - frame.rect.x0;
            let end_column = start_column.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
            target.extents.mark(row, start_column, end_column);
            page(target.page, (start, stride), (sources, mode), initial, kind);
        }
    }
}

/// Stores one span into the page buffer from pixel `start`.
fn page(
    page: &mut PageBuffer,
    (start, stride): (usize, usize),
    (sources, mode): (&Sources<'_>, &Mode),
    initial: &[f64],
    kind: Store<'_>,
) {
    let len = sources.alpha.len();
    let colors = &sources.colors;
    match page {
        PageBuffer::F64(values) => {
            let begin = start * stride;
            if let Some(pixels) = values.get_mut(begin..begin + len * stride) {
                kernel::composite(pixels, sources, initial, mode);
            }
        }
        PageBuffer::N16(words) => {
            if let Some(pixels) = words.get_mut(start..start + len) {
                match kind {
                    Store::Mark(Some((source, coverage))) => {
                        kernel::store_n16(pixels, source, coverage);
                    }
                    Store::Mark(None) | Store::Result => {
                        kernel::dynamic_n16(pixels, colors, sources.alpha);
                    }
                }
            }
        }
        PageBuffer::F32(values) => {
            if let Some(pixels) = values.get_mut(start..start + len) {
                levels(pixels, colors, sources.alpha, kind);
            }
        }
    }
}

fn levels(pixels: &mut [[f32; 4]], colors: &Colors<'_>, alpha: &[f64], kind: Store<'_>) {
    match kind {
        Store::Mark(_) => kernel::store_f32(pixels, colors, alpha),
        Store::Result => kernel::dynamic_f32(pixels, colors, alpha),
    }
}
