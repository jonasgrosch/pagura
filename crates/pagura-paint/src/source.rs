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

pub(crate) use image::{ImageBudgets, ImageSampler, ScratchLayout};
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

/// Mutable storage of prepared paints, owned by the caller: the paints of
/// one storage take turns with it ([`crate::plan::Slots`]).
#[derive(Debug, Default)]
pub(crate) struct PaintScratch {
    image: Option<Box<image::ImageScratch>>,
    /// The paint that used it last.
    owner: Option<u32>,
}

impl PaintScratch {
    /// Allocates or reuses capacities of `layout`, charging them to `bytes`.
    pub(crate) fn make(
        layout: Option<ScratchLayout>,
        previous: Option<Self>,
        bytes: &mut Meter,
    ) -> Result<Self> {
        bytes.charge(layout.map_or(0, ScratchLayout::bytes))?;
        if let Some(mut previous) = previous.filter(|previous| previous.fits(layout)) {
            previous.reset();
            return Ok(previous);
        }
        Ok(Self {
            image: layout
                .map(|layout| layout.make().map(Box::new))
                .transpose()?,
            owner: None,
        })
    }

    /// Whether the capacities match `layout`, without reusing its values.
    pub(crate) fn fits(&self, layout: Option<ScratchLayout>) -> bool {
        self.image.as_ref().map(|scratch| scratch.layout()) == layout
    }

    /// Takes the storage for `paint`: the caches of another paint's
    /// placement go, the colour memo of their shared image stays.
    pub(crate) fn claim(&mut self, paint: u32) {
        if self.owner != Some(paint) {
            if let Some(image) = &mut self.image {
                image.switch();
            }
            self.owner = Some(paint);
        }
    }

    /// Bytes retained by this paint's execution storage.
    pub(crate) fn bytes(&self) -> usize {
        self.image.as_ref().map_or(0, |scratch| scratch.bytes())
    }

    /// Invalidates source-dependent values before rendering.
    pub(crate) fn reset(&mut self) {
        if let Some(image) = &mut self.image {
            image.reset();
        }
        self.owner = None;
    }
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
    /// The part of the full range by which near-upright image sampling may
    /// move values in the buffer it paints into, as that buffer's storage
    /// admits (ADR 0004 §3); zero for exact sampling.
    pub(crate) upright: f64,
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
    /// How precisely the span's colours are kept.
    pub(crate) precision: Precision,
}

impl Pixels<'_> {
    /// A span without a shading-pattern boundary, kept exactly.
    pub(crate) fn new(x: u32, y: u32, len: usize) -> Self {
        Self {
            x,
            y,
            len,
            boundary: None,
            precision: Precision::Exact,
        }
    }
}

/// How precisely a span's colour values are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Precision {
    /// Every value as computed.
    Exact,
    /// Only each value's F32 level ([`crate::kernel::level`]), as a store
    /// into F32 levels keeps it: a value may be any other with the same
    /// level.
    Level,
}

impl Source {
    /// Adds the optional storage that speeds painting this source, charged
    /// to `bytes` where it has room once the page's own storage is: an
    /// image's curve values ([`ImageSampler::admit_curves`]).
    pub(crate) fn admit_optional(&mut self, bytes: &mut Meter) {
        if let Self::Image(image) = self {
            image.admit_curves(bytes);
        }
    }

    pub(crate) fn scratch_layout(&self) -> Option<ScratchLayout> {
        match self {
            Self::Image(image) => Some(image.scratch_layout()),
            Self::Solid { .. } | Self::Shading(_) => None,
        }
    }

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
        (functions, bytes): (&mut Meter, &Meter),
    ) -> Result<Self> {
        match paint {
            Paint::Solid(color) => Self::solid(color, context, (functions, bytes)),
            Paint::Shading(pattern) => {
                let source = ShadingSource::pattern(
                    &pattern.shading,
                    pattern.matrix.then(context.transform),
                    context,
                    pixels,
                    (functions, bytes),
                )?;
                Self::from_shading(source, bytes)
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
        budgets: (&mut Meter, &Meter),
    ) -> Result<Self> {
        let bytes = budgets.1;
        let source = ShadingSource::prepare(shading, shading_to_device, context, pixels, budgets)?;
        Self::from_shading(source, bytes)
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
        let ImageBudgets {
            bytes: remaining,
            work,
            functions,
        } = budgets;
        let sampler = ImageSampler::prepare_with(
            image,
            unit_to_device,
            context,
            pixels,
            stencil,
            &mut ImageBudgets {
                bytes: &mut *remaining,
                work,
                functions: &mut *functions,
            },
        )?;
        let mut bytes = Meter::new(remaining.remaining(), "render memory budget exceeded");
        bytes.charge(sampler.bytes())?;
        Ok(match sampler.constant(&mut bytes, functions)? {
            Some((color, painted)) => Self::Solid { color, painted },
            None => Self::Image(sampler),
        })
    }

    /// A solid colour, converted once.
    fn solid(
        color: &Color,
        context: &PaintContext<'_>,
        budgets: (&mut Meter, &Meter),
    ) -> Result<Self> {
        let (color, painted) = Self::solid_color(color, context, budgets)?;
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
        (functions, bytes): (&mut Meter, &Meter),
    ) -> Result<(Box<[f64]>, Painted)> {
        let mut bytes = Meter::new(bytes.remaining(), "render memory budget exceeded");
        let converter = Converter::new_metered(
            &color.space,
            context.target,
            &context.composite.conversion,
            &mut bytes,
        )?;
        functions.charge(converter.cost())?;
        let mut converted = Vec::new();
        bytes.reserve(&mut converted, context.target.channels())?;
        converted.resize(context.target.channels(), 0.0);
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

    fn from_shading(source: ShadingSource, bytes: &Meter) -> Result<Self> {
        let Some(color) = source.constant() else {
            return Ok(Self::Shading(source));
        };
        let mut bytes = Meter::new(bytes.remaining(), "render memory budget exceeded");
        bytes.charge(source.bytes())?;
        let mut converted = Vec::new();
        bytes.reserve(&mut converted, color.len())?;
        converted.extend_from_slice(color);
        Ok(Self::Solid {
            color: converted.into(),
            painted: source.painted(),
        })
    }

    /// The colour of every pixel and the channels it paints for a solid.
    pub(crate) fn constant(&self) -> Option<(&[f64], Painted)> {
        match self {
            Self::Solid { color, painted } => Some((color, *painted)),
            Self::Image(_) | Self::Shading(_) => None,
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
    pub(crate) fn fill(
        &self,
        pixels: Pixels<'_>,
        scratch: &mut PaintScratch,
        out: &mut SpanColors,
        shape: &mut Vec<f64>,
    ) {
        let Pixels {
            x,
            y,
            len,
            boundary,
            precision,
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
                Some(boundary) => shading.span_with_boundary(x, y, boundary, precision, out),
                None => shading.span(x, y, len, precision, out),
            },
            Self::Image(image) => {
                let Some(scratch) = scratch.image.as_deref_mut() else {
                    debug_assert!(false, "image storage was fitted before execution");
                    return;
                };
                if image.shape_is_alpha() {
                    image.span(pixels, scratch, out);
                } else {
                    image.span_with_shape(pixels, scratch, out, shape);
                }
            }
        }
    }

    /// Writes into `out` the N16 words a store at full opacity writes for
    /// device pixels `x .. x + out.len()` of row `y`, where the source gives
    /// them without forming colours first (opaque images sampled nearest);
    /// false otherwise.
    pub(crate) fn words(
        &self,
        pixels: Pixels<'_>,
        scratch: &mut PaintScratch,
        out: &mut [[u16; 4]],
    ) -> bool {
        match self {
            Self::Image(image) => scratch
                .image
                .as_deref_mut()
                .is_some_and(|scratch| image.words(pixels, scratch, out)),
            Self::Solid { .. } | Self::Shading(_) => false,
        }
    }

    /// Render work units filling `region` costs besides compositing (mesh
    /// geometry tests), counted until it passes `limit`.
    pub(crate) fn geometry_work(&self, region: DeviceRect, limit: usize) -> usize {
        match self {
            Self::Shading(shading) => shading.geometry_work(region, limit),
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

/// Copies the first values of `from` into `to`, as many as both hold: a
/// fixed-size move for the common counts, where a copy call per texel
/// or sample costs more than the copy.
#[inline]
pub(crate) fn copy_values(to: &mut [f64], from: &[f64]) {
    #[inline]
    fn fixed<const N: usize>(to: &mut [f64], from: &[f64]) {
        if let (Some(to), Some(from)) = (to.first_chunk_mut::<N>(), from.first_chunk::<N>()) {
            *to = *from;
        }
    }
    match to.len().min(from.len()) {
        0 => {}
        1 => fixed::<1>(to, from),
        2 => fixed::<2>(to, from),
        3 => fixed::<3>(to, from),
        4 => fixed::<4>(to, from),
        5 => fixed::<5>(to, from),
        6 => fixed::<6>(to, from),
        n => to[..n].copy_from_slice(&from[..n]),
    }
}
