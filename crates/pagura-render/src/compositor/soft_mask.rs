//! Soft masks: the mask group rendered to per-pixel values (11.6.5).

use super::memory::Held;
use super::{Backdrop, Context, Frame, Pixel};
use crate::Tile;
use crate::transparency::{MaskMethod, Model, SoftMask};
use pagura_core::{Error, Result};
use std::sync::Arc;

pub(super) struct CachedMask {
    pub(super) id: usize,
    pub(super) tile: Tile,
    pub(super) model: Model,
    pub(super) values: Arc<Vec<f64>>,
    /// The values' bytes, released when the cache entry goes.
    pub(super) _held: Held,
}

impl Context {
    pub(super) fn discard_mask(&mut self) {
        self.mask = None;
    }
    pub(super) fn soft_mask(
        &mut self,
        mask: &Arc<SoftMask>,
        tile: Tile,
        parent_model: Model,
        depth: usize,
    ) -> Result<Arc<Vec<f64>>> {
        let id = Arc::as_ptr(mask) as usize;
        if let Some(cached) = &self.mask
            && cached.id == id
            && cached.tile == tile
            && cached.model == parent_model
        {
            return Ok(Arc::clone(&cached.values));
        }
        self.discard_mask();
        let model = mask.group.attributes.model.unwrap_or(parent_model);
        let initial =
            if matches!(mask.method, MaskMethod::Luminosity) && !mask.group.attributes.isolated {
                Backdrop::Color(mask.backdrop)
            } else {
                Backdrop::Transparent
            };
        let mut surface = self
            .region(&mask.group, tile)?
            .map(|region| self.surface(region, initial))
            .transpose()?;
        if let Some(surface) = &mut surface {
            // Mask G has its own BBox; the page crop is applied to the painted
            // object later, not to the mask source (11.6.5.1).
            let previous = std::mem::replace(&mut self.soft_source, true);
            self.clip = None;
            let suspended = self.transfers.as_ref().is_some_and(|r| r.suspended);
            if let Some(regions) = &mut self.transfers {
                regions.suspended = true;
            }
            let spot_batch = self.spot_batch.take();
            let mask_colors = std::mem::replace(&mut self.mask_colors, true);
            let members = Frame::group(initial, model, mask.group.attributes.knockout, depth);
            let result = self.commands(&mask.group.commands, surface, members);
            self.soft_source = previous;
            if let Some(regions) = &mut self.transfers {
                regions.suspended = suspended;
            }
            self.spot_batch = spot_batch;
            self.mask_colors = mask_colors;
            self.clip = None;
            result?;
        }
        self.discard_mask();
        let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height))
            .map_err(|_| Error::limit("mask size exceeds usize"))?;
        let bytes = count
            .checked_mul(size_of::<f64>())
            .ok_or(Error::limit("mask bytes overflow"))?;
        self.trim_raster(bytes);
        let held = self
            .memory
            .hold(bytes, "soft-mask memory budget exceeded")?;
        self.charge(count)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("soft-mask allocation failed"))?;
        let (mut last, mut mapped) = (f64::NAN, 0.0);
        for y in tile.y..tile.y + tile.height {
            for x in tile.x..tile.x + tile.width {
                let p = surface
                    .as_ref()
                    .and_then(|s| s.index(x, y).and_then(|i| s.pixels().get(i)))
                    .copied()
                    .unwrap_or_else(|| initial.pixel(x, y));
                let value = mask_value(mask, p, model).clamp(0.0, 1.0);
                if value != last {
                    mapped = if let Some(function) = &mask.transfer {
                        function
                            .evaluate_scalar(value, &mut self.function_work)?
                            .clamp(0.0, 1.0)
                    } else {
                        value
                    };
                    last = value;
                }
                values.push(mapped);
            }
        }
        drop(surface);
        let values = Arc::new(values);
        self.mask = Some(CachedMask {
            id,
            tile,
            model: parent_model,
            values: Arc::clone(&values),
            _held: held,
        });
        Ok(values)
    }
}

fn mask_value(mask: &SoftMask, pixel: Pixel, model: Model) -> f64 {
    match mask.method {
        MaskMethod::Alpha => pixel.group,
        MaskMethod::Luminosity => {
            let color = if mask.group.attributes.isolated {
                [0, 1, 2, 3].map(|i| pixel.color[i] + (1.0 - pixel.alpha) * mask.backdrop[i])
            } else {
                pixel.color
            };
            model.luminosity(color)
        }
    }
}
