//! The colour, alpha and shape a mark has at each device pixel: a solid
//! colour, a shading sampled at pixel centres (or 2×2 sub-pixel centres with
//! `AntiAlias`), or an image's samples.

use super::{Context, Renderer};
use crate::color::{Channels, MAX, Prepared};
use crate::image::{ColorSamples, Magnify, Sampler, Values, opacity};
use crate::shading;
use pagura_color::Painted;
use pagura_core::{Error, Result};
use pagura_interpret::{Composite, ImageMask, ImageNode, Paint, Shading};
use pagura_raster::{Point, Transform};
use std::rc::Rc;

/// The most values the reference holds for one image's samples.
const MAX_IMAGE_VALUES: usize = 1 << 28;

/// What a mark contributes at one pixel before coverage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sample {
    /// Straight colour in the target's channels.
    pub(crate) color: Channels,
    /// Alpha.
    pub(crate) alpha: f64,
    /// Shape.
    pub(crate) shape: f64,
    /// Channels left to the backdrop.
    pub(crate) preserve: u64,
}

impl Sample {
    /// Black, opaque, painting every channel.
    pub(crate) fn opaque() -> Self {
        Self {
            color: [0.0; MAX],
            alpha: 1.0,
            shape: 1.0,
            preserve: 0,
        }
    }
}

/// Whether a shading is a mark's paint (`sh`) or a pattern's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Use {
    /// Painted with `sh`: `Background` is ignored.
    Mark,
    /// A shading pattern: `Background` fills where the shading does not.
    Pattern,
}

/// A prepared per-pixel colour source.
#[derive(Debug)]
pub(crate) enum Sources<'a> {
    /// One colour everywhere.
    Solid(Sample),
    /// A shading.
    Shading(ShadingSamples<'a>),
    /// An image.
    Image(ImageSamples),
}

impl Sources<'_> {
    /// The sample at `(x, y)`; `None` where the source paints nothing.
    pub(crate) fn sample(&self, x: u32, y: u32) -> Option<Sample> {
        match self {
            Self::Solid(sample) => Some(*sample),
            Self::Shading(shading) => shading.sample(x, y),
            Self::Image(image) => Some(image.sample(x, y)),
        }
    }
}

/// A shading prepared for one mark.
#[derive(Debug)]
pub(crate) struct ShadingSamples<'a> {
    sampler: shading::Sampler<'a>,
    /// Device space to shading space.
    inverse: Transform,
    convert: Rc<Prepared>,
    channels: usize,
    rate: u32,
    background: Option<Channels>,
    preserve: u64,
}

impl ShadingSamples<'_> {
    fn sample(&self, x: u32, y: u32) -> Option<Sample> {
        let shading = self.sampler.shading();
        let n = shading.space.components();
        let mut components = [0.0; pagura_color::MAX_COMPONENTS];
        let (color, shape) = supersample(
            (x, y),
            self.rate,
            self.inverse,
            self.channels,
            |point, out| {
                let inside = || {
                    shading.bbox.is_none_or(|b| {
                        point.x >= b.x0()
                            && point.x <= b.x1()
                            && point.y >= b.y0()
                            && point.y <= b.y1()
                    })
                };
                if (self.background.is_none() || inside())
                    && self.sampler.sample(point, &mut components)
                {
                    self.convert.convert(&components[..n], out);
                    true
                } else if let Some(background) = &self.background {
                    out.copy_from_slice(&background[..self.channels]);
                    true
                } else {
                    false
                }
            },
        )?;
        Some(Sample {
            color,
            alpha: shape,
            shape,
            preserve: self.preserve,
        })
    }
}

/// The mean colour of the `rate × rate` sub-pixel centres of pixel `(x, y)`
/// (mapped by `inverse`) where `sample` paints, and the fraction it paints;
/// `None` where it paints none.
pub(crate) fn supersample(
    (x, y): (u32, u32),
    rate: u32,
    inverse: Transform,
    channels: usize,
    mut sample: impl FnMut(Point, &mut [f64]) -> bool,
) -> Option<(Channels, f64)> {
    let step = f64::from(rate);
    let mut sum = [0.0; MAX];
    let mut color = [0.0; MAX];
    let mut covered = 0_u32;
    for j in 0..rate {
        for i in 0..rate {
            let point = Point::new(
                f64::from(x) + (f64::from(i) + 0.5) / step,
                f64::from(y) + (f64::from(j) + 0.5) / step,
            ) * inverse;
            if sample(point, &mut color[..channels]) {
                for (sum, v) in sum.iter_mut().zip(&color).take(channels) {
                    *sum += v;
                }
                covered += 1;
            }
        }
    }
    if covered == 0 {
        return None;
    }
    let count = f64::from(covered);
    for v in sum.iter_mut().take(channels) {
        *v /= count;
    }
    Some((sum, count / (step * step)))
}

/// An image prepared for one mark.
#[derive(Debug)]
pub(crate) struct ImageSamples {
    sampler: Sampler,
    values: Values,
    /// For a stencil mask, its colour; samples are then opacities.
    stencil: Option<Channels>,
    mask: Option<MaskSamples>,
    channels: usize,
    preserve: u64,
}

/// An image's stencil or soft mask (8.9.6.3, 11.6.5.3).
#[derive(Debug)]
struct MaskSamples {
    sampler: Sampler,
    values: Values,
    /// The mask scales shape as well as alpha: a stencil mask, or a soft
    /// mask under alpha-is-shape.
    scales_shape: bool,
}

impl ImageSamples {
    fn sample(&self, x: u32, y: u32) -> Sample {
        let n = self.channels;
        let mut sample = Sample::opaque();
        sample.preserve = self.preserve;
        if let Some(color) = &self.stencil {
            let mut opacity = [0.0];
            self.sampler.sample(&self.values, (x, y), &mut opacity);
            sample.color = *color;
            (sample.alpha, sample.shape) = (opacity[0], opacity[0]);
            return sample;
        }
        let mut values = [0.0; MAX + 2];
        self.sampler
            .sample(&self.values, (x, y), &mut values[..n + 2]);
        let (mut alpha, mut shape) = (values[n], values[n + 1]);
        if let Some(mask) = &self.mask {
            let mut factor = [0.0];
            mask.sampler.sample(&mask.values, (x, y), &mut factor);
            for v in values.iter_mut().take(n) {
                *v *= factor[0];
            }
            alpha *= factor[0];
            if mask.scales_shape {
                shape *= factor[0];
            }
        }
        for (out, &v) in sample.color.iter_mut().zip(&values[..n]) {
            *out = if alpha == 1.0 {
                v
            } else if alpha > 0.0 {
                v / alpha
            } else {
                0.0
            };
        }
        sample.alpha = alpha;
        sample.shape = shape;
        sample
    }
}

impl Renderer<'_> {
    /// Prepares `shading` drawn under `to_device` (shading space to device).
    pub(super) fn shading_source<'a>(
        &mut self,
        shading: &'a Shading,
        to_device: Transform,
        state: &Composite,
        context: &Context<'_>,
        usage: Use,
    ) -> Result<Sources<'a>> {
        let inverse = to_device.inverse().ok_or(Error::invalid_argument(
            "the reference has no singular shading transform",
        ))?;
        let target = context.target;
        let convert = self
            .colors
            .prepare(&shading.space, target, &state.conversion)?;
        let mut probe = [0.0; MAX];
        let painted = convert.convert(&shading.space.initial(), &mut probe[..target.channels()]);
        let background = match (usage, &shading.background) {
            (Use::Pattern, Some(components)) => {
                let mut color = [0.0; MAX];
                convert.convert(components, &mut color[..target.channels()]);
                Some(color)
            }
            _ => None,
        };
        Ok(Sources::Shading(ShadingSamples {
            sampler: shading::Sampler::new(shading)?,
            inverse,
            convert,
            channels: target.channels(),
            rate: if shading.antialias { 2 } else { 1 },
            background,
            preserve: self.preserve(state, painted, target, None),
        }))
    }

    /// Prepares an image (or stencil mask painted with `stencil`) drawn
    /// under `to_device` (its unit square to device).
    pub(super) fn image_source(
        &mut self,
        node: &ImageNode,
        stencil: Option<&Paint>,
        to_device: Transform,
        state: &Composite,
        context: &Context<'_>,
    ) -> Result<Sources<'static>> {
        let image = node.image.as_ref();
        let decoded = self.decoded(image)?;
        let dims = (decoded.width, decoded.height);
        let magnify = if image.interpolate {
            Magnify::Bilinear
        } else {
            Magnify::Nearest
        };
        let sampler = Sampler::new(dims, magnify, to_device)
            .ok_or(Error::invalid_argument("singular image"))?;
        let target = context.target;
        let channels = target.channels();
        let Some(space) = &image.space else {
            let Some(Paint::Solid(color)) = stencil else {
                return Err(Error::invalid_argument(
                    "the reference has no pattern-coloured stencil masks",
                ));
            };
            let solid = self.solid(color, state, target)?;
            let values = Values::build(dims, 1, MAX_IMAGE_VALUES, |i, out| {
                out[0] = opacity(image, &decoded, i)?;
                Ok(())
            })?;
            return Ok(Sources::Image(ImageSamples {
                sampler,
                values,
                stencil: Some(solid.color),
                mask: None,
                channels,
                preserve: solid.preserve,
            }));
        };
        let mut conversion = state.conversion.clone();
        if let Some(intent) = image.intent {
            conversion.intent = intent;
        }
        let convert = self.colors.prepare(space, target, &conversion)?;
        let mut probe = [0.0; MAX];
        let painted: Painted = convert.convert(&space.initial(), &mut probe[..channels]);
        let mask_image = match &image.mask {
            ImageMask::Stencil(mask) | ImageMask::Soft { mask, .. } => {
                Some(Rc::clone(&self.decoded(mask)?))
            }
            _ => None,
        };
        let matte = match (&image.mask, &mask_image) {
            (
                ImageMask::Soft {
                    mask,
                    matte: Some(matte),
                },
                Some(decoded),
            ) => Some((&matte[..], mask.as_ref(), decoded.as_ref())),
            _ => None,
        };
        let samples = ColorSamples {
            image,
            decoded: &decoded,
            matte,
            convert: &convert,
            channels,
            alpha_is_shape: state.alpha_is_shape,
        };
        let values = Values::build(dims, channels + 2, MAX_IMAGE_VALUES, |i, out| {
            samples.value(i, out)
        })?;
        let mask = match (&image.mask, &mask_image) {
            (ImageMask::Stencil(mask) | ImageMask::Soft { mask, .. }, Some(decoded)) => {
                let mask_magnify = if mask.interpolate {
                    Magnify::Bilinear
                } else {
                    Magnify::Nearest
                };
                let sampler =
                    Sampler::new((decoded.width, decoded.height), mask_magnify, to_device)
                        .ok_or(Error::invalid_argument("singular image mask"))?;
                let values = Values::build(
                    (decoded.width, decoded.height),
                    1,
                    MAX_IMAGE_VALUES,
                    |i, out| {
                        out[0] = opacity(mask, decoded, i)?;
                        Ok(())
                    },
                )?;
                Some(MaskSamples {
                    sampler,
                    values,
                    scales_shape: matches!(image.mask, ImageMask::Stencil(_))
                        || state.alpha_is_shape,
                })
            }
            _ => None,
        };
        Ok(Sources::Image(ImageSamples {
            sampler,
            values,
            stencil: None,
            mask,
            channels,
            preserve: self.preserve(state, painted, target, None),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anti_aliasing_averages_the_painted_sub_pixel_centres() {
        // The triangle x + y <= 4: pixel (1, 2) has the sub-pixel centres
        // (1.25, 2.25), (1.75, 2.25), (1.25, 2.75) inside or on the edge and
        // (1.75, 2.75) outside.
        let inside = |p: Point, out: &mut [f64]| {
            out[0] = p.x;
            p.x + p.y <= 4.0
        };
        let (color, shape) = supersample((1, 2), 2, Transform::IDENTITY, 1, inside).unwrap();
        assert_eq!(shape, 0.75);
        assert!((color[0] - (1.25 + 1.75 + 1.25) / 3.0).abs() < 1e-15);
        let (color, shape) = supersample((3, 0), 1, Transform::IDENTITY, 1, inside).unwrap();
        assert_eq!((color[0], shape), (3.5, 1.0));
        assert!(supersample((3, 3), 2, Transform::IDENTITY, 1, inside).is_none());
    }
}
