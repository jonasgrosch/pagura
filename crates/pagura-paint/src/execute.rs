//! The band executor (ADR 0034 §4): bands of the global grid top to bottom;
//! in each band the page group's active items in source order, groups and
//! soft masks in band-local buffers on stacks, every output pixel quantized
//! once. Execution reads only what preparation prepared and reserved, so it
//! cannot fail.

mod frame;
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
pub(crate) use workspace::{MaskCache, PageBuffer, Sizes, Usage, Workspace};

use crate::kernel::{self, Colors, Mode, Sources};
use crate::paint::PaintSpans;
use crate::plan::{Compositing, Item, ItemKind, Mark, Plan, area};
use crate::prepare::Producer;
use pagura_core::convert::u32_to_usize;
use pagura_raster::{DeviceRect, Pixmap, Span};
use std::ops::Range;

#[cfg(test)]
mod tests;

/// One band's execution.
pub(crate) struct Executor<'a> {
    plan: &'a Plan,
    ws: &'a mut Workspace,
    producer: &'a mut dyn Producer,
    band: u32,
    rows: Range<u32>,
    first: u32,
}

impl<'a> Executor<'a> {
    /// Executes `band` of `plan` and writes its rows to `target`.
    pub(crate) fn band(
        plan: &'a Plan,
        ws: &'a mut Workspace,
        producer: &'a mut dyn Producer,
        band: u32,
        target: &mut Pixmap,
    ) {
        let rows = plan.area.band_rows(band);
        if rows.is_empty() {
            return;
        }
        let first = plan.area.bands().start;
        let mut executor = Executor {
            plan,
            ws,
            producer,
            band,
            rows,
            first,
        };
        executor.page();
        output::write(executor.plan, executor.ws, &executor.rows, target);
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
        self.ws
            .page
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
            if !self.visible(item, frame) {
                continue;
            }
            match &item.kind {
                ItemKind::Mark(mark) => self.mark(item, mark, frame, tops),
                ItemKind::Group(used) => self.group(item, used, frame, tops),
            }
        }
    }

    /// The hook of exact occlusion (ADR 0034 §4): whether `item` can still
    /// change `frame` in this band. A span fully covered by a later opaque
    /// Normal mark of the same group, with no mask, transfer or other blend
    /// between, may be skipped for earlier items; this pass skips nothing.
    #[expect(
        clippy::unused_self,
        reason = "the occlusion state will live in the executor"
    )]
    fn visible(&self, item: &Item, frame: Frame) -> bool {
        let _ = (item, frame);
        true
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
        if let Some(boundary) = mark.boundary {
            self.producer
                .rasterize(Some(boundary), None, self.band, &mut self.ws.boundary.0);
        }
        let mut coverage = std::mem::take(&mut self.ws.coverage);
        self.producer.rasterize(
            mark.cover.shape,
            mark.cover.clip,
            self.band,
            &mut coverage,
        );
        for row in coverage.rows() {
            if row.y < frame.rect.y0 || row.y >= frame.rect.y1 {
                continue;
            }
            for span in row.spans() {
                if let Some((x, cover)) = clamp(span, frame.rect) {
                    self.mark_span(mark, frame, (x, row.y), cover);
                }
            }
        }
        self.ws.coverage = coverage;
    }

    fn mark_span(&mut self, mark: &Mark, frame: Frame, (x, y): (u32, u32), cover: Cover<'_>) {
        let plan = self.plan;
        let Some(paint) = plan.paints.get(u32_to_usize(mark.paint)) else {
            return;
        };
        let paint = paint.as_ref();
        let len = cover.len();
        let first_row = self.rows.start;
        let ws = &mut *self.ws;
        let compositing = &mark.compositing;
        let mask = match compositing.mask {
            Some(m) => mask::row(plan, &ws.masks, &mut ws.mask, m, (x, y), len),
            None => None,
        };
        let constant = paint.constant();
        if constant.is_none() {
            match mark.boundary {
                Some(_) => {
                    let (boundary, dense) = &mut ws.boundary;
                    let Some(dense) = dense.get_mut(..len) else {
                        return;
                    };
                    densify(boundary, (x, y), dense);
                    paint.span_with_boundary(x, y, dense, &mut ws.colors);
                }
                None if paint.shape_is_alpha() => paint.span(x, y, len, &mut ws.colors),
                None => paint.span_with_shape(x, y, len, &mut ws.colors, &mut ws.texel_shape),
            }
        }
        let paint_shape = (constant.is_none() && !paint.shape_is_alpha())
            .then(|| ws.texel_shape.get(..len))
            .flatten();
        let (colors, paint_alpha, painted) = match constant {
            Some(color) => (Colors::Constant(color), None, paint.painted()),
            None => {
                let alpha = if ws.colors.alpha.is_empty() {
                    None
                } else if let Some(alpha) = ws.colors.alpha.get(..len) {
                    Some(alpha)
                } else {
                    debug_assert!(false, "a paint source filled a short alpha span");
                    return;
                };
                let colors = Colors::Strided {
                    values: &ws.colors.colors,
                    stride: ws.colors.channels,
                };
                (colors, alpha, ws.colors.painted)
            }
        };
        if !colors.holds(len, frame.layout.channels()) {
            debug_assert!(false, "a paint source filled a malformed span");
            return;
        }
        let (Some(alpha), Some(shape)) = (ws.alpha.get_mut(..len), ws.shape.get_mut(..len)) else {
            return;
        };
        opacity((paint_alpha, paint_shape), cover.coverage(), alpha, shape);
        apply(compositing, mask, alpha, shape);
        if mark.records && !ws.selection.is_empty() {
            output::record(plan, &mut ws.selection, (x, y, first_row), cover, mark.select);
        }
        ws.usage.work = ws.usage.work.saturating_add(len);
        let preserve = compositing.overprint & !painted.0 & frame.layout.mask();
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
        let kind = Store::Mark(n16_words(paint, compositing, cover));
        store(
            Target {
                page: &mut ws.page,
                wide: &mut ws.wide,
                levels: &mut ws.levels,
                bytes: &mut ws.bytes,
                initial: &mut ws.initial,
            },
            frame,
            (x, y),
            (&sources, &mode),
            kind,
        );
    }
}

/// The coverage of `coverage` at the pixels from `(x, y)`, one byte each.
fn densify(coverage: &pagura_raster::BandCoverage, (x, y): (u32, u32), out: &mut [u8]) {
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

/// The source words and coverage of an opaque constant store into N16.
fn n16_words<'c>(
    paint: &dyn PaintSpans,
    compositing: &Compositing,
    cover: Cover<'c>,
) -> Option<([u16; 4], Option<&'c [u8]>)> {
    let color = paint.constant()?;
    if !compositing.is_opaque() || color.len() < 3 {
        return None;
    }
    let words = [
        kernel::word(color[0]),
        kernel::word(color[1]),
        kernel::word(color[2]),
        u16::MAX,
    ];
    Some((words, cover.coverage()))
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
/// (its alpha unless given) times coverage, in the earlier compositor's
/// order.
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
            let factor = f64::from(c) / 255.0;
            *a *= factor;
            *s *= factor;
        }
    }
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
            // The earlier compositor multiplied by `constant × 1.0`, which is
            // `constant` exactly.
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
    pub(crate) wide: &'a mut [f64],
    pub(crate) levels: &'a mut [[f32; 4]],
    pub(crate) bytes: &'a mut [[u8; 4]],
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
    target: Target<'_>,
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
            page: target.page,
            wide: target.wide,
            levels: target.levels,
            bytes: target.bytes,
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
        Buffer::Bytes(offset) => {
            if let Some(pixels) = target.bytes.get_mut(offset + start..offset + start + len) {
                kernel::store_u8(pixels, colors, sources.alpha);
            }
        }
        Buffer::Page => match target.page {
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
            PageBuffer::U8(bytes) => {
                if let Some(pixels) = bytes.get_mut(start..start + len) {
                    kernel::store_u8(pixels, colors, sources.alpha);
                }
            }
        },
    }
}

fn levels(pixels: &mut [[f32; 4]], colors: &Colors<'_>, alpha: &[f64], kind: Store<'_>) {
    match kind {
        Store::Mark(_) => kernel::store_f32(pixels, colors, alpha),
        Store::Result => kernel::dynamic_f32(pixels, colors, alpha),
    }
}
