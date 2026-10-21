//! Prepared paint (ADR 0034): a closed set of colour sources chosen once
//! per paint and render, each filling spans of working colours or reporting
//! a constant. Colour conversion runs through prepared converters and
//! functions are total, so filling a span cannot fail.

use pagura_color::{Painted, Target};
use pagura_core::{Error, Limits, Meter, Result, Store};
use pagura_interpret::{Composite, Paint};
use pagura_raster::Transform;

mod image;
mod shading;

pub use image::ImageSampler;
pub use shading::ShadingSource;

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
    /// # Errors
    ///
    /// Returns `Unsupported` for colour the conversion cannot perform and a
    /// `Limit` error when the reservation exceeds `functions`.
    pub fn prepare(
        paint: &Paint,
        context: &PaintContext<'_>,
        pixels: usize,
        functions: &mut Meter,
    ) -> Result<Self> {
        let _ = (paint, context, pixels, functions);
        Err(Error::invalid_argument("prepared paint is not implemented"))
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
