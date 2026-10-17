//! Image colour, shape and opacity enter the common compositor (8.9; 11.6.5.2).
use super::sampled::{Sampler, Texel};
use super::{Backdrop, Context, Model, Source, Surface};
use crate::{display::ImageCommand, interpreter::images::Image};
use pagura_core::{Error, Result};
use pagura_image::{ImageKind, MaskKind};
use pagura_raster::{FillRule, PathBuilder, Transform};

impl Context {
    pub(super) fn image(
        &mut self,
        command: &ImageCommand,
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        if command.transfer.is_some() {
            return Err(Error::unsupported(
                crate::features::TRANSPARENCY_TRANSFER,
                "image transfer selection in transparency is not implemented",
            ));
        }
        if model == Model::Cmyk && command.composite.overprint {
            return Err(Error::unsupported(
                crate::features::TRANSPARENCY_OVERPRINT,
                "CMYK image overprint is not implemented",
            ));
        }
        let transform = command.transform.then(self.device);
        if !transform.is_finite() {
            return Err(Error::limit("image transform overflow"));
        }
        let Some(inverse) = transform.inverse() else {
            return Ok(());
        };
        let clip = self.clip(command.clip.as_ref(), target.tile)?;
        let mut bounds = PathBuilder::new();
        bounds.rect(0.0, 0.0, 1.0, 1.0);
        let Some(clip) =
            clip.clipped_path(&bounds.finish(), transform, FillRule::NonZero, self.limits)?
        else {
            return Ok(());
        };
        let soft = command
            .composite
            .mask
            .as_ref()
            .map(|m| self.soft_mask(m, target.tile, model, depth + 1))
            .transpose()?;
        let stencil = self.stencil_color(command, model)?;
        let image = &command.image;
        let sampler = sampler(image, inverse);
        let mask_sampler = image.mask.as_ref().map(|m| sampler_for(m, inverse));
        let width = usize::try_from(clip.width())
            .map_err(|_| Error::limit("image clip width exceeds usize"))?;
        let origin = clip.origin();
        self.charge(clip.coverage().len())?;
        for (row, coverage) in clip.coverage().chunks_exact(width).enumerate() {
            let y =
                origin.1 + u32::try_from(row).map_err(|_| Error::limit("image row exceeds u32"))?;
            for (column, &coverage) in coverage.iter().enumerate().filter(|(_, v)| **v != 0) {
                let x = origin.0
                    + u32::try_from(column)
                        .map_err(|_| Error::limit("image column exceeds u32"))?;
                let mut work = 0;
                let maximum = self.limits.raster_work.saturating_sub(self.work);
                let mut value = sampler.sample(x, y, &mut work, maximum, |pixel| {
                    self.image_texel(command, pixel, model, stencil)
                })?;
                if let (Some(mask), Some(mask_sampler)) = (&image.mask, &mask_sampler) {
                    let sample = mask_sampler
                        .sample(x, y, &mut work, maximum, |pixel| mask_texel(mask, pixel))?;
                    let factor = sample.alpha;
                    value.color = value.color.map(|v| v * factor);
                    value.alpha *= factor;
                    if matches!(image.decoded.mask, MaskKind::Stencil(_))
                        || command.composite.alpha_is_shape
                    {
                        value.shape *= factor;
                    }
                }
                self.charge(work)?;
                let factor = f64::from(coverage) / 255.0;
                let source = Source {
                    color: if value.alpha > 0.0 {
                        value.color.map(|v| v / value.alpha)
                    } else {
                        [0.0; 4]
                    },
                    alpha: value.alpha * factor,
                    shape: value.shape * factor,
                };
                let index = target
                    .index(x, y)
                    .ok_or(Error::invalid_argument("image outside surface"))?;
                let mask = soft
                    .as_ref()
                    .and_then(|m| m.get(index))
                    .copied()
                    .unwrap_or(1.0);
                target
                    .pixels
                    .get_mut(index)
                    .ok_or(Error::invalid_argument("image index outside surface"))?
                    .paint(
                        source.apply(&command.composite, mask),
                        &command.composite,
                        initial.pixel(x, y),
                        knockout,
                        model,
                    );
            }
        }
        Ok(())
    }
    fn stencil_color(&mut self, command: &ImageCommand, model: Model) -> Result<Option<[f64; 4]>> {
        let stencil = if command.image.space.is_none() {
            if matches!(command.stencil, crate::color::Color::Pattern { .. }) {
                return Err(Error::unsupported(
                    crate::interpreter::images::STENCIL_PATTERN,
                    "pattern-coloured image stencils are not implemented",
                ));
            }
            Some(model.color(
                &command.stencil,
                &command.composite.conversion,
                &mut self.function_work,
            )?)
        } else {
            None
        };
        Ok(stencil)
    }
    fn image_texel(
        &mut self,
        command: &ImageCommand,
        pixel: usize,
        model: Model,
        stencil: Option<[f64; 4]>,
    ) -> Result<Texel> {
        let image = &command.image;
        if let Some(color) = stencil {
            let mut value = mask_texel(image, pixel)?;
            value.color = color.map(|v| v * value.alpha);
            return Ok(value);
        }
        let mut components = [0.0; 32];
        image.components(pixel, &mut components)?;
        let (space, count) = expand_indexed(image, &mut components)?;
        let mut shape = 1.0;
        let mut alpha = 1.0;
        if let MaskKind::ColorKey(ranges) = &image.decoded.mask {
            let offset = pixel
                .checked_mul(usize::from(image.decoded.components))
                .ok_or(Error::limit("colour key sample overflow"))?;
            if ranges.iter().enumerate().all(|(i, &(lo, hi))| {
                image
                    .decoded
                    .samples
                    .get(offset + i)
                    .is_some_and(|v| v >= lo && v <= hi)
            }) {
                shape = 0.0;
                alpha = 0.0;
            }
        }
        if let MaskKind::Embedded(plane) = &image.decoded.mask {
            let opacity = f64::from(
                plane
                    .samples
                    .get(pixel)
                    .ok_or(Error::syntax("missing JPX opacity"))?,
            ) / precision_max(plane.bits_per_component)?;
            alpha *= opacity;
            if command.composite.alpha_is_shape {
                shape *= opacity;
            }
            if plane.premultiplied {
                for c in components.iter_mut().take(count) {
                    *c = if opacity > 0.0 { *c / opacity } else { 0.0 };
                }
            }
        }
        if let (Some(matte), Some(mask)) = (&image.matte, &image.mask) {
            let opacity = mask_texel(mask, pixel)?.alpha;
            for (value, &matte) in components.iter_mut().zip(matte) {
                *value = if opacity > 0.0 {
                    matte + (*value - matte) / opacity
                } else {
                    0.0
                };
            }
        }
        let color = model.source(
            space,
            &components[..count],
            &command.composite.conversion,
            &mut self.function_work,
        )?;
        Ok(Texel {
            color: color.map(|v| v * alpha),
            alpha,
            shape,
        })
    }
}
fn sampler(image: &Image, inverse: Transform) -> Sampler {
    sampler_for(image, inverse)
}
fn sampler_for(image: &Image, inverse: Transform) -> Sampler {
    Sampler {
        width: image.decoded.width,
        height: image.decoded.height,
        interpolate: image.decoded.interpolate,
        inverse: inverse.then(Transform::new(
            f64::from(image.decoded.width),
            0.0,
            0.0,
            -f64::from(image.decoded.height),
            0.0,
            f64::from(image.decoded.height),
        )),
    }
}
fn mask_texel(image: &Image, pixel: usize) -> Result<Texel> {
    let sample = image
        .decoded
        .samples
        .get(pixel)
        .ok_or(Error::syntax("missing mask sample"))?;
    let alpha = if let ImageKind::Stencil { inverted } = image.decoded.kind {
        f64::from(u8::from((sample == 1) == inverted))
    } else {
        let mut values = [0.0; 32];
        image.components(pixel, &mut values)?;
        values[0].clamp(0.0, 1.0)
    };
    Ok(Texel {
        color: [alpha; 4],
        alpha,
        shape: alpha,
    })
}
fn precision_max(bits: u8) -> Result<f64> {
    if !(1..=16).contains(&bits) {
        return Err(Error::syntax("invalid opacity precision"));
    }
    Ok(f64::from(u16::MAX >> (16 - bits)))
}

fn expand_indexed<'a>(
    image: &'a Image,
    values: &mut [f64; 32],
) -> Result<(&'a crate::color::Space, usize)> {
    use crate::color::Space;
    let space = image
        .space
        .as_ref()
        .ok_or(Error::syntax("colour image missing space"))?;
    if let Space::Indexed { base, high, lookup } = space {
        let value = values[0].round().clamp(0.0, f64::from(*high));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "index rounded and clamped to 0..255"
        )]
        let index = value as usize;
        let n = base.components();
        let bytes = lookup
            .get(index * n..(index + 1) * n)
            .ok_or(Error::syntax("truncated image palette"))?;
        let decoded = if let Space::Cie(cie) = base.as_ref() {
            cie.decode_lookup(bytes)
        } else {
            bytes.iter().map(|&v| f64::from(v) / 255.0).collect()
        };
        for (out, value) in values.iter_mut().zip(decoded) {
            *out = value;
        }
        Ok((base, n))
    } else {
        Ok((space, usize::from(image.decoded.components)))
    }
}
