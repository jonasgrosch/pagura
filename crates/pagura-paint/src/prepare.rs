//! Preparation, the only fallible step of rendering, and execution.

use crate::request::RenderRequest;
use pagura_core::{Error, Limits, Result, Store};
use pagura_interpret::Scene;
use pagura_raster::{DeviceRect, Pixmap};

/// A scene prepared for one request: every mark that meets the region
/// culled, validated, reserved and binned to bands, with its coverage
/// source and prepared paint, and every buffer's storage format chosen.
#[derive(Debug)]
pub struct PreparedScene {
    region: DeviceRect,
    band_height: u32,
}

/// How far a render got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// Every band was rendered.
    Complete,
    /// Rendering stopped between bands; rows from `region.y0` up to this
    /// row are rendered.
    Stopped {
        /// The first row not rendered.
        row: u32,
    },
}

/// Prepares `scene` for `request`, using `store` for decoded images, glyph
/// edge setups, prepared functions and colour transforms.
///
/// # Errors
///
/// Returns the first error in source order among the marks that meet the
/// region: `Syntax` and `Unsupported` for content that cannot be painted,
/// and `Limit` errors for `render_bytes`, `function_work`, render work and
/// the per-fill budgets. Errors do not depend on band height or on how a
/// page is partitioned into regions.
pub fn prepare(
    scene: &Scene,
    request: &RenderRequest,
    store: &Store,
    limits: &Limits,
) -> Result<PreparedScene> {
    let _ = (scene, store, limits);
    if request.band_height == 0 || request.region.is_empty() {
        return Err(Error::invalid_argument("empty region or band"));
    }
    Err(Error::invalid_argument("preparation is not implemented"))
}

impl PreparedScene {
    /// The rendered region.
    pub fn region(&self) -> DeviceRect {
        self.region
    }

    /// Renders every band into `target`, which covers the region.
    pub fn render(&mut self, target: &mut Pixmap) {
        let _ = self.render_until(target, |_| false);
    }

    /// Renders bands into `target` top to bottom, asking `stop` with the
    /// next band's first row before each band.
    pub fn render_until(
        &mut self,
        target: &mut Pixmap,
        mut stop: impl FnMut(u32) -> bool,
    ) -> Completion {
        let _ = (target, self.band_height);
        if stop(self.region.y0) {
            return Completion::Stopped {
                row: self.region.y0,
            };
        }
        Completion::Complete
    }
}
