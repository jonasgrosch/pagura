//! The image sampler family (8.9.5, ADR 0004): nearest samples and
//! exact-area averages, with decode arrays, colour keys, stencil and soft
//! masks and matte, over images decoded on request at the reduction and
//! region painting needs.

use super::{PaintContext, SpanColors};
use pagura_core::{Error, Meter, Result};
use pagura_interpret::Image;
use pagura_raster::Transform;

/// An image prepared for one render: decoded (through the store) and ready
/// to sample. A stencil mask samples alpha only; the executor colours it
/// with its paint.
#[derive(Debug)]
pub struct ImageSampler {
    _private: (),
}

impl ImageSampler {
    /// Prepares `image` drawn under `unit_to_device` (the image's unit
    /// square to device pixels), decoding what the region needs.
    ///
    /// # Errors
    ///
    /// Returns the decode's errors and a `Limit` error when decoding or
    /// sampling exceeds the budgets.
    pub fn prepare(
        image: &Image,
        unit_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        work: &mut Meter,
    ) -> Result<Self> {
        let _ = (image, unit_to_device, context, pixels, work);
        Err(Error::invalid_argument("image samplers are not implemented"))
    }

    /// Fills `out` with the colours and alpha of device pixels `x .. x +
    /// len` of row `y`.
    pub fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
        let _ = (x, y, len);
        out.colors.clear();
        out.alpha.clear();
    }
}
