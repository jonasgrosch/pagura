//! Transfer functions (10.5) selected per output pixel (11.7.5.2).
//!
//! The last element whose shape fully covers a pixel selects the transfer:
//! its own when it and every enclosing group are opaque Normal marks
//! without a soft mask, the identity otherwise. Group results and soft-mask
//! content select nothing. A partially covering element where a
//! non-identity transfer is involved has no defined result here: averaging
//! colours before a nonlinear transfer loses their correlation with
//! regions, so the reference declines such pages, as the renderer does.

use crate::area::{index, pixels};
use pagura_core::{Error, Result};
use pagura_interpret::{Composite, Scene, Transfer};
use pagura_raster::DeviceRect;
use std::sync::Arc;

/// The transfer each output pixel selects.
#[derive(Debug)]
pub(crate) struct Transfers {
    functions: Vec<Arc<Transfer>>,
    window: DeviceRect,
    /// 0 for the identity, `i + 1` for `functions[i]`.
    selected: Vec<usize>,
}

/// Whether marks select transfers where they are painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selection {
    /// Soft-mask content and tiling cells select nothing.
    Off,
    /// Page content within opaque Normal groups.
    Opaque,
    /// Page content within a group that is not opaque Normal.
    Translucent,
}

impl Transfers {
    /// The selection for `scene`'s window; `None` when no state has a
    /// transfer, so every pixel keeps the identity.
    pub(crate) fn new(scene: &Scene, window: DeviceRect) -> Option<Self> {
        let mut functions: Vec<Arc<Transfer>> = Vec::new();
        for state in &scene.states {
            if let Some(transfer) = &state.transfer
                && !functions.iter().any(|f| Arc::ptr_eq(f, transfer))
            {
                functions.push(Arc::clone(transfer));
            }
        }
        (!functions.is_empty()).then(|| Self {
            functions,
            window,
            selected: vec![0; pixels(window)],
        })
    }

    /// Records an element of `shape` at `(x, y)` painted with `state`.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a partial shape where a non-identity
    /// transfer is involved.
    pub(crate) fn record(
        &mut self,
        (x, y): (u32, u32),
        shape: f64,
        state: &Composite,
        opaque: bool,
    ) -> Result<()> {
        if shape <= 0.0 {
            return Ok(());
        }
        let function = if opaque && simple(state) {
            state.transfer.as_ref().map_or(0, |transfer| {
                self.functions
                    .iter()
                    .position(|f| Arc::ptr_eq(f, transfer))
                    .map_or(0, |i| i + 1)
            })
        } else {
            0
        };
        let Some(selected) = index(self.window, x, y).and_then(|i| self.selected.get_mut(i)) else {
            return Ok(());
        };
        if shape < 1.0 && (function != 0 || *selected != 0) {
            return Err(Error::invalid_argument(
                "the reference has no result for a partial transfer boundary",
            ));
        }
        if shape >= 1.0 {
            *selected = function;
        }
        Ok(())
    }

    /// Applies the transfer selected at `(x, y)` to an RGB output colour.
    pub(crate) fn apply(&self, (x, y): (u32, u32), rgb: [f64; 3]) -> [f64; 3] {
        let Some(transfer) = index(self.window, x, y)
            .and_then(|i| self.selected.get(i))
            .and_then(|&s| s.checked_sub(1))
            .and_then(|i| self.functions.get(i))
        else {
            return rgb;
        };
        let mut out = rgb;
        for (i, value) in out.iter_mut().enumerate() {
            let function = match transfer.as_ref() {
                Transfer::All(function) => Some(function),
                Transfer::PerComponent(functions) => functions.get(i).and_then(Option::as_ref),
            };
            if let Some(function) = function {
                let mut result = [0.0];
                function.evaluate(&[value.clamp(0.0, 1.0)], &mut result);
                *value = result[0].clamp(0.0, 1.0);
            }
        }
        out
    }
}

/// Opaque Normal without a soft mask (11.7.5.2).
pub(crate) fn simple(state: &Composite) -> bool {
    state.alpha == 1.0
        && state.blend == pagura_interpret::BlendMode::Normal
        && state.soft_mask.is_none()
}
