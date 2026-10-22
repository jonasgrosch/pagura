//! Prepared paint (ADR 0034 §4): a closed set of colour sources chosen once
//! per paint and render, each filling spans of working colours or reporting
//! a constant. Colour conversion runs through prepared converters and
//! functions are total, so filling a span cannot fail. A shading that
//! gives every pixel one opaque colour is a solid from preparation on.
//!
//! Transfer functions (10.5) are not part of prepared paint: the executor
//! selects, per output pixel, the transfer of the last opaque elementary
//! mark that fully covers it (11.7.5.2) and applies it to the final colour,
//! so sources always carry untransferred colours.

use pagura_color::{Converter, Painted, Target};
use pagura_core::{Diagnostic, Limits, Meter, Result, Store};
use pagura_interpret::{Color, Composite, Image, Paint};
use pagura_raster::{DeviceRect, Transform};

mod image;
mod shading;

pub(crate) use image::{ImageBudgets, ImageSampler};
pub(crate) use shading::{BBoxUse, ShadingSource};

/// Working colours of one span: `channels` values per pixel in the target's
/// order (additive channels as intensity, subtractive as `1 − tint`) and
/// one alpha per pixel.
#[derive(Debug, Default)]
pub(crate) struct SpanColors {
    /// Values per pixel.
    pub(crate) channels: usize,
    /// `len × channels` colour values.
    pub(crate) colors: Vec<f64>,
    /// `len` alpha values; empty when every pixel is opaque.
    pub(crate) alpha: Vec<f64>,
    /// The channels the colours paint, for overprint.
    pub(crate) painted: Painted,
}

/// What preparing a paint needs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaintContext<'a> {
    /// Paint space (default user space, or pattern space composed by the
    /// caller) to device pixels.
    pub(crate) transform: Transform,
    /// The blending space and spot channels of the buffer it paints into.
    pub(crate) target: &'a Target,
    /// The mark's composite state (intent, BG and UCR, transfer).
    pub(crate) composite: &'a Composite,
    /// Cache of decoded images and prepared functions.
    pub(crate) store: &'a Store,
    /// Budgets.
    pub(crate) limits: &'a Limits,
    /// The device pixels the paint is sampled in: the render's region
    /// intersected with the mark's bounds. Sources may skip what lies
    /// outside (rows of an image's decode, for example).
    pub(crate) region: DeviceRect,
}

/// A prepared colour source.
#[derive(Debug)]
pub(crate) enum Source {
    /// One opaque colour everywhere.
    Solid {
        /// The colour, `target.channels()` values.
        color: Box<[f64]>,
        /// The channels it paints.
        painted: Painted,
    },
    /// A shading.
    Shading(ShadingSource),
    /// An image's colours and alpha, or a stencil mask's alpha with its
    /// colour.
    Image(ImageSampler),
}

/// The pixels of one span of paint.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pixels<'a> {
    /// The first pixel's column.
    pub(crate) x: u32,
    /// The row.
    pub(crate) y: u32,
    /// Pixels.
    pub(crate) len: usize,
    /// For a shading pattern whose degenerate `BBox` outline mixes its
    /// `Background` in ([`BBoxUse::Boundary`]): the outline's coverage of
    /// each pixel.
    pub(crate) boundary: Option<&'a [u8]>,
}

impl Source {
    /// Prepares `paint` (a solid colour or shading pattern; tiling patterns
    /// are prepared as groups) and charges `function_work` for every
    /// evaluation filling `pixels` device pixels will need.
    ///
    /// A solid colour is converted here, once. A shading pattern
    /// ([`Paint::Shading`]) is prepared with [`ShadingSource::pattern`]
    /// under its matrix composed with `context.transform`, and becomes a
    /// solid when it paints one opaque colour everywhere.
    ///
    /// The channels a source paints follow the mark's overprint state: every
    /// channel without overprinting; with it, the channels the colour
    /// specifies, without the zero components of a DeviceCMYK solid under
    /// `OPM 1` (8.6.7, 11.7.4.3).
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a tiling pattern, `Unsupported` for
    /// colour the conversion cannot perform or a singular shading
    /// transform, and a `Limit` error when the reservation exceeds
    /// `functions`.
    pub(crate) fn prepare(
        paint: &Paint,
        context: &PaintContext<'_>,
        pixels: usize,
        functions: &mut Meter,
    ) -> Result<Self> {
        match paint {
            Paint::Solid(color) => Self::solid(color, context, functions),
            Paint::Shading(pattern) => {
                let source = ShadingSource::pattern(
                    &pattern.shading,
                    pattern.matrix.then(context.transform),
                    context,
                    pixels,
                    functions,
                )?;
                Ok(Self::from_shading(source))
            }
            Paint::Tiling(_) => Err(pagura_core::Error::invalid_argument(
                "tiling patterns are prepared as groups",
            )),
        }
    }

    /// Prepares the shading of an `sh` mark under `shading_to_device`, as
    /// [`ShadingSource::prepare`] does, collapsing it to a solid when it
    /// paints one opaque colour everywhere.
    ///
    /// # Errors
    ///
    /// As [`ShadingSource::prepare`].
    pub(crate) fn shading(
        shading: &pagura_interpret::Shading,
        shading_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        functions: &mut Meter,
    ) -> Result<Self> {
        let source =
            ShadingSource::prepare(shading, shading_to_device, context, pixels, functions)?;
        Ok(Self::from_shading(source))
    }

    /// Prepares `image` drawn under `unit_to_device` as
    /// [`ImageSampler::prepare_with`] does; a stencil mask takes its solid
    /// `stencil` colour into its texels, so that they composite exactly.
    ///
    /// # Errors
    ///
    /// As [`ImageSampler::prepare_with`].
    pub(crate) fn image(
        image: &Image,
        unit_to_device: Transform,
        stencil: Option<(&[f64], Painted)>,
        (context, pixels): (&PaintContext<'_>, usize),
        budgets: ImageBudgets<'_>,
    ) -> Result<Self> {
        let sampler = ImageSampler::prepare_with(image, unit_to_device, context, pixels, budgets)?;
        Ok(Self::Image(match stencil {
            Some((color, painted)) => sampler.with_stencil_color(color, painted),
            None => sampler,
        }))
    }

    /// A solid colour, converted once.
    fn solid(color: &Color, context: &PaintContext<'_>, functions: &mut Meter) -> Result<Self> {
        let (color, painted) = Self::solid_color(color, context, functions)?;
        Ok(Self::Solid { color, painted })
    }

    /// The working colour of `color` and the channels it paints, converted
    /// once.
    ///
    /// # Errors
    ///
    /// `Unsupported` for colour the conversion cannot perform, and a `Limit`
    /// error when the conversion exceeds `functions`.
    pub(crate) fn solid_color(
        color: &Color,
        context: &PaintContext<'_>,
        functions: &mut Meter,
    ) -> Result<(Box<[f64]>, Painted)> {
        let converter =
            Converter::new(&color.space, context.target, &context.composite.conversion)?;
        functions.charge(converter.cost())?;
        let mut converted = vec![0.0; context.target.channels()];
        let specified = converter.convert(&color.components, &mut converted);
        let overprint = context.composite.overprint;
        let painted = if !color.space.paints() {
            Painted(0)
        } else if !overprint.enabled {
            Painted::all(context.target.channels())
        } else if overprint.nonzero {
            converter.nonzero(&color.components)
        } else {
            specified
        };
        Ok((converted.into(), painted))
    }

    fn from_shading(source: ShadingSource) -> Self {
        match source.constant() {
            Some(color) => Self::Solid {
                color: color.into(),
                painted: source.painted(),
            },
            None => Self::Shading(source),
        }
    }

    /// The colour of every pixel and the channels it paints, when every
    /// pixel has the same opaque one: a solid, or an image of one texel
    /// sampled nearest.
    pub(crate) fn constant(&self) -> Option<(&[f64], Painted)> {
        match self {
            Self::Solid { color, painted } => Some((color, *painted)),
            Self::Image(image) => image.uniform(),
            Self::Shading(_) => None,
        }
    }

    /// Whether every pixel's shape is its alpha; otherwise [`Self::fill`]
    /// gives each pixel's shape too (images with a soft mask or embedded
    /// opacity whose alpha is not shape).
    pub(crate) fn shape_is_alpha(&self) -> bool {
        match self {
            Self::Image(image) => image.shape_is_alpha(),
            Self::Solid { .. } | Self::Shading(_) => true,
        }
    }

    /// Fills `out` with the straight colours and alpha of `pixels`, sampled
    /// at pixel centres, or over the pixel area where the contract asks for
    /// an average, and `shape` with their shapes where
    /// [`Self::shape_is_alpha`] is false.
    pub(crate) fn fill(&self, pixels: Pixels<'_>, out: &mut SpanColors, shape: &mut Vec<f64>) {
        let Pixels {
            x,
            y,
            len,
            boundary,
        } = pixels;
        match self {
            Self::Solid { color, painted } => {
                out.channels = color.len();
                out.colors.clear();
                for _ in 0..len {
                    out.colors.extend_from_slice(color);
                }
                out.alpha.clear();
                out.painted = *painted;
            }
            Self::Shading(shading) => match boundary {
                Some(boundary) => shading.span_with_boundary(x, y, boundary, out),
                None => shading.span(x, y, len, out),
            },
            Self::Image(image) if image.shape_is_alpha() => image.span(x, y, len, out),
            Self::Image(image) => image.span_with_shape(x, y, len, out, shape),
        }
    }

    /// Writes into `out` the N16 words a store at full opacity writes for
    /// device pixels `x .. x + out.len()` of row `y`, where the source gives
    /// them without forming colours first (opaque images sampled nearest);
    /// false otherwise.
    pub(crate) fn words(&self, x: u32, y: u32, out: &mut [[u16; 4]]) -> bool {
        match self {
            Self::Image(image) => image.words(x, y, out),
            Self::Solid { .. } | Self::Shading(_) => false,
        }
    }

    /// Render work units one pixel costs besides compositing (mesh
    /// geometry tests).
    pub(crate) fn work_per_pixel(&self) -> usize {
        match self {
            Self::Shading(shading) => shading.geometry_work(),
            Self::Solid { .. } | Self::Image(_) => 0,
        }
    }

    /// Bytes the source holds besides its own value.
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Self::Solid { color, .. } => size_of_val::<[f64]>(color),
            Self::Shading(shading) => shading.bytes(),
            Self::Image(image) => image.bytes(),
        }
    }

    /// Repairs preparing the source made, to report in source order.
    pub(crate) fn repairs(&self) -> &[Diagnostic] {
        match self {
            Self::Image(image) => image.repairs(),
            Self::Solid { .. } | Self::Shading(_) => &[],
        }
    }
}
