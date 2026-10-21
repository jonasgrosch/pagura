//! The image sampler family (8.9.5, ADR 0004): nearest samples and
//! exact-area averages, with decode arrays, Indexed lookups, colour keys,
//! stencil and soft masks and Matte, over images decoded on request at the
//! reduction and region painting needs.
//!
//! The arithmetic is the exact general route's: each source sample becomes
//! a texel (premultiplied colour, alpha and shape) through a converter
//! prepared once; texels sum with the footprint's weights in a fixed order;
//! a mask's opacity, sampled over its own grid, multiplies the sum; and the
//! colour is unpremultiplied for the executor.

mod geometry;
#[cfg(test)]
mod tests;
mod texels;

use super::{PaintContext, SpanColors};
use geometry::Geometry;
use pagura_color::{ColorSpace, Converter, Painted, Process};
use pagura_core::{Built, Diagnostic, Error, Meter, Result, StoreKey};
use pagura_image::{
    DecodeRequest, Decoded, DecodedImage, ImageId, ImageKind, ImageSource, Layout, MaskKind,
    Reduce, Samples,
};
use pagura_interpret::{Image, ImageMask};
use pagura_raster::Transform;
use std::sync::Arc;
use texels::{
    ByteCodes, Colors, Convert, Lookup, Opacity, OpacityTexels, PackedCodes, RgbTexels,
    SampleTexels, Strategy, TableTexels,
};

/// An image prepared for one render: decoded (through the store) and ready
/// to sample. A stencil mask samples alpha only; the executor colours it
/// with its paint, or [`ImageSampler::with_stencil_color`] does.
#[derive(Debug)]
pub struct ImageSampler {
    prepared: Option<Box<Prepared>>,
    channels: usize,
}

/// The budgets preparing an image charges.
#[derive(Debug)]
pub struct ImageBudgets<'a> {
    /// Render work: one unit per texel the filled pixels read.
    pub work: &'a mut Meter,
    /// `function_work` of the colour conversions sampling makes.
    pub functions: &'a mut Meter,
}

#[derive(Debug)]
struct Prepared {
    image: Arc<Decoded>,
    geometry: Geometry,
    paint: Paint,
    mask: Option<MaskSampling>,
    painted: Painted,
    /// Whether shape equals alpha, rather than the image's own shape.
    shape_is_alpha: bool,
}

#[derive(Debug)]
enum Paint {
    /// A stencil mask: per code its texel, `[colour × alpha.., alpha,
    /// alpha]`, with no colour until one is given.
    Stencil { table: Vec<f64>, opacity: [f64; 2] },
    /// A colour image.
    Colors(Box<(Colors, Strategy)>),
}

#[derive(Debug)]
struct MaskSampling {
    image: Arc<Decoded>,
    geometry: Geometry,
    opacity: Opacity,
    /// Whether the mask's opacity also scales shape: an explicit stencil
    /// mask, or alpha is shape.
    scales_shape: bool,
}

/// A decoded image in the store: the source, the reduction, the region
/// and the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DecodedKey {
    image: ImageId,
    request: DecodeRequest,
}

impl StoreKey for DecodedKey {
    type Value = Decoded;
}

impl ImageSampler {
    /// Prepares `image` drawn under `unit_to_device` (the image's unit
    /// square to device pixels), decoding what the region needs, and
    /// charges `work` for the texels `pixels` filled device pixels read.
    /// Colour conversions are charged to a `function_work` budget of their
    /// own; [`Self::prepare_with`] charges a caller's.
    ///
    /// # Errors
    ///
    /// Returns the decode's errors and a `Limit` error when decoding or
    /// sampling exceeds the budgets.
    pub fn prepare(
        image: &Image,
        unit_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        work: &mut Meter,
    ) -> Result<Self> {
        let mut functions = Meter::new(
            context.limits.function_work,
            "image colour conversion work budget exceeded",
        );
        Self::prepare_with(
            image,
            unit_to_device,
            context,
            pixels,
            ImageBudgets {
                work,
                functions: &mut functions,
            },
        )
    }

    /// As [`Self::prepare`], charging colour conversions to
    /// `budgets.functions`.
    ///
    /// # Errors
    ///
    /// As [`Self::prepare`].
    pub fn prepare_with(
        image: &Image,
        unit_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        mut budgets: ImageBudgets<'_>,
    ) -> Result<Self> {
        if !unit_to_device.is_finite() {
            return Err(Error::limit("image transform overflow"));
        }
        let channels = if image.space.is_some() {
            context.target.channels()
        } else {
            0
        };
        let empty = Self {
            prepared: None,
            channels,
        };
        let Some(inverse) = unit_to_device.inverse() else {
            return Ok(empty);
        };
        if image.space.as_deref().is_some_and(|space| !space.paints()) {
            return Ok(empty);
        }
        let decoded = decode(&image.source, unit_to_device, context)?;
        let (mask, description) = match &image.mask {
            ImageMask::Stencil(mask) => (Some(mask), MaskDescription::Stencil),
            ImageMask::Soft { mask, matte } => (
                Some(mask),
                MaskDescription::Soft {
                    decode: mask.decode.first().copied().unwrap_or([0.0, 1.0]),
                    matte: matte.as_deref(),
                },
            ),
            ImageMask::None | ImageMask::ColorKey(_) | ImageMask::Embedded => {
                (None, MaskDescription::None)
            }
        };
        let mask = mask
            .map(|mask| decode_request(&mask.source, Reduce::Full, context))
            .transpose()?;
        let description = Description {
            space: image.space.as_deref(),
            decode: &image.decode,
            key: match &image.mask {
                ImageMask::ColorKey(ranges) => Some(ranges),
                _ => None,
            },
            mask: description,
            alpha_is_shape: context.composite.alpha_is_shape,
        };
        let convert = match description.space {
            Some(space) => {
                let mut conversion = context.composite.conversion.clone();
                if let Some(intent) = image.intent {
                    conversion.intent = intent;
                }
                let base = Lookup::of(space).map_or(space, |(_, base)| base);
                let converter: Box<dyn Convert> =
                    Box::new(Converter::new(base, context.target, &conversion)?);
                Some((converter, rgb_target(base, context)))
            }
            None => None,
        };
        let (prepared, conversions) =
            Prepared::new(&description, (decoded, mask), inverse, convert)?;
        prepared.charge(pixels, conversions, &mut budgets)?;
        Ok(Self {
            prepared: Some(Box::new(prepared)),
            channels,
        })
    }

    /// Gives a stencil mask its colour, `color` in working colours painting
    /// `painted`, so that spans carry the colour the exact route composites:
    /// each texel's colour is the colour times its alpha before averaging.
    /// Other images are unchanged.
    #[must_use]
    pub fn with_stencil_color(mut self, color: &[f64], painted: Painted) -> Self {
        let Some(prepared) = self.prepared.as_mut() else {
            self.channels = color.len();
            return self;
        };
        if let Paint::Stencil { table, opacity } = &mut prepared.paint {
            let lanes = color.len() + 2;
            table.clear();
            for alpha in *opacity {
                table.extend(color.iter().map(|v| v * alpha));
                table.extend([alpha, alpha]);
            }
            debug_assert_eq!(table.len(), 2 * lanes);
            prepared.painted = painted;
            self.channels = color.len();
        }
        self
    }

    /// Whether the image is a stencil mask, which paints its colour where
    /// its samples say.
    pub fn is_stencil(&self) -> bool {
        self.prepared
            .as_ref()
            .is_some_and(|prepared| matches!(prepared.paint, Paint::Stencil { .. }))
    }

    /// Whether every pixel's shape equals its alpha. Otherwise (a soft mask
    /// or embedded opacity when alpha is not shape) the shape is the
    /// image's own, which [`Self::span_with_shape`] gives.
    pub fn shape_is_alpha(&self) -> bool {
        self.prepared
            .as_ref()
            .is_none_or(|prepared| prepared.shape_is_alpha)
    }

    /// The repairs decoding made, for the caller to report in source order.
    pub fn repairs(&self) -> &[Diagnostic] {
        self.prepared
            .as_ref()
            .map_or(&[], |prepared| prepared.image.diagnostics.as_slice())
    }

    /// Fills `out` with the colours and alpha of device pixels `x .. x +
    /// len` of row `y`: unpremultiplied working colours (none for an
    /// uncoloured stencil mask) and one alpha per pixel, before coverage.
    pub fn span(&self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
        self.fill((x, y, len), out, None);
    }

    /// As [`Self::span`], also filling `shape` with each pixel's shape.
    pub fn span_with_shape(
        &self,
        x: u32,
        y: u32,
        len: usize,
        out: &mut SpanColors,
        shape: &mut Vec<f64>,
    ) {
        self.fill((x, y, len), out, Some(shape));
    }

    fn fill(
        &self,
        (x, y, len): (u32, u32, usize),
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        let channels = self.channels;
        out.channels = channels;
        out.colors.clear();
        out.alpha.clear();
        let Some(prepared) = &self.prepared else {
            out.colors.resize(len * channels, 0.0);
            out.alpha.resize(len, 0.0);
            if let Some(shape) = shape {
                shape.clear();
                shape.resize(len, 0.0);
            }
            out.painted = Painted(0);
            return;
        };
        out.painted = prepared.painted;
        let lanes = channels + 2;
        if let Some(mask) = &prepared.mask {
            out.alpha.resize(len, 0.0);
            let texels = OpacityTexels {
                mask: &mask.image.image,
                opacity: &mask.opacity,
            };
            mask.geometry.fill((x, y, len), &texels, &mut out.alpha);
        }
        out.colors.resize(len * lanes, 0.0);
        prepared.texels((x, y, len), lanes, &mut out.colors);
        let mut shape = shape;
        if let Some(shape) = shape.as_deref_mut() {
            shape.clear();
        }
        let factors = prepared.mask.as_ref().map(|mask| mask.scales_shape);
        for i in 0..len {
            let base = i * lanes;
            let (color, rest) = out.colors[base..base + lanes].split_at_mut(channels);
            let (mut alpha, mut own) = (rest[0], rest[1]);
            if let Some(scales_shape) = factors {
                let factor = out.alpha[i];
                for value in color.iter_mut() {
                    *value *= factor;
                }
                alpha *= factor;
                if scales_shape {
                    own *= factor;
                }
            }
            if alpha != 1.0 {
                for value in color.iter_mut() {
                    *value = if alpha > 0.0 { *value / alpha } else { 0.0 };
                }
            }
            out.colors.copy_within(base..base + channels, i * channels);
            if factors.is_some() {
                out.alpha[i] = alpha;
            } else {
                out.alpha.push(alpha);
            }
            if let Some(shape) = shape.as_deref_mut() {
                shape.push(own);
            }
        }
        out.colors.truncate(len * channels);
    }
}

impl Prepared {
    /// Writes the summed texels of the span's pixels, `lanes` values each.
    fn texels(&self, span: (u32, u32, usize), lanes: usize, out: &mut [f64]) {
        let image = &self.image.image;
        match &self.paint {
            Paint::Stencil { table, .. } => self.table(span, (table, lanes), out),
            Paint::Colors(paint) => match &paint.1 {
                Strategy::Table(table) => self.table(span, (table, lanes), out),
                Strategy::Rgb(tables) => {
                    let Samples::Bytes(bytes) = &image.samples else {
                        return;
                    };
                    let texels = RgbTexels {
                        pixels: bytes.as_chunks::<3>().0,
                        tables,
                    };
                    self.geometry.fill(span, &texels, out);
                }
                Strategy::Cached(cache) => {
                    let texels = SampleTexels::new(image, &paint.0, None, Some(cache));
                    self.geometry.fill(span, &texels, out);
                }
                Strategy::General => {
                    let colors = &paint.0;
                    let matte = self
                        .mask
                        .as_ref()
                        .filter(|_| colors.matte.is_some())
                        .map(|mask| (&mask.image.image, &mask.opacity));
                    let texels = SampleTexels::new(image, colors, matte, None);
                    self.geometry.fill(span, &texels, out);
                }
            },
        }
    }

    fn table(&self, span: (u32, u32, usize), (table, lanes): (&[f64], usize), out: &mut [f64]) {
        match &self.image.image.samples {
            Samples::Bytes(bytes) => {
                let texels = TableTexels {
                    codes: ByteCodes(bytes),
                    table,
                    lanes,
                };
                self.geometry.fill(span, &texels, out);
            }
            Samples::Packed(packed) => {
                let texels = TableTexels {
                    codes: PackedCodes(packed),
                    table,
                    lanes,
                };
                self.geometry.fill(span, &texels, out);
            }
            Samples::Words(_) | Samples::Mixed { .. } | Samples::Planar(_) => {}
        }
    }
}

/// The image decoded for painting under `unit_to_device`, through the
/// store: drawn-size cells under the `reduced_image_decode` fast parameter,
/// one-bit samples packed.
fn decode(
    source: &ImageSource,
    unit_to_device: Transform,
    context: &PaintContext<'_>,
) -> Result<Arc<Decoded>> {
    let reduce = if context.reduced_image_decode {
        let t = unit_to_device;
        let ratio = (f64::from(source.width()) / t.a.hypot(t.b))
            .min(f64::from(source.height()) / t.c.hypot(t.d));
        source.drawn(ratio)
    } else {
        Reduce::Full
    };
    decode_request(source, reduce, context)
}

fn decode_request(
    source: &ImageSource,
    reduce: Reduce,
    context: &PaintContext<'_>,
) -> Result<Arc<Decoded>> {
    let layout = if source.components() == 1 && source.bits_per_component() == 1 {
        Layout::Native
    } else {
        Layout::Interleaved
    };
    let request = DecodeRequest::new(reduce, None, layout);
    let key = DecodedKey {
        image: source.id(),
        request,
    };
    context.store.get_or_build(key, || {
        let decoded = source.decode(&request, context.limits, source.recovery())?;
        let bytes = decoded_bytes(&decoded.image);
        Ok(Built {
            value: decoded,
            bytes,
        })
    })
}

/// Bytes a decoded image retains.
fn decoded_bytes(image: &DecodedImage) -> usize {
    let alpha = match &image.mask {
        MaskKind::Embedded(alpha) => pagura_image::deferred::storage_bytes(&alpha.samples),
        _ => 0,
    };
    pagura_image::deferred::storage_bytes(&image.samples)
        .saturating_add(alpha)
        .saturating_add(size_of::<Decoded>())
}

/// The sample counts sampling relies on.
fn validate(image: &DecodedImage) -> Result<()> {
    let pixels = u64::from(image.width) * u64::from(image.height);
    let count = usize::try_from(pixels.saturating_mul(u64::from(image.components)))
        .map_err(|_| Error::limit("image sample count overflow"))?;
    if image.components == 0 || image.samples.len() != count {
        return Err(Error::syntax("image sample count mismatch"));
    }
    if let MaskKind::Embedded(alpha) = &image.mask
        && u64::try_from(alpha.samples.len()).ok() != Some(pixels)
    {
        return Err(Error::syntax("image opacity sample count mismatch"));
    }
    Ok(())
}

/// The geometry of `image` under the device-to-unit-square `inverse`.
fn placement(image: &DecodedImage, inverse: Transform) -> Result<Geometry> {
    Geometry::new(
        (image.width, image.height),
        image.extent(),
        image.interpolate,
        inverse,
    )
}

/// Whether `base` converts into the target as the identity with clipping
/// per channel: DeviceRGB into an RGB blending space without spot channels
/// (10.4.2).
fn rgb_target(base: &ColorSpace, context: &PaintContext<'_>) -> bool {
    matches!(base, ColorSpace::DeviceRgb)
        && context.target.process == Process::Rgb
        && context.target.spots.is_empty()
}

/// What sampling reads of an image besides its samples.
#[derive(Debug, Clone, Copy)]
struct Description<'a> {
    /// The colour space; `None` for a stencil mask.
    space: Option<&'a ColorSpace>,
    /// The decode interval of each component.
    decode: &'a [[f64; 2]],
    /// Colour-key ranges of raw codes.
    key: Option<&'a [[u16; 2]]>,
    mask: MaskDescription<'a>,
    alpha_is_shape: bool,
}

/// An image's explicit mask, apart from its samples.
#[derive(Debug, Clone, Copy)]
enum MaskDescription<'a> {
    None,
    /// A stencil mask (`Mask`), whose opacity also scales shape.
    Stencil,
    /// A soft mask (`SMask`) with its decode interval and Matte.
    Soft {
        decode: [f64; 2],
        matte: Option<&'a [f64]>,
    },
}

impl Prepared {
    /// Prepares sampling `decoded` (and its `mask`) under the device-to-unit
    /// square `inverse`, converting colours with `convert` (and per channel
    /// when it is the identity with clipping). Returns the conversions
    /// preparing tables made.
    fn new(
        description: &Description<'_>,
        (decoded, mask): (Arc<Decoded>, Option<Arc<Decoded>>),
        inverse: Transform,
        convert: Option<(Box<dyn Convert>, bool)>,
    ) -> Result<(Self, usize)> {
        validate(&decoded.image)?;
        let geometry = placement(&decoded.image, inverse)?;
        let mask = match (mask, description.mask) {
            (Some(image), MaskDescription::Stencil | MaskDescription::Soft { .. }) => {
                validate(&image.image)?;
                let (decode, scales_shape) = match description.mask {
                    MaskDescription::Soft { decode, .. } => (decode, description.alpha_is_shape),
                    _ => ([0.0, 1.0], true),
                };
                Some(MaskSampling {
                    opacity: Opacity::new(&image.image, decode),
                    geometry: placement(&image.image, inverse)?,
                    image,
                    scales_shape,
                })
            }
            _ => None,
        };
        let (paint, conversions, painted) = match convert {
            Some((convert, rgb)) => {
                let colors = colors(description, &decoded.image, mask.as_ref(), convert)?;
                let painted = initial_painted(description, &colors);
                let (strategy, conversions) = Strategy::new(&decoded.image, &colors, rgb);
                (
                    Paint::Colors(Box::new((colors, strategy))),
                    conversions,
                    painted,
                )
            }
            None => (stencil_paint(&decoded.image), 0, Painted(0)),
        };
        let shape_is_alpha = matches!(
            description.mask,
            MaskDescription::None | MaskDescription::Stencil
        ) && !matches!(decoded.image.mask, MaskKind::Embedded(_))
            || description.alpha_is_shape;
        Ok((
            Self {
                image: decoded,
                geometry,
                paint,
                mask,
                painted,
                shape_is_alpha,
            },
            conversions,
        ))
    }

    /// Charges the texels `pixels` filled pixels read to `work`, and the
    /// colour conversions of preparing tables and of sampling to
    /// `functions`.
    fn charge(
        &self,
        pixels: usize,
        conversions: usize,
        budgets: &mut ImageBudgets<'_>,
    ) -> Result<()> {
        let per_pixel = self
            .geometry
            .cost()
            .saturating_add(self.mask.as_ref().map_or(0, |mask| mask.geometry.cost()));
        budgets.work.charge_product(pixels, per_pixel)?;
        if let Paint::Colors(paint) = &self.paint {
            let (colors, strategy) = paint.as_ref();
            let cost = colors.convert.cost();
            budgets.functions.charge_product(conversions, cost)?;
            if strategy.converts_while_painting() {
                budgets
                    .functions
                    .charge_product(pixels.saturating_mul(self.geometry.cost()), cost)?;
            }
        }
        Ok(())
    }
}

/// The texels of a stencil mask's two codes, without colour.
fn stencil_paint(image: &DecodedImage) -> Paint {
    let inverted = matches!(image.kind, ImageKind::Stencil { inverted: true });
    let opacity = [0u16, 1].map(|code| f64::from(u8::from((code == 1) == inverted)));
    Paint::Stencil {
        table: opacity.iter().flat_map(|&a| [a, a]).collect(),
        opacity,
    }
}

/// How a colour image's samples become texels.
fn colors(
    description: &Description<'_>,
    decoded: &DecodedImage,
    mask: Option<&MaskSampling>,
    convert: Box<dyn Convert>,
) -> Result<Colors> {
    let space = description
        .space
        .ok_or(Error::syntax("image missing colour space"))?;
    let components = usize::from(decoded.components);
    let decode = description
        .decode
        .get(..components)
        .ok_or(Error::syntax("missing Decode component"))?;
    let maxima = (0..components)
        .map(|i| {
            decoded
                .component_bits(i)
                .filter(|bits| (1..=16).contains(bits))
                .map(texels::maximum)
                .ok_or(Error::syntax("invalid image component precision"))
        })
        .collect::<Result<Vec<_>>>()?;
    let embedded = match &decoded.mask {
        MaskKind::Embedded(alpha) => {
            if !(1..=16).contains(&alpha.bits_per_component) {
                return Err(Error::syntax("invalid opacity precision"));
            }
            Some((
                texels::maximum(alpha.bits_per_component),
                alpha.premultiplied,
            ))
        }
        _ => None,
    };
    let matte = match (description.mask, mask) {
        (
            MaskDescription::Soft {
                matte: Some(matte), ..
            },
            Some(mask),
        ) => {
            if (mask.image.image.width, mask.image.image.height) != (decoded.width, decoded.height)
            {
                return Err(Error::syntax("Matte mask dimensions differ from image"));
            }
            Some(matte.to_vec())
        }
        _ => None,
    };
    Ok(Colors {
        decode: decode.to_vec(),
        maxima,
        lookup: Lookup::of(space).map(|(lookup, _)| lookup),
        key: description.key.map(<[[u16; 2]]>::to_vec),
        embedded,
        matte,
        alpha_is_shape: description.alpha_is_shape,
        convert,
    })
}

/// The channels a colour image paints: its space's selection does not
/// depend on the sample values (11.7.4.3).
fn initial_painted(description: &Description<'_>, colors: &Colors) -> Painted {
    let Some(space) = description.space else {
        return Painted(0);
    };
    let initial = match space {
        ColorSpace::Indexed(indexed) => indexed.base.initial(),
        space => space.initial(),
    };
    let mut output = vec![0.0; colors.convert.channels()];
    colors.convert.convert(&initial, &mut output)
}
