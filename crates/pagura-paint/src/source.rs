//! Prepared paint (ADR 0034): a closed set of colour sources chosen once
//! per paint and render, each filling spans of working colours or reporting
//! a constant. Colour conversion runs through prepared converters and
//! functions are total, so filling a span cannot fail.
//!
//! Transfer functions (10.5) are not part of prepared paint: as before, the
//! executor selects, per output pixel, the transfer of the last opaque
//! elementary mark that fully covers it (11.7.5.2) and applies it to the
//! final colour, so sources always carry untransferred colours.

use pagura_color::{Converter, Painted, Target};
use pagura_core::{Limits, Meter, Result, Store};
use pagura_interpret::{Color, Composite, Paint};
use pagura_raster::Transform;

mod image;
mod shading;

pub use image::ImageSampler;
pub use shading::{BBoxUse, ShadingSource};

/// Working colours of one span: `channels` values per pixel in the target's
/// order (additive channels as intensity, subtractive as `1 − tint`) and
/// one alpha per pixel.
#[derive(Debug, Default)]
pub struct SpanColors {
    /// Values per pixel.
    pub channels: usize,
    /// `len × channels` colour values.
    pub colors: Vec<f64>,
    /// `len` alpha values; empty when every pixel is opaque.
    pub alpha: Vec<f64>,
    /// The channels the colours paint, for overprint.
    pub painted: Painted,
}

/// What preparing a paint needs.
#[derive(Debug, Clone, Copy)]
pub struct PaintContext<'a> {
    /// Paint space (default user space, or pattern space composed by the
    /// caller) to device pixels.
    pub transform: Transform,
    /// The blending space and spot channels of the buffer it paints into.
    pub target: &'a Target,
    /// The mark's composite state (intent, BG and UCR, transfer).
    pub composite: &'a Composite,
    /// Cache of decoded images and prepared functions.
    pub store: &'a Store,
    /// Budgets.
    pub limits: &'a Limits,
    /// Whether images may be decoded near their drawn size.
    pub reduced_image_decode: bool,
    /// The device pixels the paint is sampled in: the render's region
    /// intersected with the mark's bounds. Sources may skip what lies
    /// outside (rows of an image's decode, for example).
    pub region: pagura_raster::DeviceRect,
}

/// A prepared colour source.
#[derive(Debug)]
pub enum Source {
    /// One colour everywhere.
    Solid {
        /// The colour, `target.channels()` values.
        color: Box<[f64]>,
        /// The channels it paints.
        painted: Painted,
    },
    /// A shading.
    Shading(ShadingSource),
    /// An image's colours and alpha.
    Image(ImageSampler),
}

impl Source {
    /// Prepares `paint` (a solid colour, shading or image; tiling patterns
    /// are prepared by the executor) and charges `function_work` for every
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
    /// Returns `InvalidArgument` for a tiling pattern or a solid in a
    /// Pattern space, `Unsupported` for colour the conversion cannot perform
    /// or a singular shading transform, and a `Limit` error when the
    /// reservation exceeds `functions`.
    pub fn prepare(
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
                Ok(Self::collapse(source))
            }
            Paint::Tiling(_) => Err(pagura_core::Error::invalid_argument(
                "tiling patterns are prepared by the executor",
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
    pub fn shading(
        shading: &pagura_interpret::Shading,
        shading_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        functions: &mut Meter,
    ) -> Result<Self> {
        let source =
            ShadingSource::prepare(shading, shading_to_device, context, pixels, functions)?;
        Ok(Self::collapse(source))
    }

    /// A solid colour, converted once.
    fn solid(color: &Color, context: &PaintContext<'_>, functions: &mut Meter) -> Result<Self> {
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
        Ok(Self::Solid {
            color: converted.into(),
            painted,
        })
    }

    fn collapse(source: ShadingSource) -> Self {
        match source.constant() {
            Some(color) => Self::Solid {
                color: color.into(),
                painted: source.painted(),
            },
            None => Self::Shading(source),
        }
    }

    /// The constant colour, when every pixel has the same one.
    pub fn constant(&self) -> Option<&[f64]> {
        match self {
            Self::Solid { color, .. } => Some(color),
            Self::Shading(_) | Self::Image(_) => None,
        }
    }

    /// Fills `out` with the colours of device pixels `x .. x + len` of row
    /// `y` (sampled at pixel centres, or over the pixel area where the
    /// contract asks for an average).
    pub fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
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
            Self::Shading(shading) => shading.span(x, y, len, out),
            Self::Image(image) => image.span(x, y, len, out),
        }
    }
}
