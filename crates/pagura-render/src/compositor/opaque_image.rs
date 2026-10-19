//! A sole opaque RGB image can reach the output without a group surface.
//! Coverage and nearest-neighbour selection still use the common implementations
//! (8.9.5.3; 11.3.3). Partial coverage, colour conversion and transparency retain
//! the f64 compositor; no intermediate colour is quantized here.

use super::{Context, Model, images::sampler_for, sampled::Sampler};
use crate::Tile;
use crate::display::ImageCommand;
use crate::interpreter::images::Image;
use pagura_core::{Error, Result};
use pagura_image::{MaskKind, Samples};
use pagura_raster::{FillRule, PathBuilder, Pixmap};

impl Context {
    // Keep image preparation separate from the general tile compositor.
    #[inline(never)]
    pub(super) fn copy_opaque_image(
        &mut self,
        command: &ImageCommand,
        model: Model,
        output: &mut Pixmap,
        tile: Tile,
        region: Tile,
    ) -> Result<bool> {
        if model != Model::Rgb
            || self.separations.enabled
            || self.limits.depth == 0
            || !command.composite.simple()
            || command.composite.overprint
            || command.transfer.is_some()
            || command.clip.is_some()
        {
            return Ok(false);
        }
        let Some(samples) = rgb_samples(&command.image) else {
            return Ok(false);
        };
        let transform = command.transform.then(self.device);
        let Some(inverse) = transform.inverse() else {
            return Ok(false);
        };
        let sampler = sampler_for(&command.image, inverse);
        if !sampler.inverse.is_finite() || sampler.minifying() {
            return Ok(false);
        }
        // Decline whenever the group path's page surface, or the charge for its
        // one command, would exceed a budget, so that path reports the error;
        // the copy itself charges the same work.
        let Ok((count, _)) = self.admit_surface(region) else {
            return Ok(false);
        };
        if self.work + count >= self.limits.raster_work {
            return Ok(false);
        }
        let clip = self
            .clip(None, region)
            .map_err(|error| crate::display::contextual(error, command.position))?;
        let mut bounds = PathBuilder::new();
        bounds.rect(0.0, 0.0, 1.0, 1.0);
        let Some(clip) = clip
            .clipped_path(bounds.finish(), transform, FillRule::NonZero, self.limits)
            .map_err(|error| crate::display::contextual(error, command.position))?
        else {
            return Ok(false);
        };
        if (clip.area().x(), clip.area().y()) != (region.x, region.y)
            || clip.width() != region.width
            || clip.height() != region.height
            || clip.coverage().iter().any(|&value| value != 255)
        {
            return Ok(false);
        }
        self.charge(count)?;
        self.charge(1)?;
        let result = (|| {
            self.charge(count)?;
            self.charge(count)?;
            copy_samples(output, tile, region, &sampler, samples)?;
            Ok(true)
        })();
        result.map_err(|error| crate::display::contextual(error, command.position))
    }
}

fn copy_samples(
    output: &mut Pixmap,
    tile: Tile,
    region: Tile,
    sampler: &Sampler,
    samples: &[[u8; 3]],
) -> Result<()> {
    let width =
        usize::try_from(region.width).map_err(|_| Error::limit("image width exceeds usize"))?;
    for y in region.y..region.y + region.height {
        let start = usize::try_from(
            u64::from(y - tile.y) * u64::from(tile.width) + u64::from(region.x - tile.x),
        )
        .map_err(|_| Error::limit("image output index exceeds usize"))?;
        let row = output
            .data_mut()
            .get_mut(start * 4..(start + width) * 4)
            .ok_or(Error::invalid_argument("image outside output"))?;
        for (x, pixel) in (region.x..region.x + region.width).zip(row.as_chunks_mut::<4>().0) {
            let &[red, green, blue] = samples
                .get(sampler.nearest(x, y)?)
                .ok_or(Error::syntax("missing image sample"))?;
            *pixel = [red, green, blue, 255];
        }
    }
    Ok(())
}

fn rgb_samples(image: &Image) -> Option<&[[u8; 3]]> {
    if !matches!(image.space, Some(crate::color::Space::Rgb))
        || image.decoded.bits_per_component != 8
        || image.decoded.components != 3
        || image.decoded.interpolate
        || !matches!(image.decoded.mask, MaskKind::None)
        || image.mask.is_some()
        || image.matte.is_some()
        || image.decode.as_slice() != [(0.0, 1.0); 3]
    {
        return None;
    }
    let Samples::Bytes(bytes) = &image.decoded.samples else {
        return None;
    };
    let (samples, rest) = bytes.as_chunks::<3>();
    rest.is_empty().then_some(samples)
}
