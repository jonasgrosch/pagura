//! A group buffer during one band, and the backdrops buffers start from.

use super::PageBuffer;
use crate::format::Format;
use crate::plan::{Layout, area};
use pagura_core::convert::u32_to_usize;
use pagura_raster::DeviceRect;

/// Where a buffer lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Buffer {
    /// The page buffer.
    Page,
    /// f64 pixels of the frame's layout on the wide stack, from this offset.
    Wide(usize),
    /// F32 RGBA levels on the level stack, from this pixel.
    Levels(usize),
}

/// The next free place of each stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Tops {
    /// f64 values.
    pub(crate) wide: usize,
    /// F32 pixels.
    pub(crate) levels: usize,
}

impl Tops {
    /// Places a buffer of `format` over `rect` on stacks of `limits`: its
    /// storage and the tops above it, or `None` where the stacks do not
    /// hold it. Preparation sizes the stacks for every nested buffer.
    pub(crate) fn place(
        self,
        (format, rect, layout): (Format, DeviceRect, Layout),
        limits: Self,
    ) -> Option<(Buffer, Self)> {
        let pixels = area(rect);
        let placed = match format {
            Format::F64 => (
                Buffer::Wide(self.wide),
                Self {
                    wide: self.wide + pixels * layout.stride(),
                    ..self
                },
            ),
            Format::F32 | Format::N16 => (
                Buffer::Levels(self.levels),
                Self {
                    levels: self.levels + pixels,
                    ..self
                },
            ),
        };
        let above = placed.1;
        let fits = above.wide <= limits.wide && above.levels <= limits.levels;
        debug_assert!(fits, "the stacks hold every nested buffer");
        fits.then_some(placed)
    }
}

/// A group's buffer in the current band.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    /// The group.
    pub(crate) group: u32,
    /// Its storage.
    pub(crate) buffer: Buffer,
    /// The pixels it holds in this band.
    pub(crate) rect: DeviceRect,
    /// Its channels.
    pub(crate) layout: Layout,
    /// Its members knock each other out.
    pub(crate) knockout: bool,
    /// What it started from: the backdrop of knockout members (11.4.6) and
    /// of the backdrop removal of non-isolated results (11.4.4).
    pub(crate) initial: Backdrop,
}

impl Frame {
    /// The pixel index of `(x, y)`.
    pub(crate) fn index(&self, x: u32, y: u32) -> Option<usize> {
        let r = self.rect;
        if x < r.x0 || x >= r.x1 || y < r.y0 || y >= r.y1 {
            return None;
        }
        Some(u32_to_usize(y - r.y0) * u32_to_usize(r.width()) + u32_to_usize(x - r.x0))
    }

    /// The backdrop a non-isolated child starts from: this buffer, or for
    /// a knockout group its own initial backdrop.
    pub(crate) fn backdrop(&self) -> Backdrop {
        if self.knockout {
            self.initial
        } else {
            Backdrop::Frame {
                buffer: self.buffer,
                rect: self.rect,
                layout: self.layout,
            }
        }
    }
}

/// A group's initial backdrop.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Backdrop {
    /// Transparent.
    Transparent,
    /// An opaque colour (a luminosity mask's `BC`).
    Opaque([f64; 4]),
    /// Another buffer, unchanged while the group runs.
    Frame {
        /// Its storage.
        buffer: Buffer,
        /// The pixels it holds.
        rect: DeviceRect,
        /// Its channels.
        layout: Layout,
    },
}

/// The buffers a backdrop is read from: everything below the buffer being
/// written.
#[derive(Clone, Copy)]
pub(crate) struct Read<'a> {
    /// The page.
    pub(crate) page: &'a PageBuffer,
    /// The wide stack.
    pub(crate) wide: &'a [f64],
    /// The level stack.
    pub(crate) levels: &'a [[f32; 4]],
}

impl Backdrop {
    /// Writes the backdrop of the pixels from `(x, y)` into `out`, f64
    /// pixels of `layout` whose group alpha and shape are 0.
    pub(crate) fn row(&self, read: Read<'_>, (x, y): (u32, u32), layout: Layout, out: &mut [f64]) {
        let stride = layout.stride();
        let n = layout.channels();
        match *self {
            Self::Transparent => out.fill(0.0),
            Self::Opaque(color) => {
                for pixel in out.chunks_exact_mut(stride) {
                    pixel.fill(0.0);
                    for (value, &c) in pixel.iter_mut().zip(&color).take(n) {
                        *value = c * 1.0;
                    }
                    pixel[n] = 1.0;
                }
            }
            Self::Frame {
                buffer,
                rect,
                layout: source,
            } => {
                let frame = Frame {
                    group: 0,
                    buffer,
                    rect,
                    layout: source,
                    knockout: false,
                    initial: Self::Transparent,
                };
                for (column, pixel) in (x..).zip(out.chunks_exact_mut(stride)) {
                    pixel.fill(0.0);
                    if let Some(index) = frame.index(column, y) {
                        let (colors, alpha) = read.pixel(buffer, index, source);
                        for (value, &c) in
                            pixel.iter_mut().zip(&colors).take(n.min(source.channels()))
                        {
                            *value = c;
                        }
                        pixel[n] = alpha;
                    }
                }
            }
        }
    }
}

/// The most colour channels of a pixel.
pub(crate) const CHANNELS: usize = 4 + crate::plan::MAX_SPOTS;

impl Read<'_> {
    /// The premultiplied colour and alpha of pixel `index` of `buffer`, as
    /// unit values.
    pub(crate) fn pixel(
        &self,
        buffer: Buffer,
        index: usize,
        layout: Layout,
    ) -> ([f64; CHANNELS], f64) {
        let mut colors = [0.0; CHANNELS];
        let n = layout.channels().min(CHANNELS);
        let stride = layout.stride();
        let narrow = |values: [f64; 4], colors: &mut [f64; CHANNELS]| {
            colors[..3].copy_from_slice(&values[..3]);
            values[3]
        };
        let wide = |values: &[f64], begin: usize, colors: &mut [f64; CHANNELS]| {
            values.get(begin..begin + stride).map_or(0.0, |pixel| {
                colors[..n].copy_from_slice(&pixel[..n]);
                pixel[layout.channels()]
            })
        };
        let alpha = match (buffer, self.page) {
            (Buffer::Wide(offset), _) => wide(self.wide, offset + index * stride, &mut colors),
            (Buffer::Levels(offset), _) => self.levels.get(offset + index).map_or(0.0, |v| {
                narrow(v.map(|l| f64::from(l) / 255.0), &mut colors)
            }),
            (Buffer::Page, PageBuffer::F64(values)) => wide(values, index * stride, &mut colors),
            (Buffer::Page, PageBuffer::N16(words)) => words.get(index).map_or(0.0, |w| {
                narrow(w.map(|v| f64::from(v) / 65535.0), &mut colors)
            }),
            (Buffer::Page, PageBuffer::F32(levels)) => levels.get(index).map_or(0.0, |v| {
                narrow(v.map(|l| f64::from(l) / 255.0), &mut colors)
            }),
        };
        (colors, alpha)
    }
}
