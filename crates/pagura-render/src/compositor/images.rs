//! Image colour, shape and opacity enter the common compositor (8.9; 11.6.5.2).
use super::reduced::Prepared;
use super::sampled::{Sampler, Texel};
use super::{Backdrop, Context, Model, Source, Surface};
use crate::display::ImageCommand;
use crate::interpreter::images::{Image, max_sample};
use pagura_core::{Error, Result};
use pagura_image::{ImageKind, MaskKind, Samples};
use pagura_raster::{FillRule, PathBuilder, Transform};

impl Context {
    // Keep sampling and painting out of the command dispatch.
    #[inline(never)]
    pub(super) fn image(
        &mut self,
        command: &ImageCommand,
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<()> {
        // Keep an expanded palette only while its image is painted.
        if matches!(
            command.image.space,
            Some(crate::color::Space::Indexed { .. })
        ) && self
            .palette
            .as_ref()
            .is_some_and(|p| !p.matches(&command.image))
        {
            self.clear_palette();
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
        if matches!(target.narrow(), Some(super::narrow::Buffer::U8(_)))
            && self.constant_stencil((command, &clip), target, initial, model, knockout, depth)?
        {
            return Ok(());
        }
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
        let mut mask_samples = MaskSamples { table: [None; 256] };
        self.charge(clip.coverage().len())?;
        if clip.spans().next().is_none() {
            return Ok(());
        }
        // Byte pages take fully covered interiors from the integer sampler.
        let interiors = self.image_interiors(command, target, model, &clip)?;
        let mut texels = Texels::new(image, model);
        let (mut sampler, mut mask_sampler) =
            self.image_samplers(image, inverse, origin.0, clip.width())?;
        let sampling_bytes = sampler.bytes() + mask_sampler.as_ref().map_or(0, Prepared::bytes);
        self.trim_raster(sampling_bytes);
        self.allocated += sampling_bytes;
        for (row, column, coverage) in clip.spans() {
            if interiors && coverage.first() == Some(&255) {
                continue;
            }
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
                    let factor = mask_sampler.sample(x, y, &mut work, maximum, |pixel| {
                        mask_samples.sample(mask, pixel)
                    })?;
                    value.color = value.color.map(|v| v * factor);
                    value.alpha *= factor;
                    if matches!(image.decoded.mask, MaskKind::Stencil(_))
                        || command.composite.alpha_is_shape
                    {
                        value.shape *= factor;
                    }
                }
                self.charge(work)?;
                let source = covered_source(value, coverage, template);
                let index = target
                    .index(x, y)
                    .ok_or(Error::invalid_argument("image outside surface"))?;
                let mask = soft
                    .as_ref()
                    .and_then(|m| m.get(index))
                    .copied()
                    .unwrap_or(1.0);
                self.image_transfer(command, (x, y), source)?;
                if let Some(buffer) = target.narrow_mut() {
                    buffer.paint(index, source.apply(&command.composite, mask))?;
                    continue;
                }
                target
                    .pixels_mut()
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
    /// Prepares the image and mask samplers, releasing a cached palette
    /// when the budget does not cover both.
    fn image_samplers(
        &mut self,
        image: &Image,
        inverse: Transform,
        origin: u32,
        width: u32,
    ) -> Result<(Prepared, Option<Prepared<f64>>)> {
        let available = self.limits.decoded_bytes.saturating_sub(self.allocated);
        match prepare(image, inverse, origin, width, available) {
            Err(error) if error.kind == pagura_core::ErrorKind::Limit && self.palette.is_some() => {
                self.clear_palette();
                let available = self.limits.decoded_bytes.saturating_sub(self.allocated);
                prepare(image, inverse, origin, width, available)
            }
            result => result,
        }
    }
    /// Paints a one-sample stencil mask, which paints its colour over its
    /// whole unit square or nothing, as a fill of that square: on byte
    /// storage, where both store the same coverage bytes.
    fn constant_stencil(
        &mut self,
        (command, clip): (&ImageCommand, &pagura_raster::Mask),
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
    ) -> Result<bool> {
        let image = &command.image;
        if image.decoded.width != 1
            || image.decoded.height != 1
            || !matches!(image.decoded.kind, ImageKind::Stencil { .. })
            || !matches!(image.decoded.mask, MaskKind::None)
            || image.mask.is_some()
            || command.composite.mask.is_some()
            || !matches!(command.stencil, crate::color::Color::Solid { .. })
        {
            return Ok(false);
        }
        if mask_texel(image, 0)?.alpha == 0.0 {
            // An unpainted sample still validates the stencil's colour.
            self.stencil_color(command, model)?;
            return Ok(true);
        }
        let paint = crate::display::PathCommand {
            path: std::sync::Arc::default(),
            mark: crate::display::Mark::Fill(FillRule::NonZero),
            clip: None,
            color: command.stencil.clone(),
            position: command.position,
            transfer: command.transfer.clone(),
            composite: command.composite.clone(),
        };
        self.paint_path((&paint, clip), target, initial, model, knockout, depth)?;
        Ok(true)
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
) -> Result<(Prepared, Option<Prepared<f64>>)> {
    let sampler = sampler_for(image, inverse).prepare(origin, width, bytes)?;
    let mask = image
        .mask
        .as_ref()
        .map(|mask| {
            sampler_for(mask, inverse).prepare_opacity(
                origin,
                width,
                bytes.saturating_sub(sampler.bytes()),
            )
        })
        .transpose()?;
    Ok((sampler, mask))
}

pub(super) fn sampler_for(image: &Image, inverse: Transform) -> Sampler {
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

pub(super) struct MaskSamples {
    table: [Option<f64>; 256],
}
impl MaskSamples {
    pub(super) fn sample(&mut self, image: &Image, pixel: usize) -> Result<f64> {
        if image.decoded.components == 1
            && (1..=8).contains(&image.decoded.bits_per_component)
            && let Samples::Bytes(values) = &image.decoded.samples
        {
            let raw = values
                .get(pixel)
                .ok_or(Error::syntax("missing mask sample"))?;
            let slot = &mut self.table[usize::from(*raw)];
            if let Some(value) = *slot {
                return Ok(value);
            }
            let value = mask_texel(image, pixel)?.alpha;
            *slot = Some(value);
            Ok(value)
        } else {
            Ok(mask_texel(image, pixel)?.alpha)
        }
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
    /// As `Rgb`, with decoded but unclipped channels and a matte to remove.
    MattedRgb {
        pixels: &'a [[u8; 3]],
        channels: [[f64; 256]; 3],
        matte: Matte<'a>,
    },
}

/// Matte removal for an image whose soft mask holds one sample of 1 to 8
/// bits per pixel (11.6.5.3), with the mask's opacity per sample code.
struct Matte<'a> {
    codes: &'a [u8],
    opacity: [f64; 256],
    color: [f64; 3],
}
impl<'a> Matte<'a> {
    fn new(image: &'a Image) -> Option<Self> {
        let &[red, green, blue] = image.matte.as_deref()? else {
            return None;
        };
        let mask = image.mask.as_ref()?;
        if mask.decoded.components != 1
            || !(1..=8).contains(&mask.decoded.bits_per_component)
            || !matches!(mask.decoded.kind, ImageKind::Color { .. })
        {
            return None;
        }
        let Samples::Bytes(codes) = &mask.decoded.samples else {
            return None;
        };
        let [(low, high)] = mask.decode.as_slice() else {
            return None;
        };
        let maximum = f64::from(max_sample(&mask.decoded, 0).ok()?);
        Some(Self {
            codes,
            // As `mask_texel`: Decode, then clipping to [0, 1].
            opacity: std::array::from_fn(|code| {
                (low + code as f64 / maximum * (high - low)).clamp(0.0, 1.0)
            }),
            color: [red, green, blue],
        })
    }

    /// Removes the matte from decoded, unclipped components, then clips them
    /// as the DeviceRGB conversion does. Zero opacity gives black.
    fn remove(&self, pixel: usize, color: [f64; 3]) -> Result<[f64; 3]> {
        let &code = self
            .codes
            .get(pixel)
            .ok_or(Error::syntax("missing mask sample"))?;
        let opacity = self.opacity[usize::from(code)];
        Ok(std::array::from_fn(|i| {
            if opacity > 0.0 {
                (self.color[i] + (color[i] - self.color[i]) / opacity).clamp(0.0, 1.0)
            } else {
                0.0
            }
        }))
    }
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
            && !matches!(
                image.decoded.mask,
                MaskKind::ColorKey(_) | MaskKind::Embedded(_)
            )
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
        let matte = Matte::new(image);
        if model == Model::Rgb
            && matches!(image.space, Some(crate::color::Space::Rgb))
            && image.decoded.bits_per_component == 8
            && image.decoded.components == 3
            && (image.matte.is_none() || matte.is_some())
            && !matches!(
                image.decoded.mask,
                MaskKind::ColorKey(_) | MaskKind::Embedded(_)
            )
            && let Samples::Bytes(bytes) = &image.decoded.samples
            && let [red, green, blue] = image.decode.as_slice()
        {
            let (pixels, remainder) = bytes.as_chunks::<3>();
            if remainder.is_empty() {
                // A matte is removed from decoded components before clipping.
                let decode = |&(low, high): &(f64, f64)| -> [f64; 256] {
                    std::array::from_fn(|sample| low + sample as f64 / 255.0 * (high - low))
                };
                let clipped = |range| decode(range).map(|v| v.clamp(0.0, 1.0));
                return match matte {
                    Some(matte) => Self::MattedRgb {
                        pixels,
                        channels: [red, green, blue].map(decode),
                        matte,
                    },
                    None => Self::Rgb {
                        pixels,
                        channels: [red, green, blue].map(clipped),
                    },
                };
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
            Self::MattedRgb {
                pixels,
                channels,
                matte,
            } => {
                let &[red, green, blue] = pixels
                    .get(pixel)
                    .ok_or(Error::syntax("missing image sample"))?;
                let [red, green, blue] = matte.remove(
                    pixel,
                    [
                        channels[0][usize::from(red)],
                        channels[1][usize::from(green)],
                        channels[2][usize::from(blue)],
                    ],
                )?;
                return Ok(Texel {
                    color: [red, green, blue, 0.0],
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

/// The unpremultiplied source of a sampled texel, scaled by its pixel coverage.
fn covered_source(value: Texel, coverage: u8, template: Source) -> Source {
    let factor = f64::from(coverage) / 255.0;
    Source {
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
            fragments: None,
            spare: Vec::new(),
            palette: None,
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
            viewport: None,
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
    fn matte_removal_matches_independent_rational_channels() {
        let mut image = byte_image(3, 8);
        image.decoded.width = 1;
        image.decoded.samples = Samples::Bytes(vec![128, 192, 0]);
        let mut mask = byte_image(1, 8);
        mask.decoded.width = 1;
        mask.decoded.samples = Samples::Bytes(vec![128]);
        image.matte = Some(vec![1.0; 3]);
        image.mask = Some(Arc::new(mask));
        image.decoded.mask = MaskKind::Soft(pagura_core::ObjectId::new(1, 0));
        let command = command(image);
        let value = context()
            .image_texel(&command, 0, Model::Rgb, None)
            .unwrap();
        // White-matte removal at opacity 128/255 gives 1/128, 65/128, 0
        // after clipping, independently of sampling or compositing (8.9.6).
        assert_eq!(
            value.color[..3]
                .iter()
                .map(|&v| super::super::quantize(v))
                .collect::<Vec<_>>(),
            [2, 129, 0]
        );
        assert_eq!(value.alpha, 1.0);
        assert_eq!(value.shape, 1.0);
    }

    #[test]
    fn matte_tables_equal_scalar_texels_for_every_sample_and_mask_code() {
        for matte in [[1.0; 3], [0.0; 3], [2.0, -1.0, 0.5]] {
            for range in [(0.0, 1.0), (1.0, 0.0), (-0.5, 1.5)] {
                for bits in [1, 2, 4, 8] {
                    for polarity in [(0.0, 1.0), (1.0, 0.0), (0.25, 0.75)] {
                        let mut image = byte_image(3, 8);
                        image.matte = Some(matte.to_vec());
                        image.decode = vec![range; 3];
                        let mut mask = byte_image(1, bits);
                        mask.decode[0] = polarity;
                        image.mask = Some(Arc::new(mask));
                        image.decoded.mask = MaskKind::Soft(pagura_core::ObjectId::new(1, 0));
                        let command = command(image);
                        let mut context = context();
                        let mut table = Texels::new(&command.image, Model::Rgb);
                        assert!(matches!(table, Texels::MattedRgb { .. }));
                        for pixel in 0..256 {
                            let expected = context
                                .image_texel(&command, pixel, Model::Rgb, None)
                                .unwrap();
                            let actual = table
                                .sample(&mut context, &command, pixel, Model::Rgb, None)
                                .unwrap();
                            assert_eq!(
                                actual.color.map(f64::to_bits),
                                expected.color.map(f64::to_bits)
                            );
                            assert_eq!(actual.alpha.to_bits(), expected.alpha.to_bits());
                            assert_eq!(actual.shape.to_bits(), expected.shape.to_bits());
                        }
                    }
                }
            }
        }
        // A stencil mask or a short matte keeps the scalar path.
        let mut image = byte_image(3, 8);
        image.matte = Some(vec![1.0; 2]);
        image.mask = Some(Arc::new(byte_image(1, 8)));
        assert!(matches!(Texels::new(&image, Model::Rgb), Texels::Uncached));
    }

    #[test]
    fn mask_lookups_preserve_precision_decode_and_stencil_polarity() {
        for bits in 1..=8 {
            for inverted in [false, true] {
                let mut image = byte_image(1, bits);
                if inverted {
                    image.decode[0] = (1.0, 0.0);
                }
                let maximum = u32::from(u8::MAX >> (8 - bits));
                let mut samples = MaskSamples { table: [None; 256] };
                for pixel in (0..256).rev().chain(0..256) {
                    let raw = u32::try_from(pixel).unwrap() & maximum;
                    let numerator = if inverted { maximum - raw } else { raw };
                    let expected = u8::try_from((numerator * 255 + maximum / 2) / maximum).unwrap();
                    let alpha = samples.sample(&image, pixel).unwrap();
                    assert_eq!(super::super::quantize(alpha), expected);
                }
            }
        }
        for inverted in [false, true] {
            let mut image = byte_image(1, 1);
            image.decoded.kind = ImageKind::Stencil { inverted };
            image.decode[0] = (1.0, 0.0);
            let mut samples = MaskSamples { table: [None; 256] };
            for pixel in [0, 1, 0, 1] {
                let expected = if inverted { pixel } else { 1 - pixel };
                assert_eq!(
                    samples.sample(&image, pixel).unwrap(),
                    f64::from(u32::try_from(expected).unwrap())
                );
            }
        }
        let mut image = byte_image(1, 8);
        image.decoded.width = 3;
        image.decoded.bits_per_component = 16;
        image.decoded.samples = Samples::Words(vec![0, 32768, 65535]);
        let mut samples = MaskSamples { table: [None; 256] };
        for (pixel, expected) in [0, 128, 255].into_iter().enumerate() {
            assert_eq!(
                super::super::quantize(samples.sample(&image, pixel).unwrap()),
                expected
            );
        }
        assert!(samples.sample(&image, 3).is_err());
        image.decoded.width = 1;
        image.decoded.bits_per_component = 0;
        image.decoded.samples = Samples::Mixed {
            values: vec![128],
            bits: vec![8],
        };
        assert_eq!(
            super::super::quantize(samples.sample(&image, 0).unwrap()),
            128
        );
    }

    #[test]
    fn scalar_palette_tables_preserve_each_source_sample_exactly() {
        let mut image = byte_image(1, 8);
        let palette = |v: u8| [v, 255 - v, v.wrapping_mul(73)];
        image.space = Some(crate::color::Space::Indexed {
            base: Arc::new(crate::color::Space::Rgb),
            high: 255,
            lookup: (0..=255).flat_map(palette).collect::<Vec<_>>().into(),
        });
        image.decode[0] = (0.0, 255.0);
        let command = command(image);
        let mut tables = Texels::new(&command.image, Model::Rgb);
        assert!(matches!(tables, Texels::Cached(_)));
        let mut context = context();
        for raw in (0..=255_u8).rev().chain(0..=255) {
            let pixel = usize::from(raw);
            let actual = tables
                .sample(&mut context, &command, pixel, Model::Rgb, None)
                .unwrap();
            let scalar = context
                .image_texel(&command, pixel, Model::Rgb, None)
                .unwrap();
            assert_eq!(
                actual.color.map(f64::to_bits),
                scalar.color.map(f64::to_bits)
            );
            // Indexed lookup bytes are RGB components before averaging
            // (8.6.6.3; 8.9.5.2), independently fixing every expected channel.
            let expected = palette(raw);
            for (value, expected) in actual.color.iter().zip(expected) {
                assert_eq!(super::super::quantize(*value), expected);
            }
            assert_eq!(actual.alpha, 1.0);
            assert_eq!(actual.shape, 1.0);
        }
    }

    #[test]
    fn external_mask_metadata_preserves_exact_channel_tables() {
        let id = pagura_core::ObjectId::new(1, 0);
        for mask in [MaskKind::Stencil(id), MaskKind::Soft(id)] {
            for components in [1, 3] {
                let mut image = byte_image(components, 8);
                image.decoded.mask = mask.clone();
                let command = command(image);
                let mut tables = Texels::new(&command.image, Model::Rgb);
                assert!(matches!(tables, Texels::Gray { .. } | Texels::Rgb { .. }));
                let mut context = context();
                for pixel in 0..256 {
                    let value = tables
                        .sample(&mut context, &command, pixel, Model::Rgb, None)
                        .unwrap();
                    let scalar = context
                        .image_texel(&command, pixel, Model::Rgb, None)
                        .unwrap();
                    assert_eq!(
                        value.color.map(f64::to_bits),
                        scalar.color.map(f64::to_bits)
                    );
                    assert_eq!(value.alpha, 1.0);
                    assert_eq!(value.shape, 1.0);
                }
            }
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
