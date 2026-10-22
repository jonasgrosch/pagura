//! Group results (11.4): a group runs in a band-local buffer started from
//! its initial backdrop; the backdrop is removed from its colour, the colour
//! is converted into the parent's blending space, and the result composites
//! into the parent with the group's shape and alpha, or for the instances of
//! a tiling pattern through the painted mark's coverage. A result reaches a
//! narrow parent as one dynamic store, rounded once (ADR 0004 §6).

use super::frame::CHANNELS;
use super::{
    Backdrop, Buffer, Cover, Executor, Frame, Read, Store, Target, Tops, Workspace, apply, clamp,
    mask, store,
};
use crate::kernel::{Colors, Mode, Sources, level};
use crate::plan::{Conversion, GroupUse, Item, Layout, Start, area, intersect};
use crate::process::{self, Separation};
use pagura_core::convert::u32_to_usize;
use pagura_raster::DeviceRect;

impl Executor<'_> {
    /// Runs `used`'s group for this band and composites its result into
    /// `parent`; its buffer goes above `tops`.
    pub(super) fn group(&mut self, item: &Item, used: &GroupUse, parent: Frame, tops: Tops) {
        let plan = self.plan;
        let Some(child) = plan.groups.get(u32_to_usize(used.group)) else {
            return;
        };
        let rect = intersect(child.bounds, parent.rect);
        if rect.is_empty() {
            return;
        }
        self.count_shape(item);
        let layout = child.layout;
        let (buffer, above) = tops.place(child.format, rect, layout);
        if !above.within(plan.stack) {
            debug_assert!(false, "the stacks hold every nested group buffer");
            return;
        }
        let initial = match child.start {
            Start::Transparent => Backdrop::Transparent,
            Start::Opaque(color) => Backdrop::Opaque(color),
            Start::Backdrop => parent.backdrop(),
        };
        self.start(initial, buffer, rect, layout);
        let frame = Frame {
            group: used.group,
            buffer,
            rect,
            layout,
            knockout: child.knockout,
            initial,
        };
        self.run(frame, above);
        if let Some(mask) = used.compositing.mask {
            self.ensure_mask(mask, above);
        }
        let width = u32_to_usize(rect.width());
        match used.cover {
            None => {
                for y in rect.y0..rect.y1 {
                    self.result(used, (frame, parent), (rect.x0, y), Cover::Full(width));
                }
            }
            Some(cover) => {
                let mut coverage = std::mem::take(&mut self.ws.coverage);
                self.shapes
                    .rasterize(self.rasterizer, cover, self.band, &mut coverage);
                for row in coverage.rows() {
                    if row.y < rect.y0 || row.y >= rect.y1 {
                        continue;
                    }
                    for span in row.spans() {
                        if let Some((x, cover)) = clamp(span, rect) {
                            self.result(used, (frame, parent), (x, row.y), cover);
                        }
                    }
                }
                self.ws.coverage = coverage;
            }
        }
    }

    /// Fills the pixels of `rect` in `buffer` with `initial`.
    pub(super) fn start(
        &mut self,
        initial: Backdrop,
        buffer: Buffer,
        rect: DeviceRect,
        layout: Layout,
    ) {
        let ws = &mut *self.ws;
        let pixels = area(rect);
        ws.usage.work = ws.usage.work.saturating_add(pixels);
        match buffer {
            Buffer::Wide(offset) => {
                let (below, above) = ws.wide.split_at_mut(offset);
                let read = Read {
                    page: &ws.page,
                    wide: below,
                    levels: &ws.levels,
                };
                let row = u32_to_usize(rect.width()) * layout.stride();
                for (y, out) in (rect.y0..rect.y1).zip(above.chunks_exact_mut(row.max(1))) {
                    initial.row(read, (rect.x0, y), layout, out);
                }
            }
            Buffer::Levels(offset) => {
                let Some(out) = ws.levels.get_mut(offset..offset + pixels) else {
                    return;
                };
                let value = narrow_start(initial, level);
                out.fill(value);
            }
            Buffer::Page => {}
        }
    }

    /// Composites the pixels of the group result from `(x, y)` that have
    /// shape and coverage, run by run.
    fn result(
        &mut self,
        used: &GroupUse,
        frames: (Frame, Frame),
        (x, y): (u32, u32),
        cover: Cover<'_>,
    ) {
        let (child, _) = frames;
        let len = cover.len();
        let mut k = 0;
        while k < len {
            if !visible(self.ws, child, (x, y), k) || cover.at(k) == 0 {
                k += 1;
                continue;
            }
            let begin = k;
            while k < len && visible(self.ws, child, (x, y), k) && cover.at(k) > 0 {
                k += 1;
            }
            let start = x + u32::try_from(begin).unwrap_or(u32::MAX);
            self.result_run(used, frames, (start, y), cover.slice(begin, k));
        }
    }

    fn result_run(
        &mut self,
        used: &GroupUse,
        (child, parent): (Frame, Frame),
        (x, y): (u32, u32),
        cover: Cover<'_>,
    ) {
        let plan = self.plan;
        let len = cover.len();
        let conversion = used
            .conversion
            .and_then(|c| plan.conversions.get(u32_to_usize(c)));
        let channels = parent.layout.channels();
        let ws = &mut *self.ws;
        let spent = sources(ws, (child, parent.layout), (x, y), cover, conversion);
        ws.usage.functions = ws.usage.functions.saturating_add(spent);
        let mask = match used.compositing.mask {
            Some(m) => mask::row(plan, &ws.masks, &mut ws.mask, m, (x, y), len),
            None => None,
        };
        let (Some(alpha), Some(shape)) = (ws.alpha.get_mut(..len), ws.shape.get_mut(..len)) else {
            return;
        };
        apply(&used.compositing, mask, alpha, shape);
        ws.usage.work = ws.usage.work.saturating_add(len.saturating_mul(2));
        let mode = Mode {
            layout: parent.layout,
            blend: used.compositing.blend,
            knockout: parent.knockout,
            preserve: 0,
        };
        let sources = Sources {
            colors: Colors::Strided {
                values: ws.source.get(..len * channels).unwrap_or_default(),
                stride: channels.max(1),
            },
            alpha: &ws.alpha[..len],
            shape: &ws.shape[..len],
        };
        store(
            &mut Target {
                page: &mut ws.page,
                extents: &mut ws.extents,
                wide: &mut ws.wide,
                levels: &mut ws.levels,
                initial: &mut ws.initial,
            },
            parent,
            (x, y),
            (&sources, &mode),
            Store::Result,
        );
    }
}

/// The RGBA a narrow buffer starts with.
fn narrow_start<T: Copy + Default>(initial: Backdrop, encode: impl Fn(f64) -> T) -> [T; 4] {
    match initial {
        Backdrop::Opaque(color) => [
            encode(color[0]),
            encode(color[1]),
            encode(color[2]),
            encode(1.0),
        ],
        Backdrop::Transparent | Backdrop::Frame { .. } => [T::default(); 4],
    }
}

/// Whether pixel `k` from `(x, y)` of a group result has shape.
fn visible(ws: &Workspace, child: Frame, (x, y): (u32, u32), k: usize) -> bool {
    let Some(index) = u32::try_from(k)
        .ok()
        .and_then(|k| child.index(x.saturating_add(k), y))
    else {
        return false;
    };
    let n = child.layout.channels();
    match child.buffer {
        Buffer::Wide(offset) => ws
            .wide
            .get(offset + index * child.layout.stride() + n + 2)
            .is_some_and(|&shape| shape > 0.0),
        Buffer::Levels(offset) => ws.levels.get(offset + index).is_some_and(|p| p[3] > 0.0),
        Buffer::Page => false,
    }
}

/// Writes the straight colours (in the parent's channels), alpha and shape
/// of a run of group-result pixels to the workspace's span scratch; returns
/// the function work of conversion.
fn sources(
    ws: &mut Workspace,
    (child, to): (Frame, Layout),
    (x, y): (u32, u32),
    cover: Cover<'_>,
    conversion: Option<&Conversion>,
) -> usize {
    let len = cover.len();
    let Some(first) = child.index(x, y) else {
        return 0;
    };
    let from = child.layout;
    let stride = from.stride();
    // The initial backdrop of a wide buffer's run, read once.
    if let Buffer::Wide(offset) = child.buffer {
        let (below, _) = ws.wide.split_at(offset);
        let read = Read {
            page: &ws.page,
            wide: below,
            levels: &ws.levels,
        };
        if let Some(initial) = ws.initial.get_mut(..len * stride) {
            child.initial.row(read, (x, y), from, initial);
        }
    }
    let separation = conversion.map(|c| Separation {
        black: c.black.as_ref(),
        removal: c.removal.as_ref(),
    });
    let mut work = 0;
    let mut straight = [0.0; CHANNELS];
    for k in 0..len {
        let Some((group, shape)) = removed(ws, child, first + k, k, &mut straight) else {
            return work;
        };
        let (Some(target), Some(alpha), Some(shape_out)) = (
            ws.source
                .get_mut(k * to.channels()..(k + 1) * to.channels()),
            ws.alpha.get_mut(k),
            ws.shape.get_mut(k),
        ) else {
            return work;
        };
        match (conversion, &separation) {
            (Some(c), Some(separation)) => {
                convert(c, (from, to), separation, &straight, target);
                work += separation.cost();
            }
            _ => {
                for (slot, &value) in target.iter_mut().zip(&straight[..from.channels()]) {
                    *slot = value;
                }
            }
        }
        // A pattern's instances composite through the painted mark's
        // coverage: Source::new(colour, group × c, shape × c).
        if let Cover::Partial(_) = cover {
            let factor = f64::from(cover.at(k)) / 255.0;
            *alpha = group * factor;
            *shape_out = shape * factor;
        } else {
            *alpha = group;
            *shape_out = shape;
        }
    }
    work
}

/// The straight colour of pixel `index` of a group result with its initial
/// backdrop removed (11.4.4), whose backdrop is pixel `k` of the
/// workspace's initial run; returns its group alpha and shape.
fn removed(
    ws: &Workspace,
    child: Frame,
    index: usize,
    k: usize,
    straight: &mut [f64; CHANNELS],
) -> Option<(f64, f64)> {
    let n = child.layout.channels();
    let stride = child.layout.stride();
    match child.buffer {
        Buffer::Wide(offset) => {
            let begin = offset + index * stride;
            let pixel = ws.wide.get(begin..begin + stride)?;
            let base = ws.initial.get(k * stride..(k + 1) * stride)?;
            let group = pixel[n + 1];
            for (i, value) in straight.iter_mut().enumerate().take(n) {
                *value = if group > 0.0 {
                    ((pixel[i] - (1.0 - group) * base[i]) / group).clamp(0.0, 1.0)
                } else {
                    0.0
                };
            }
            Some((group, pixel[n + 2]))
        }
        Buffer::Levels(offset) => {
            let pixel = ws.levels.get(offset + index)?.map(f64::from);
            Some(narrow_straight(pixel.map(|v| v / 255.0), straight))
        }
        Buffer::Page => None,
    }
}

/// A group colour converted into its parent's blending space (11.7.2): the
/// process components through RGB, spot channels as they are.
fn convert(
    conversion: &Conversion,
    (from, to): (Layout, Layout),
    separation: &Separation<'_>,
    straight: &[f64; CHANNELS],
    target: &mut [f64],
) {
    let process = conversion.from.components();
    let rgb = process::rgb(conversion.from, &straight[..process]);
    process::encode_rgb(conversion.to, rgb, separation, target);
    let spots = from.spots.min(to.spots);
    for (slot, &value) in target
        .iter_mut()
        .skip(conversion.to.components())
        .zip(&straight[process..process + spots])
    {
        *slot = value;
    }
}

/// The straight RGB of a narrow premultiplied pixel, and its alpha as both
/// group alpha and shape (a narrow buffer starts transparent and stores
/// source-over only, so its alpha is its group alpha).
fn narrow_straight(pixel: [f64; 4], straight: &mut [f64; CHANNELS]) -> (f64, f64) {
    let alpha = pixel[3];
    for (value, &c) in straight.iter_mut().zip(&pixel[..3]) {
        *value = if alpha > 0.0 {
            (c / alpha).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    (alpha, alpha)
}
