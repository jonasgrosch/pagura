//! Image colour, shape and opacity enter the common compositor (8.9; 11.6.5.2).
use super::reduced::Prepared;
use super::sampled::{Sampler, Texel};
use super::{Backdrop, Context, Model, Source, Surface};
use crate::{display::ImageCommand, interpreter::images::Image};
use pagura_core::{Error, Result};
use pagura_image::{ImageKind, MaskKind, Samples};
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
        let template = self.image_source(command, model, stencil)?;
        let image = &command.image;
        let origin = clip.origin();
        self.charge(clip.coverage().len())?;
        if clip.spans().next().is_none() {
            return Ok(());
        }
        let mut texels = Texels::new(image, model);
        let available = self.limits.decoded_bytes.saturating_sub(self.allocated);
        let (mut sampler, mut mask_sampler) =
            prepare(image, inverse, origin.0, clip.width(), available)?;
        let sampling_bytes = sampler.bytes() + mask_sampler.as_ref().map_or(0, Prepared::bytes);
        self.trim_raster(sampling_bytes);
        self.allocated += sampling_bytes;
        for (row, column, coverage) in clip.spans() {
            let y =
                origin.1 + u32::try_from(row).map_err(|_| Error::limit("image row exceeds u32"))?;
            for (offset, &coverage) in coverage.iter().enumerate() {
                let column = column + offset;
                let x = origin.0
                    + u32::try_from(column)
                        .map_err(|_| Error::limit("image column exceeds u32"))?;
                let mut work = 0;
                let maximum = self.limits.raster_work.saturating_sub(self.work);
                let mut value = sampler.sample(x, y, &mut work, maximum, |pixel| {
                    texels.sample(self, command, pixel, model, stencil)
                })?;
                if let (Some(mask), Some(mask_sampler)) = (&image.mask, &mut mask_sampler) {
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
                    color: if value.alpha == 1.0 {
                        value.color
                    } else if value.alpha > 0.0 {
                        value.color.map(|v| v / value.alpha)
                    } else {
                        [0.0; 4]
                    },
                    alpha: value.alpha * factor,
                    shape: value.shape * factor,
                    ..template
                };
                let index = target
                    .index(x, y)
                    .ok_or(Error::invalid_argument("image outside surface"))?;
                let mask = soft
                    .as_ref()
                    .and_then(|m| m.get(index))
                    .copied()
                    .unwrap_or(1.0);
                self.image_transfer(command, (x, y), source)?;
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
        self.allocated -= sampling_bytes;
        Ok(())
    }
    fn image_source(
        &mut self,
        command: &ImageCommand,
        model: Model,
        stencil: Option<Source>,
    ) -> Result<Source> {
        if let Some(stencil) = stencil {
            return Ok(stencil);
        }
        let space = command
            .image
            .space
            .as_ref()
            .ok_or(Error::syntax("image missing colour space"))?;
        Ok(self.sampled_source(space, model, &command.composite))
    }
    fn stencil_color(&mut self, command: &ImageCommand, model: Model) -> Result<Option<Source>> {
        let stencil = if command.image.space.is_none() {
            if matches!(command.stencil, crate::color::Color::Pattern { .. }) {
                return Err(Error::unsupported(
                    crate::interpreter::images::STENCIL_PATTERN,
                    "pattern-coloured image stencils are not implemented",
                ));
            }
            Some(self.color(&command.stencil, model, &command.composite, true)?)
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
        stencil: Option<Source>,
    ) -> Result<Texel> {
        let image = &command.image;
        if let Some(color) = stencil {
            let mut value = mask_texel(image, pixel)?;
            value.color = color.color.map(|v| v * value.alpha);
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
        let color = self
            .source(
                space,
                &components[..count],
                model,
                &command.composite,
                false,
            )?
            .color;
        Ok(Texel {
            color: color.map(|v| v * alpha),
            alpha,
            shape,
        })
    }
}
fn prepare(
    image: &Image,
    inverse: Transform,
    origin: u32,
    width: u32,
    bytes: usize,
) -> Result<(Prepared, Option<Prepared>)> {
    let sampler = sampler_for(image, inverse).prepare(origin, width, bytes)?;
    let mask = image
        .mask
        .as_ref()
        .map(|mask| {
            sampler_for(mask, inverse).prepare(origin, width, bytes.saturating_sub(sampler.bytes()))
        })
        .transpose()?;
    Ok((sampler, mask))
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
        let n = base.components();
        let decoded = crate::color::indexed_components(base, *high, lookup, values[0])?;
        for (out, value) in values.iter_mut().zip(decoded) {
            *out = value;
        }
        Ok((base, n))
    } else {
        Ok((space, usize::from(image.decoded.components)))
    }
}

// Cache by raw sample tuple, never by destination pixel. The blending model and
// stencil colour are fixed within this image invocation.
#[expect(
    clippy::large_enum_variant,
    reason = "bounded sample caches stay on the stack without per-image heap allocation"
)]
enum Texels<'a> {
    Uncached,
    Cached([Option<(u32, Texel)>; 256]),
    Gray {
        pixels: &'a [u8],
        colors: [[f64; 4]; 256],
    },
    Rgb {
        pixels: &'a [[u8; 3]],
        channels: [[f64; 256]; 3],
    },
}
impl<'a> Texels<'a> {
    fn new(image: &'a Image, model: Model) -> Self {
        // Decode and clipping apply per sample before any weighted sum (8.9.5.2).
        // For DeviceGray, and DeviceRGB into RGB, each channel depends on one
        // sample, so a 256-entry table equals the scalar result.
        if matches!(model, Model::Gray | Model::Rgb)
            && matches!(image.space, Some(crate::color::Space::Gray))
            && (1..=8).contains(&image.decoded.bits_per_component)
            && image.decoded.components == 1
            && image.matte.is_none()
            && matches!(image.decoded.mask, MaskKind::None)
            && let Samples::Bytes(pixels) = &image.decoded.samples
            && let [(low, high)] = image.decode.as_slice()
        {
            let maximum = f64::from(u16::MAX >> (16 - image.decoded.bits_per_component));
            let colors = std::array::from_fn(|sample| {
                let value = (low + sample as f64 / maximum * (high - low)).clamp(0.0, 1.0);
                if model == Model::Rgb {
                    [value, value, value, 0.0]
                } else {
                    [value, 0.0, 0.0, 0.0]
                }
            });
            return Self::Gray { pixels, colors };
        }
        if model == Model::Rgb
            && matches!(image.space, Some(crate::color::Space::Rgb))
            && image.decoded.bits_per_component == 8
            && image.decoded.components == 3
            && image.matte.is_none()
            && matches!(image.decoded.mask, MaskKind::None)
            && let Samples::Bytes(bytes) = &image.decoded.samples
            && let [red, green, blue] = image.decode.as_slice()
        {
            let (pixels, remainder) = bytes.as_chunks::<3>();
            if remainder.is_empty() {
                let channels = [red, green, blue].map(|&(low, high)| {
                    std::array::from_fn(|sample| {
                        (low + sample as f64 / 255.0 * (high - low)).clamp(0.0, 1.0)
                    })
                });
                return Self::Rgb { pixels, channels };
            }
        }
        if (1..=4).contains(&image.decoded.components)
            && (0..usize::from(image.decoded.components)).all(|i| {
                image
                    .decoded
                    .component_bits(i)
                    .is_some_and(|bits| (1..=8).contains(&bits))
            })
            && image.matte.is_none()
            && !matches!(image.decoded.mask, MaskKind::Embedded(_))
        {
            Self::Cached([None; 256])
        } else {
            Self::Uncached
        }
    }
    fn sample(
        &mut self,
        context: &mut Context,
        command: &ImageCommand,
        pixel: usize,
        model: Model,
        stencil: Option<Source>,
    ) -> Result<Texel> {
        let values = match self {
            Self::Gray { pixels, colors } => {
                let sample = pixels
                    .get(pixel)
                    .ok_or(Error::syntax("missing image sample"))?;
                return Ok(Texel {
                    color: colors[usize::from(*sample)],
                    alpha: 1.0,
                    shape: 1.0,
                });
            }
            Self::Rgb { pixels, channels } => {
                let &[red, green, blue] = pixels
                    .get(pixel)
                    .ok_or(Error::syntax("missing image sample"))?;
                return Ok(Texel {
                    color: [
                        channels[0][usize::from(red)],
                        channels[1][usize::from(green)],
                        channels[2][usize::from(blue)],
                        0.0,
                    ],
                    alpha: 1.0,
                    shape: 1.0,
                });
            }
            Self::Uncached => return context.image_texel(command, pixel, model, stencil),
            Self::Cached(values) => values,
        };
        let count = usize::from(command.image.decoded.components);
        let offset = pixel
            .checked_mul(count)
            .ok_or(Error::limit("image sample offset overflow"))?;
        let mut key = 0_u32;
        for component in 0..count {
            let sample = command
                .image
                .decoded
                .samples
                .get(offset + component)
                .filter(|&v| v <= 255)
                .ok_or(Error::syntax("missing or out-of-range image sample"))?;
            key = (key << 8) | u32::from(sample);
        }
        let index = if count == 1 {
            usize::from(key.to_le_bytes()[0])
        } else {
            usize::from(key.wrapping_mul(0x9e37_79b1).to_be_bytes()[0])
        };
        let cached = &mut values[index];
        if let Some((previous, value)) = *cached
            && previous == key
        {
            return Ok(value);
        }
        let value = context.image_texel(command, pixel, model, stencil)?;
        *cached = Some((key, value));
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_image::DecodedImage;
    use std::sync::Arc;
    fn context() -> Context {
        let limits = pagura_core::Limits::default();
        Context {
            overprint: crate::overprint::Policy::Ignore,
            elide_opaque_groups: false,
            separations: Arc::new(super::super::Separations::default()),
            preview: super::super::colorants::Preview::default(),
            spot_batch: None,
            mask_colors: false,
            raster: pagura_raster::Rasterizer::default(),
            bounds: super::super::regions::Prepared::new(Transform::IDENTITY, limits.decoded_bytes),
            glyphs: crate::display::GlyphCache::new(limits.decoded_bytes),
            limits,
            device: Transform::IDENTITY,
            page_device: Transform::IDENTITY,
            view_unit: 1.0,
            crop: PathBuilder::new().finish(),
            allocated: 0,
            work: 0,
            clip: None,
            mask: None,
            function_work: 0,
            soft_source: false,
            transfers: None,
        }
    }

    fn command(image: Image) -> ImageCommand {
        ImageCommand {
            image: Arc::new(image),
            transform: Transform::IDENTITY,
            clip: None,
            stencil: crate::color::Color::default(),
            composite: crate::transparency::Composite::opaque(),
            transfer: None,
            position: pagura_core::Position::Unknown,
        }
    }

    fn byte_image(components: u8, bits: u8) -> Image {
        Image {
            decoded: DecodedImage {
                width: 256,
                height: 1,
                bits_per_component: bits,
                components,
                samples: Samples::Bytes(
                    (0..=255)
                        .flat_map(|v| {
                            std::iter::repeat_n(
                                v & (u8::MAX >> (8 - bits)),
                                usize::from(components),
                            )
                        })
                        .collect(),
                ),
                kind: ImageKind::Color { color_space: None },
                decode: None,
                mask: MaskKind::None,
                interpolate: false,
            },
            space: Some(if components == 1 {
                crate::color::Space::Gray
            } else {
                crate::color::Space::Rgb
            }),
            mask: None,
            matte: None,
            decode: vec![(0.0, 1.0); usize::from(components)],
            intent: None,
        }
    }

    #[test]
    fn byte_tables_preserve_decode_clipping_and_scalar_evaluation() {
        for (components, bits, model) in [
            (3, 8, Model::Rgb),
            (1, 1, Model::Rgb),
            (1, 2, Model::Rgb),
            (1, 4, Model::Rgb),
            (1, 8, Model::Gray),
            (1, 8, Model::Rgb),
        ] {
            for range in [(0.0, 1.0), (1.0, 0.0), (-0.25, 1.25), (0.125, 0.875)] {
                let mut image = byte_image(components, bits);
                image.decode.fill(range);
                if components == 3
                    && let Samples::Bytes(bytes) = &mut image.decoded.samples
                {
                    for pixel in bytes.as_chunks_mut::<3>().0 {
                        pixel[1] = 255 - pixel[0];
                        pixel[2] = pixel[0].wrapping_mul(73);
                    }
                    image.decode[1] = (range.1, range.0);
                }
                let command = command(image);
                let mut texels = Texels::new(&command.image, model);
                assert!(matches!(texels, Texels::Rgb { .. } | Texels::Gray { .. }));
                let mut context = context();
                for pixel in 0..256 {
                    let actual = texels
                        .sample(&mut context, &command, pixel, model, None)
                        .unwrap();
                    let scalar = context.image_texel(&command, pixel, model, None).unwrap();
                    assert_eq!(
                        actual.color.map(f64::to_bits),
                        scalar.color.map(f64::to_bits)
                    );
                    // ISO 32000-2 8.9.5.2: Decode is applied per component,
                    // followed by source-space clipping; channels never mix here.
                    let maximum = f64::from(u16::MAX >> (16 - bits));
                    let raw = pixel & usize::from(u8::MAX >> (8 - bits));
                    let expected =
                        (range.0 + raw as f64 / maximum * (range.1 - range.0)).clamp(0.0, 1.0);
                    assert_eq!(actual.color[0].to_bits(), expected.to_bits());
                    assert_eq!(actual.alpha, 1.0);
                    assert_eq!(actual.shape, 1.0);
                }
                assert_eq!(context.function_work, 0);
            }
        }
    }

    #[test]
    fn byte_tables_exclude_masks_matte_and_nonmatching_spaces() {
        let mut image = byte_image(3, 8);
        image.decoded.mask = MaskKind::ColorKey(vec![(0, 0); 3]);
        assert!(matches!(Texels::new(&image, Model::Rgb), Texels::Cached(_)));
        image.decoded.mask = MaskKind::None;
        image.matte = Some(vec![0.0; 3]);
        assert!(matches!(Texels::new(&image, Model::Rgb), Texels::Uncached));
        image.matte = None;
        assert!(matches!(
            Texels::new(&image, Model::Cmyk),
            Texels::Cached(_)
        ));
        image.space = Some(crate::color::Space::Icc {
            base: Arc::new(crate::color::Space::Rgb),
            profile: None,
            range: [[0.0, 1.0]; 4],
        });
        assert!(matches!(Texels::new(&image, Model::Rgb), Texels::Cached(_)));
    }

    #[test]
    fn scalar_cache_uses_component_precision_for_mixed_samples() {
        let image = Arc::new(Image {
            decoded: DecodedImage {
                width: 1,
                height: 1,
                bits_per_component: 0,
                components: 1,
                samples: Samples::Mixed {
                    values: vec![32768],
                    bits: vec![16],
                },
                kind: ImageKind::Color {
                    color_space: Some(pagura_core::Value::name(b"DeviceGray")),
                },
                decode: None,
                mask: MaskKind::None,
                interpolate: false,
            },
            space: Some(crate::color::Space::Gray),
            mask: None,
            matte: None,
            decode: vec![(0.0, 1.0)],
            intent: None,
        });
        let command = ImageCommand {
            image,
            transform: Transform::IDENTITY,
            clip: None,
            stencil: crate::color::Color::default(),
            composite: crate::transparency::Composite::opaque(),
            transfer: None,
            position: pagura_core::Position::Unknown,
        };
        let mut texels = Texels::new(&command.image, Model::Rgb);
        let mut context = context();
        let sample = texels
            .sample(&mut context, &command, 0, Model::Rgb, None)
            .unwrap();
        assert_eq!(
            sample.color,
            [32768.0 / 65535.0, 32768.0 / 65535.0, 32768.0 / 65535.0, 0.0]
        );
        assert_eq!(sample.alpha, 1.0);
    }
}
