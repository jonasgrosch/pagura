//! Shading sources (8.7.4): axial, radial, function-based and mesh
//! shadings evaluated per device pixel with a prepared function and
//! converter, with anti-aliasing semantics as ADR 0004 defines them.

use super::{PaintContext, SpanColors};
use pagura_core::{Error, Meter, Result};
use pagura_interpret::Shading;
use pagura_raster::Transform;

/// A shading prepared for one render.
#[derive(Debug)]
pub struct ShadingSource {
    _private: (),
}

impl ShadingSource {
    /// Prepares `shading` under `shading_to_device` and reserves the
    /// `function_work` of filling `pixels` device pixels.
    ///
    /// # Errors
    ///
    /// As [`super::Source::prepare`].
    pub fn prepare(
        shading: &Shading,
        shading_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        functions: &mut Meter,
    ) -> Result<Self> {
        let _ = (shading, shading_to_device, context, pixels, functions);
        Err(Error::invalid_argument("shading sources are not implemented"))
    }

    /// Fills `out` with the colours of device pixels `x .. x + len` of row
    /// `y`.
    pub fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
        let _ = (x, y, len);
        out.colors.clear();
        out.alpha.clear();
    }
}
