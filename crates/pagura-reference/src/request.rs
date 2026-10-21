//! What a reference rendering asks for, and its result.

use crate::area::{index, pixels};
use pagura_raster::{DeviceRect, Rgba8, Transform, quantize_unit};

/// Whether overprinting is simulated (8.6.7, 11.7.4), resolved from the
/// caller's preview policy and the document's print intent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Overprint {
    /// Overprint flags are ignored.
    #[default]
    Ignore,
    /// Overprinting is simulated, with spot colorants as channels.
    Simulate,
}

/// One reference rendering of a scene.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Request {
    /// Default user space to device pixels (`PageSpace::device_transform`).
    pub transform: Transform,
    /// The pixels to evaluate. Every pixel is evaluated on its own, so the
    /// result for a pixel does not depend on the region.
    pub region: DeviceRect,
    /// The colour the page is composited onto.
    pub background: Rgba8,
    /// Overprint simulation.
    pub overprint: Overprint,
}

impl Request {
    /// `region` under `transform`, on white, without overprint simulation.
    pub fn new(transform: Transform, region: DeviceRect) -> Self {
        Self {
            transform,
            region,
            background: Rgba8::WHITE,
            overprint: Overprint::Ignore,
        }
    }
}

/// The exact result of a rendering: per pixel the premultiplied RGBA value
/// in `0..=1` before the one output quantization, row by row over the
/// region.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendering {
    region: DeviceRect,
    pixels: Vec<[f64; 4]>,
}

impl Rendering {
    pub(crate) fn new(region: DeviceRect, pixels: Vec<[f64; 4]>) -> Self {
        debug_assert_eq!(pixels.len(), crate::area::pixels(region));
        Self { region, pixels }
    }

    /// The rendered region.
    pub fn region(&self) -> DeviceRect {
        self.region
    }

    /// The exact premultiplied RGBA value of device pixel `(x, y)`; `None`
    /// outside the region.
    pub fn exact(&self, x: u32, y: u32) -> Option<[f64; 4]> {
        index(self.region, x, y)
            .and_then(|i| self.pixels.get(i))
            .copied()
    }

    /// The exact values, row by row.
    pub fn values(&self) -> &[[f64; 4]] {
        &self.pixels
    }

    /// The correctly rounded premultiplied RGBA8 bytes, quantized once with
    /// [`quantize_unit`], row by row.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(pixels(self.region).saturating_mul(4));
        for pixel in &self.pixels {
            bytes.extend(pixel.map(quantize_unit));
        }
        bytes
    }
}
