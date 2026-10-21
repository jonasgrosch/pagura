//! Compositing buffers over a rectangle of device pixels, and the initial
//! backdrops groups start from.

use crate::area::{index, pixels};
use crate::pixel::Pixel;
use pagura_core::{Error, Result};
use pagura_raster::DeviceRect;

/// Pixels of one group, soft mask or tiling instance set, in f64. Each pixel
/// stores its channels, then alpha, group alpha and shape.
#[derive(Debug, Clone)]
pub(crate) struct Buffer {
    rect: DeviceRect,
    channels: usize,
    data: Vec<f64>,
}

impl Buffer {
    /// A buffer over `rect` holding `initial`'s pixels with zero group
    /// alpha and shape (11.4.3).
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when the buffer exceeds `max_pixels`.
    pub(crate) fn new(
        rect: DeviceRect,
        channels: usize,
        initial: &Initial<'_>,
        max_pixels: usize,
    ) -> Result<Self> {
        let count = pixels(rect);
        if count > max_pixels {
            return Err(Error::limit("reference buffer exceeds the pixel budget"));
        }
        let stride = channels + 3;
        let mut buffer = Self {
            rect,
            channels,
            data: vec![0.0; count.saturating_mul(stride)],
        };
        if !matches!(initial, Initial::Transparent) {
            for y in rect.y0..rect.y1 {
                for x in rect.x0..rect.x1 {
                    buffer.set(x, y, &initial.pixel(x, y));
                }
            }
        }
        Ok(buffer)
    }

    /// The pixels it covers.
    pub(crate) fn rect(&self) -> DeviceRect {
        self.rect
    }

    /// Pixel `(x, y)`; transparent outside the buffer.
    pub(crate) fn get(&self, x: u32, y: u32) -> Pixel {
        let mut pixel = Pixel::TRANSPARENT;
        if let Some(values) = self.slot(x, y).and_then(|range| self.data.get(range)) {
            let n = self.channels;
            pixel.color[..n].copy_from_slice(&values[..n]);
            pixel.alpha = values[n];
            pixel.group = values[n + 1];
            pixel.shape = values[n + 2];
        }
        pixel
    }

    /// Stores pixel `(x, y)`; ignored outside the buffer.
    pub(crate) fn set(&mut self, x: u32, y: u32, pixel: &Pixel) {
        let n = self.channels;
        if let Some(values) = self.slot(x, y).and_then(|range| self.data.get_mut(range)) {
            values[..n].copy_from_slice(&pixel.color[..n]);
            values[n] = pixel.alpha;
            values[n + 1] = pixel.group;
            values[n + 2] = pixel.shape;
        }
    }

    fn slot(&self, x: u32, y: u32) -> Option<std::ops::Range<usize>> {
        let stride = self.channels + 3;
        let start = index(self.rect, x, y)?.checked_mul(stride)?;
        Some(start..start.checked_add(stride)?)
    }
}

/// The backdrop a group starts from (11.4.3).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Initial<'a> {
    /// Transparent: isolated groups.
    Transparent,
    /// An opaque colour in compositing form: luminosity masks over their
    /// backdrop colour.
    Color(&'a [f64]),
    /// The backdrop of a non-isolated group.
    Buffer(&'a Buffer),
}

impl Initial<'_> {
    /// The backdrop at `(x, y)`, with zero group alpha and shape.
    pub(crate) fn pixel(&self, x: u32, y: u32) -> Pixel {
        let mut pixel = match self {
            Self::Transparent => Pixel::TRANSPARENT,
            Self::Color(color) => Pixel::opaque(color),
            Self::Buffer(buffer) => buffer.get(x, y),
        };
        pixel.group = 0.0;
        pixel.shape = 0.0;
        pixel
    }
}
