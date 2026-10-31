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
    Extents, MaskCache, PageBand, PageBuffer, Sizes, Usage, Workspace, coverage_capacity,
};

use crate::kernel::{self, Colors, Mode, Sources};
use crate::plan::{Boundary, Compositing, Item, ItemKind, Layout, Mark, Plan, area};
use crate::prepare::Completion;
use crate::prepare::Shapes;
use crate::source::{Pixels, Precision, Source, SpanColors};
use pagura_color::Painted;
use pagura_core::convert::u32_to_usize;
use pagura_raster::{BandCoverage, BandRasterizer, DeviceRect, RowOut, Span};
use std::ops::Range;

#[cfg(test)]
mod tests;

/// An admitted constant store and the matching storage of the page or a
/// group, borrowed together so execution cannot pair a colour with the
/// wrong format.
enum Direct<'a> {
    N16(StoreOut<'a, [u16; 4]>),
    Dynamic(StoreOut<'a, Dynamic>),
    F32(StoreOut<'a, Levels<4>>),
    F32Cmyk(StoreOut<'a, Levels<5>>),
    F64(WideOut<'a>),
}

impl<'a> Direct<'a> {
    fn new(
        plan: &'a Plan,
        (page, wide, levels): (&'a mut PageBand, &'a mut [f64], &'a mut [[f32; 4]]),
        mark: &Mark,
        frame: Frame,
    ) -> Option<Self> {
        let compositing = &mark.compositing;
        if mark.records
            || frame.knockout
            || compositing.mask.is_some()
            || compositing.blend != pagura_interpret::BlendMode::Normal
            || compositing.overprint != 0
        {
            return None;
        }
        let paint = plan.paints.get(u32_to_usize(mark.paint))?;
        let (color, _) = paint
            .constant()
            .filter(|(color, _)| color.len() >= frame.layout.channels())?;
        let alpha = compositing.alpha;
        // A group's buffer stores as its span stores do; extents and the
        // background are the page's.
        let pixels = area(frame.rect);
        match frame.buffer {
            Buffer::Page => {}
            Buffer::Wide(offset) => {
                let page = wide.get_mut(offset..offset + pixels * frame.layout.stride())?;
                return (alpha == 1.0).then_some(Self::F64(WideOut {
                    page,
                    extents: None,
                    rect: frame.rect,
                    layout: frame.layout,
                    color,
                    row: None,
                    stored: 0,
                }));
            }
            Buffer::Levels(offset) => {
                let page = levels.get_mut(offset..offset + pixels)?;
                let color = (Levels(kernel::levels(color), alpha), [0.0; 4]);
                return Some(Self::F32(StoreOut::new(page, None, frame.rect, color)));
            }
        }
        let background = plan.output.background;
        let (pixels, extents) = page.parts_mut();
        let extents = Some(extents);
        Some(match (pixels, mark.words) {
            (PageBuffer::N16(page), words) => {
                let background = background.map(|b| u16::from(b) * 257);
                match words {
                    Some(words) => Self::N16(StoreOut::new(
                        page,
                        extents,
                        frame.rect,
                        (words, background),
                    )),
                    None => Self::Dynamic(StoreOut::new(
                        page,
                        extents,
                        frame.rect,
                        (Dynamic(kernel::targets(color, 65535.0), alpha), background),
                    )),
                }
            }
            (PageBuffer::F32(page), _) => Self::F32(StoreOut::new(
                page,
                extents,
                frame.rect,
                (
                    Levels(kernel::levels(color), alpha),
                    background.map(f32::from),
                ),
            )),
            // A CMYK page starts transparent: output adds the background.
            (PageBuffer::F32Cmyk(page), _) => Self::F32Cmyk(StoreOut::new(
                page,
                extents,
                frame.rect,
                (Levels(kernel::levels(color), alpha), [0.0; 5]),
            )),
            (PageBuffer::F64(page), _) if alpha == 1.0 => Self::F64(WideOut {
                page,
                extents,
                rect: frame.rect,
                layout: frame.layout,
                color,
                row: None,
                stored: 0,
            }),
            (PageBuffer::F64(_), _) => return None,
        })
    }

    /// Emits coverage into the admitted store and counts the pixels written.
    fn rasterize(&mut self, rasterize: impl FnOnce(&mut dyn RowOut)) -> usize {
        fn emit<S: Stores>(
            out: &mut StoreOut<'_, S>,
            rasterize: impl FnOnce(&mut dyn RowOut),
        ) -> usize {
            rasterize(out);
            std::mem::take(&mut out.stored)
        }
        match self {
            Self::N16(out) => emit(out, rasterize),
            Self::Dynamic(out) => emit(out, rasterize),
            Self::F32(out) => emit(out, rasterize),
            Self::F32Cmyk(out) => emit(out, rasterize),
            Self::F64(out) => {
                rasterize(out);
                std::mem::take(&mut out.stored)
            }
        }
    }
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
        ws.page
            .reset(area(rect), plan.output.background, plan.output.layout);
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
            started: None,
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
        let storage = (
            &mut self.ws.page,
            &mut self.ws.wide[..],
            &mut self.ws.levels[..],
        );
        if let Some(mut direct) = Direct::new(self.plan, storage, mark, frame) {
            let stored = direct.rasterize(|out| {
                self.shapes
                    .rasterize_into(self.rasterizer, mark.cover, self.band, out);
            });
            self.ws.usage.work = self.ws.usage.work.saturating_add(stored);
            return;
        }
        let mut coverage = std::mem::take(&mut self.ws.coverage);
        self.shapes
            .rasterize(self.rasterizer, mark.cover, self.band, &mut coverage);
        self.store_coverage(mark, frame, &coverage);
        self.ws.coverage = coverage;
    }

    /// Stores every span of `coverage` within `frame`.
    fn store_coverage(&mut self, mark: &Mark, frame: Frame, coverage: &BandCoverage) {
        each_span(coverage, frame.rect, |at, cover| {
            self.mark_span(mark, frame, at, cover);
        });
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
                || !fill_colors(ws, (mark, paint), frame, (x, y), len))
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
            && overwrite(&mut ws.page, frame, Pixels::new(x, y, len), |pixels| {
                kernel::overwrite_n16(pixels, values.get(..len * 3).unwrap_or(&[]));
                true
            })
        {
            ws.usage.work = ws.usage.work.saturating_add(len);
            return;
        }
        let (Some(alpha), Some(shape)) = (ws.alpha.get_mut(..len), ws.shape.get_mut(..len)) else {
            return;
        };
        let (alpha, shape) = source_opacity(
            (paint_alpha, paint_shape),
            (cover, compositing, mask),
            (alpha, shape),
        );
        ws.usage.work = ws.usage.work.saturating_add(len);
        let mode = Mode {
            layout: frame.layout,
            blend: compositing.blend,
            knockout: frame.knockout,
            preserve,
        };
        let sources = Sources {
            colors,
            alpha,
            shape,
        };
        let kind = Store::Mark(mark.words.map(|words| (words, cover.coverage())));
        store(
            &mut Target {
                page: &mut ws.page,
                wide: &mut ws.wide,
                levels: &mut ws.levels,
                starts: &mut ws.starts,
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
    frame: Frame,
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
    // A mark stores into F32 levels through `kernel::level` alone.
    let levels = match frame.buffer {
        Buffer::Levels(_) => true,
        Buffer::Page => matches!(
            ws.page.pixels(),
            PageBuffer::F32(_) | PageBuffer::F32Cmyk(_)
        ),
        Buffer::Wide(_) => false,
    };
    let pixels = Pixels {
        x,
        y,
        len,
        boundary: outline,
        precision: if levels {
            Precision::Level
        } else {
            Precision::Exact
        },
    };
    let Some(scratch) = ws.paint_scratch.get_mut(u32_to_usize(mark.scratch)) else {
        return false;
    };
    scratch.claim(mark.paint);
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
    let Some(scratch) = ws.paint_scratch.get_mut(u32_to_usize(mark.scratch)) else {
        return false;
    };
    scratch.claim(mark.paint);
    let written = cover.coverage().is_none()
        && !mark.records
        && mark.boundary == Boundary::Absent
        && compositing.alpha == 1.0
        && compositing.mask.is_none()
        && compositing.blend == pagura_interpret::BlendMode::Normal
        && compositing.overprint == 0
        && overwrite(&mut ws.page, frame, Pixels::new(x, y, len), |pixels| {
            paint.words(Pixels::new(x, y, len), scratch, pixels)
        });
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
    band: &mut PageBand,
    frame: Frame,
    pixels: Pixels<'_>,
    write: impl FnOnce(&mut [[u16; 4]]) -> bool,
) -> bool {
    let Pixels { x, y, len, .. } = pixels;
    let Some(start) = frame.index(x, y).filter(|_| frame.buffer == Buffer::Page) else {
        return false;
    };
    let (page, extents) = band.parts_mut();
    let stored = match page {
        PageBuffer::N16(page) => page
            .get_mut(start..start + len)
            .is_some_and(write)
            .then_some(()),
        PageBuffer::F64(_) | PageBuffer::F32(_) | PageBuffer::F32Cmyk(_) => None,
    };
    if stored.is_some() {
        let column = x - frame.rect.x0;
        let end = column.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
        extents.mark(u32_to_usize(y - frame.rect.y0), column, end);
    }
    stored.is_some()
}

/// A constant colour stored into narrow page pixels at coverage bytes.
trait Stores: Copy {
    /// N16 words or F32 levels.
    type Pixel: Copy + PartialEq;
    /// The pixel a store at full coverage leaves, whatever it covers.
    fn opaque(self) -> Option<Self::Pixel> {
        None
    }
    /// Stores at coverage `c` into every pixel.
    fn uniform(self, pixels: &mut [Self::Pixel], c: u8);
    /// Stores at `coverage`, which leaves pixels at zero as they are; the
    /// pixels from the first to past the last one with coverage.
    fn runs(self, pixels: &mut [Self::Pixel], coverage: &[u8]) -> Option<(usize, usize)>;
}

impl Stores for [u16; 4] {
    type Pixel = Self;

    fn opaque(self) -> Option<Self> {
        Some(self)
    }

    #[inline]
    fn uniform(self, pixels: &mut [Self], c: u8) {
        kernel::store_n16_uniform(pixels, self, c);
    }

    #[inline]
    fn runs(self, pixels: &mut [Self], coverage: &[u8]) -> Option<(usize, usize)> {
        kernel::store_n16_runs(pixels, self, coverage)
    }
}

/// The targets of N16 dynamic stores at a constant alpha.
#[derive(Debug, Clone, Copy)]
pub(super) struct Dynamic([f64; 4], f64);

impl Stores for Dynamic {
    type Pixel = [u16; 4];

    #[inline]
    fn uniform(self, pixels: &mut [[u16; 4]], c: u8) {
        kernel::dynamic_n16_uniform(pixels, (self.0, self.1), c);
    }

    #[inline]
    fn runs(self, pixels: &mut [[u16; 4]], coverage: &[u8]) -> Option<(usize, usize)> {
        kernel::dynamic_n16_runs(pixels, (self.0, self.1), coverage)
    }
}

/// F32 levels of `N` lanes at a constant alpha.
#[derive(Debug, Clone, Copy)]
pub(super) struct Levels<const N: usize>([f32; N], f64);

impl<const N: usize> Stores for Levels<N> {
    type Pixel = [f32; N];

    fn opaque(self) -> Option<[f32; N]> {
        (self.1 == 1.0).then_some(self.0)
    }

    #[inline]
    fn uniform(self, pixels: &mut [[f32; N]], c: u8) {
        kernel::store_f32_uniform(pixels, (self.0, self.1), c);
    }

    #[inline]
    fn runs(self, pixels: &mut [[f32; N]], coverage: &[u8]) -> Option<(usize, usize)> {
        kernel::store_f32_runs(pixels, (self.0, self.1), coverage)
    }
}

/// Coverage rows stored as they are emitted: a constant colour at each
/// pixel's coverage, within the frame's `rect`.
struct StoreOut<'a, S: Stores> {
    page: &'a mut [S::Pixel],
    /// The page's stored columns; none for a group's buffer.
    extents: Option<&'a mut Extents>,
    rect: DeviceRect,
    width: usize,
    color: S,
    /// The colour is the background, which pixels no store reached hold.
    background: bool,
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

impl<'a, S: Stores> StoreOut<'a, S> {
    /// Stores of `color` into `page`, the pixels of `rect` row by row,
    /// where on the page pixels no store reached hold `background`.
    fn new(
        page: &'a mut [S::Pixel],
        extents: Option<&'a mut Extents>,
        rect: DeviceRect,
        (color, background): (S, S::Pixel),
    ) -> Self {
        Self {
            page,
            background: extents.is_some() && color.opaque() == Some(background),
            extents,
            rect,
            width: u32_to_usize(rect.width()),
            color,
            row: None,
            stored: 0,
        }
    }

    /// Columns `x .. x + len` of the current row within `rect`, if any.
    fn columns(&self, x: u32, len: usize) -> Option<Columns> {
        let row = self.row?;
        let end = x.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
        let (mut start, mut stop) = (x.max(self.rect.x0), end.min(self.rect.x1));
        if self.background {
            // A store of the background, at any coverage, leaves a pixel
            // that holds it as it is: only stored columns can change.
            let (lo, hi) = self.extents.as_ref()?.get(row)?;
            start = start.max(self.rect.x0 + lo);
            stop = stop.min(self.rect.x0 + hi);
        }
        (start < stop).then(|| Columns {
            row,
            column: start - self.rect.x0,
            count: u32_to_usize(stop - start),
            skip: u32_to_usize(start - x),
        })
    }

    /// The page pixels of `columns`.
    fn pixels(&mut self, columns: &Columns) -> Option<&mut [S::Pixel]> {
        let first = columns.row * self.width + u32_to_usize(columns.column);
        self.page.get_mut(first..first + columns.count)
    }

    /// Records stores at `lo .. hi` of `columns`.
    fn stored(&mut self, columns: &Columns, (lo, hi): (usize, usize)) {
        let at = |k: usize| columns.column + u32::try_from(k).unwrap_or(u32::MAX);
        if let Some(extents) = &mut self.extents {
            extents.mark(columns.row, at(lo), at(hi));
        }
        self.stored += hi - lo;
    }
}

impl<S: Stores> RowOut for StoreOut<'_, S> {
    fn push_row(&mut self, y: u32) {
        self.row = (self.rect.y0..self.rect.y1)
            .contains(&y)
            .then(|| u32_to_usize(y - self.rect.y0));
    }

    fn put_run(&mut self, x: u32, len: u32, value: u8) {
        let color = self.color;
        let Some(columns) = self.columns(x, u32_to_usize(len)).filter(|_| value != 0) else {
            return;
        };
        if let Some(pixels) = self.pixels(&columns) {
            match color.opaque().filter(|_| value == u8::MAX) {
                Some(pixel) => pixels.fill(pixel),
                None => color.uniform(pixels, value),
            }
        }
        self.stored(&columns, (0, columns.count));
    }

    fn put_row(&mut self, x: u32, values: &[u8]) {
        let color = self.color;
        let Some(columns) = self.columns(x, values.len()) else {
            return;
        };
        let Some(values) = values.get(columns.skip..columns.skip + columns.count) else {
            return;
        };
        // Zero coverage leaves a pixel as it is: only covered pixels count.
        let covered = self
            .pixels(&columns)
            .and_then(|pixels| color.runs(pixels, values));
        if let Some(stretch) = covered {
            self.stored(&columns, stretch);
        }
    }
}

/// Stores of an opaque constant colour into an f64 page row by row, as
/// the rasterizer emits them: each covered pixel composites the colour at
/// its coverage's opacity, as a span of the mark does.
struct WideOut<'a> {
    page: &'a mut [f64],
    /// The page's stored columns; none for a group's buffer.
    extents: Option<&'a mut Extents>,
    rect: DeviceRect,
    layout: Layout,
    color: &'a [f64],
    /// The current row within `rect`, if it lies there.
    row: Option<usize>,
    stored: usize,
}

impl WideOut<'_> {
    /// Composites `coverage` at columns `x ..` of the current row, those
    /// within `rect` and from the first to past the last covered one.
    fn store(&mut self, x: u32, len: usize, coverage: kernel::Coverage<'_>) {
        let Some(row) = self.row else {
            return;
        };
        let end = x.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
        let (start, stop) = (x.max(self.rect.x0), end.min(self.rect.x1));
        if start >= stop {
            return;
        }
        let (mut column, mut count) = (start - self.rect.x0, u32_to_usize(stop - start));
        let coverage = match coverage {
            kernel::Coverage::Uniform(c) => kernel::Coverage::Uniform(c),
            kernel::Coverage::Bytes(bytes) => {
                // Zero coverage leaves a pixel as it is.
                let skip = u32_to_usize(start - x);
                let Some(bytes) = bytes.get(skip..skip + count) else {
                    return;
                };
                let Some(lo) = bytes.iter().position(|&c| c != 0) else {
                    return;
                };
                let hi = bytes.iter().rposition(|&c| c != 0).unwrap_or(lo) + 1;
                column += u32::try_from(lo).unwrap_or(u32::MAX);
                count = hi - lo;
                kernel::Coverage::Bytes(&bytes[lo..hi])
            }
        };
        let stride = self.layout.stride();
        let pixel = row * u32_to_usize(self.rect.width()) + u32_to_usize(column);
        if let Some(pixels) = self.page.get_mut(pixel * stride..(pixel + count) * stride) {
            kernel::cover(pixels, self.color, self.layout, coverage);
        }
        let at = |k: usize| column + u32::try_from(k).unwrap_or(u32::MAX);
        if let Some(extents) = &mut self.extents {
            extents.mark(row, at(0), at(count));
        }
        self.stored += count;
    }
}

impl RowOut for WideOut<'_> {
    fn push_row(&mut self, y: u32) {
        self.row = (self.rect.y0..self.rect.y1)
            .contains(&y)
            .then(|| u32_to_usize(y - self.rect.y0));
    }

    fn put_run(&mut self, x: u32, len: u32, value: u8) {
        if value != 0 {
            self.store(x, u32_to_usize(len), kernel::Coverage::Uniform(value));
        }
    }

    fn put_row(&mut self, x: u32, values: &[u8]) {
        self.store(x, values.len(), kernel::Coverage::Bytes(values));
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
            let factor = kernel::UNIT[usize::from(c)];
            *a *= factor;
            *s *= factor;
        }
    }
}

/// The source alpha and shape of a span of `cover`: the paint's own over
/// a whole span at full opacity without a soft mask, as they are; else
/// formed in `alpha` and `shape` by [`opacity`] and [`apply`].
fn source_opacity<'a>(
    (paint, paint_shape): (Option<&'a [f64]>, Option<&'a [f64]>),
    (cover, compositing, mask): (Cover<'_>, &Compositing, Option<&[f64]>),
    (alpha, shape): (&'a mut [f64], &'a mut [f64]),
) -> (&'a [f64], &'a [f64]) {
    if let Some(own) = paint
        && cover.coverage().is_none()
        && compositing.alpha == 1.0
        && mask.is_none()
    {
        return (own, paint_shape.unwrap_or(own));
    }
    opacity((paint, paint_shape), cover.coverage(), alpha, shape);
    apply(compositing, mask, alpha, shape);
    (alpha, shape)
}

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
    pub(crate) page: &'a mut PageBand,
    pub(crate) wide: &'a mut [f64],
    pub(crate) levels: &'a mut [[f32; 4]],
    pub(crate) starts: &'a mut [[u32; 2]],
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
    let colors = &sources.colors;
    match frame.buffer {
        Buffer::Wide(offset) => {
            let (below, buffer) = target.wide.split_at_mut(offset);
            let read = Read {
                page: target.page.pixels(),
                wide: below,
                levels: &*target.levels,
            };
            // Knockout members composite over the initial backdrop: the
            // backdrop buffer's own pixels where they are f64 pixels of these
            // channels, else read.
            let shared = mode.knockout.then(|| {
                let at = (x, y);
                frame
                    .initial
                    .pixels(read.page, below, at, frame.layout, len)
            });
            let initial = match shared {
                Some(Some(pixels)) => pixels,
                Some(None) => {
                    let Some(initial) = target.initial.get_mut(..len * stride) else {
                        return;
                    };
                    frame.initial.row(read, (x, y), frame.layout, initial);
                    &*initial
                }
                None => &[],
            };
            if let Some(offset) = frame.started {
                let rows = (&mut *target.starts, offset);
                start_columns((buffer, rows), read, frame, (x, y), len);
            }
            let begin = start * stride;
            if let Some(pixels) = buffer.get_mut(begin..begin + len * stride) {
                kernel::composite(pixels, sources, initial, mode);
            }
        }
        Buffer::Levels(offset) => {
            if let Some(pixels) = target.levels.get_mut(offset + start..offset + start + len) {
                levels(pixels, colors, sources.alpha, kind);
            }
        }
        Buffer::Page => {
            let initial = if mode.knockout {
                let Some(initial) = target.initial.get_mut(..len * stride) else {
                    return;
                };
                let read = Read {
                    page: target.page.pixels(),
                    wide: &*target.wide,
                    levels: &*target.levels,
                };
                frame.initial.row(read, (x, y), frame.layout, initial);
                &*initial
            } else {
                &[]
            };
            let row = u32_to_usize(y - frame.rect.y0);
            let start_column = x - frame.rect.x0;
            let end_column = start_column.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
            let (pixels, extents) = target.page.parts_mut();
            extents.mark(row, start_column, end_column);
            page(pixels, (start, stride), (sources, mode), initial, kind);
        }
    }
}

/// Starts the pixels `x .. x + len` of row `y` of the lazily started
/// `frame` that have not started, from its initial backdrop, as starting
/// the whole buffer would have: each row's started columns, in `rows` from
/// `offset`, stay one interval. `buffer` is the wide stack from the frame's
/// storage and `read` what lies below it.
fn start_columns(
    (buffer, (rows, offset)): (&mut [f64], (&mut [[u32; 2]], usize)),
    read: Read<'_>,
    frame: Frame,
    (x, y): (u32, u32),
    len: usize,
) {
    let row = u32_to_usize(y - frame.rect.y0);
    let Some(slot) = rows.get_mut(offset + row) else {
        return;
    };
    let begin = x - frame.rect.x0;
    let end = begin.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
    let [lo, hi] = match *slot {
        [lo, hi] if lo < hi => [lo, hi],
        _ => [begin, begin],
    };
    *slot = [lo.min(begin), hi.max(end)];
    let (n, stride) = (frame.layout.channels(), frame.layout.stride());
    let first = row * u32_to_usize(frame.rect.width());
    for (from, to) in [(begin, lo), (hi, end)] {
        let pixels = (first + u32_to_usize(from)) * stride..(first + u32_to_usize(to)) * stride;
        let Some(out) = buffer.get_mut(pixels).filter(|out| !out.is_empty()) else {
            continue;
        };
        let at = (frame.rect.x0 + from, y);
        let count = u32_to_usize(to - from);
        match frame
            .initial
            .pixels(read.page, read.wide, at, frame.layout, count)
        {
            Some(backdrop) => {
                out.copy_from_slice(backdrop);
                for pixel in out.chunks_exact_mut(stride) {
                    pixel[n + 1] = 0.0;
                    pixel[n + 2] = 0.0;
                }
            }
            None => frame.initial.row(read, at, frame.layout, out),
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
        PageBuffer::F32Cmyk(values) => {
            if let Some(pixels) = values.get_mut(start..start + len) {
                levels(pixels, colors, sources.alpha, kind);
            }
        }
    }
}

fn levels<const N: usize>(
    pixels: &mut [[f32; N]],
    colors: &Colors<'_>,
    alpha: &[f64],
    kind: Store<'_>,
) {
    match kind {
        Store::Mark(_) => kernel::store_f32(pixels, colors, alpha),
        Store::Result => kernel::dynamic_f32(pixels, colors, alpha),
    }
}
