//! Group results (11.4): a group runs in a band-local buffer started from
//! its initial backdrop; the backdrop is removed from its colour, the colour
//! is converted into the parent's blending space, and the result composites
//! into the parent with the group's shape and alpha, or for the instances of
//! a tiling pattern through the painted mark's coverage. A result reaches a
//! narrow parent as one dynamic store, rounded once (ADR 0004 §6).

use super::frame::CHANNELS;
use super::workspace::EMPTY;
use super::{
    Backdrop, Buffer, Cover, Executor, Frame, Read, Store, Target, Tops, Workspace, apply,
    each_span, mask, store,
};
use crate::kernel::{Colors, Mode, Sources, level};
use crate::plan::{Conversion, GroupUse, Item, Layout, Start, area, intersect};
use crate::process;
use crate::source::copy_values;
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
        let Some((buffer, above)) = tops.place((child.format, layout), rect, plan.stack) else {
            return;
        };
        let initial = match child.start {
            Start::Transparent => Backdrop::Transparent,
            Start::Opaque(color) => Backdrop::Opaque(color),
            Start::Backdrop => parent.backdrop(),
        };
        // A knockout group's buffer is read only by its own result, never as
        // a backdrop: it starts where its stores reach.
        let lazy = (child.knockout && matches!(buffer, Buffer::Wide(_)))
            .then(|| above.rows(rect, plan.stack))
            .flatten();
        let (started, above) = if let Some((offset, above)) = lazy {
            self.start_lazily(offset, rect);
            (Some(offset), above)
        } else {
            self.start(initial, buffer, rect, layout);
            (None, above)
        };
        let frame = Frame {
            group: used.group,
            buffer,
            rect,
            layout,
            knockout: child.knockout,
            initial,
            started,
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
                each_span(&coverage, rect, |at, cover| {
                    self.result(used, (frame, parent), at, cover);
                });
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
                    page: ws.page.pixels(),
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

    /// Starts the rows of a lazily started buffer over `rect` with no
    /// started columns, its rows' intervals at `offset`.
    fn start_lazily(&mut self, offset: usize, rect: DeviceRect) {
        let ws = &mut *self.ws;
        ws.usage.work = ws.usage.work.saturating_add(area(rect));
        let rows = u32_to_usize(rect.height());
        if let Some(intervals) = ws.starts.get_mut(offset..offset + rows) {
            intervals.fill(EMPTY);
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
        let (mut k, len) = started_pixels(self.ws, child, (x, y), cover.len());
        while let Some((begin, end)) = next_run(self.ws, child, (x, y), cover, k..len) {
            let start = x + u32::try_from(begin).unwrap_or(u32::MAX);
            self.result_run(used, frames, (start, y), cover.slice(begin, end));
            k = end;
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
        let mut target = Target {
            page: &mut ws.page,
            wide: &mut ws.wide,
            levels: &mut ws.levels,
            starts: &mut ws.starts,
            initial: &mut ws.initial,
        };
        if mode.blend != pagura_interpret::BlendMode::Normal
            && super::store_blended_result(&mut target, parent, (x, y), &sources, mode.blend)
        {
            return;
        }
        store(
            &mut target,
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

/// The started pixels `begin .. end` among the `len` from `(x, y)` of a
/// group result: all of them unless its buffer started lazily, whose other
/// pixels hold neither shape nor storage.
fn started_pixels(ws: &Workspace, child: Frame, (x, y): (u32, u32), len: usize) -> (usize, usize) {
    let Some(offset) = child.started else {
        return (0, len);
    };
    let interval = y
        .checked_sub(child.rect.y0)
        .and_then(|row| ws.starts.get(offset + u32_to_usize(row)));
    let Some(&[lo, hi]) = interval.filter(|[lo, hi]| lo < hi) else {
        return (0, 0);
    };
    let column = u32_to_usize(x.saturating_sub(child.rect.x0));
    let (lo, hi) = (u32_to_usize(lo), u32_to_usize(hi));
    let begin = lo.saturating_sub(column).min(len);
    let end = hi.saturating_sub(column).min(len);
    (begin, end.max(begin))
}

/// The next run `begin .. end` within `pixels` of the group result from
/// `(x, y)` whose pixels have shape and coverage.
fn next_run(
    ws: &Workspace,
    child: Frame,
    (x, y): (u32, u32),
    cover: Cover<'_>,
    pixels: std::ops::Range<usize>,
) -> Option<(usize, usize)> {
    let first = child.index(x, y)?;
    let len = pixels.end.min(u32_to_usize(child.rect.x1 - x));
    let pixels = pixels.start..len;
    match child.buffer {
        Buffer::Wide(offset) => {
            let (n, stride) = (child.layout.channels(), child.layout.stride());
            let begin = offset + first * stride;
            let values = ws.wide.get(begin..begin + len * stride)?;
            run(pixels, |k| {
                values[k * stride + n + 2] > 0.0 && cover.at(k) > 0
            })
        }
        Buffer::Levels(offset) => {
            let values = ws.levels.get(offset + first..offset + first + len)?;
            run(pixels, |k| values[k][3] > 0.0 && cover.at(k) > 0)
        }
        Buffer::Page => None,
    }
}

/// The first run `begin .. end` of `pixels` that are all `visible`.
fn run(pixels: std::ops::Range<usize>, visible: impl Fn(usize) -> bool) -> Option<(usize, usize)> {
    let end = pixels.end;
    let begin = pixels.clone().find(|&k| visible(k))?;
    let stop = (begin..end).find(|&k| !visible(k)).unwrap_or(end);
    Some((begin, stop))
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
    let (n, stride) = (from.channels(), from.stride());
    let Workspace {
        page,
        wide,
        levels,
        initial,
        source,
        alpha,
        shape,
        ..
    } = ws;
    let (Some(colors), Some(alpha), Some(shape)) = (
        source.get_mut(..len * to.channels()),
        alpha.get_mut(..len),
        shape.get_mut(..len),
    ) else {
        return 0;
    };
    let mut work = 0;
    let mut straight = [0.0; CHANNELS];
    match child.buffer {
        Buffer::Wide(offset) => {
            let (below, buffer) = wide.split_at(offset);
            let Some(pixels) = buffer.get(first * stride..(first + len) * stride) else {
                return 0;
            };
            // The run's initial backdrop: none where it is transparent, whose
            // removal takes zero for it; the backdrop buffer's own pixels
            // where they are f64 pixels of these channels; else read once.
            let bases = match child.initial {
                Backdrop::Transparent => None,
                backdrop => {
                    if let Some(pixels) = backdrop.pixels(page.pixels(), below, (x, y), from, len) {
                        Some(pixels)
                    } else {
                        let Some(out) = initial.get_mut(..len * stride) else {
                            return 0;
                        };
                        let read = Read {
                            page: page.pixels(),
                            wide: below,
                            levels,
                        };
                        backdrop.row(read, (x, y), from, out);
                        Some(&*out)
                    }
                }
            };
            if conversion.is_none() && n == to.channels() && matches!(n, 1 | 3 | 4) {
                let run = (pixels, bases, cover);
                match n {
                    1 => removed_run::<1>(run, colors, alpha, shape),
                    3 => removed_run::<3>(run, colors, alpha, shape),
                    _ => removed_run::<4>(run, colors, alpha, shape),
                }
                return 0;
            }
            for (k, pixel) in pixels.chunks_exact(stride).enumerate() {
                let base = bases.map(|bases| &bases[k * stride..k * stride + n]);
                let removed = removed(pixel, base, n, &mut straight);
                let target = &mut colors[k * to.channels()..(k + 1) * to.channels()];
                work += emit(conversion, (from, to), &straight, target);
                (alpha[k], shape[k]) = covered(cover, k, removed);
            }
        }
        Buffer::Levels(offset) => {
            let Some(pixels) = levels.get(offset + first..offset + first + len) else {
                return 0;
            };
            // Without conversion, straight colours go to their slots directly.
            if conversion.is_none() && to.channels() == 3 {
                let colors = colors.as_chunks_mut::<3>().0;
                let targets = colors.iter_mut().zip(alpha.iter_mut().zip(shape));
                for (k, (pixel, (color, (alpha, shape)))) in pixels.iter().zip(targets).enumerate()
                {
                    let pixel = pixel.map(f64::from).map(|v| v / 255.0);
                    let removed = narrow_straight(pixel, color);
                    (*alpha, *shape) = covered(cover, k, removed);
                }
                return 0;
            }
            for (k, pixel) in pixels.iter().enumerate() {
                let pixel = pixel.map(f64::from).map(|v| v / 255.0);
                let removed = narrow_straight(pixel, &mut straight);
                let target = &mut colors[k * to.channels()..(k + 1) * to.channels()];
                work += emit(conversion, (from, to), &straight, target);
                (alpha[k], shape[k]) = covered(cover, k, removed);
            }
        }
        Buffer::Page => {}
    }
    work
}

/// Writes a straight group colour to `target` in the parent's channels,
/// converted where the spaces differ; returns the function work.
fn emit(
    conversion: Option<&Conversion>,
    (from, to): (Layout, Layout),
    straight: &[f64; CHANNELS],
    target: &mut [f64],
) -> usize {
    if let Some(c) = conversion {
        convert(c, (from, to), straight, target);
        c.encoder.cost()
    } else {
        copy_values(target, &straight[..from.channels()]);
        0
    }
}

/// The alpha and shape of pixel `k` of a run with group alpha and shape
/// `(group, shape)`: a pattern's instances composite through the painted
/// mark's coverage, Source::new(colour, group × c, shape × c).
fn covered(cover: Cover<'_>, k: usize, (group, shape): (f64, f64)) -> (f64, f64) {
    if let Cover::Partial(_) = cover {
        let factor = f64::from(cover.at(k)) / 255.0;
        (group * factor, shape * factor)
    } else {
        (group, shape)
    }
}

/// The straight colour of a group result's wide `pixel` of `n` channels
/// with its initial backdrop `base` removed (11.4.4), zero without one;
/// returns its group alpha and shape.
fn removed(pixel: &[f64], base: Option<&[f64]>, n: usize, straight: &mut [f64]) -> (f64, f64) {
    let group = pixel[n + 1];
    for (i, value) in straight.iter_mut().enumerate().take(n) {
        let base = base.map_or(0.0, |base| base[i]);
        *value = if group > 0.0 {
            ((pixel[i] - (1.0 - group) * base) / group).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    (group, pixel[n + 2])
}

/// [`sources`] of a run of `N`-channel wide `pixels` over their initial
/// `bases` without conversion: [`removed`] of each, in one pass over fixed
/// counts.
fn removed_run<const N: usize>(
    (pixels, bases, cover): (&[f64], Option<&[f64]>, Cover<'_>),
    colors: &mut [f64],
    alpha: &mut [f64],
    shape: &mut [f64],
) {
    let stride = N + 3;
    let targets = colors
        .as_chunks_mut::<N>()
        .0
        .iter_mut()
        .zip(alpha.iter_mut().zip(shape));
    for (k, (pixel, (colors, (alpha, shape)))) in
        pixels.chunks_exact(stride).zip(targets).enumerate()
    {
        let base = bases.map(|bases| &bases[k * stride..k * stride + N]);
        let removed = removed(pixel, base, N, colors);
        (*alpha, *shape) = covered(cover, k, removed);
    }
}

/// A group colour converted into its parent's blending space (11.7.2): the
/// process components through RGB, spot channels as they are.
fn convert(
    conversion: &Conversion,
    (from, to): (Layout, Layout),
    straight: &[f64; CHANNELS],
    target: &mut [f64],
) {
    let process = conversion.from.components();
    let rgb = process::rgb(conversion.from, &straight[..process]);
    let channels = conversion.to.components().min(target.len());
    conversion.encoder.encode(rgb, &mut target[..channels]);
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
fn narrow_straight(pixel: [f64; 4], straight: &mut [f64]) -> (f64, f64) {
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
