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

mod codes;
mod contour;
mod geometry;
mod sampling;
mod scratch;
mod separable;
#[cfg(test)]
mod tests;
mod texels;

use super::{PaintContext, Pixels, SpanColors};
use geometry::{Geometry, Mode, Texels};
use pagura_color::{ColorSpace, Converter, Painted, Process};
use pagura_core::{Built, Diagnostic, Error, Meter, Result, StoreKey};
use pagura_image::{
    DecodeRequest, Decoded, DecodedImage, ImageId, ImageKind, ImageSource, Layout, MaskKind,
    Reduce, Samples,
};
use pagura_interpret::{Image, ImageMask};
use pagura_raster::Transform;
use sampling::Sampling;
pub(crate) use scratch::{Layout as ScratchLayout, Scratch as ImageScratch};
use std::sync::Arc;
use texels::{
    ByteCodes, Colors, Fused, Lookup, MemoTexels, Opacity, OpacityTexels, PackedCodes, RgbTexels,
    SampleTexels, Strategy, TableTexels, TexelMode, WordCodes,
};

/// An image prepared for one render: decoded (through the store) and ready
/// to sample. A stencil mask samples alpha only, or takes its solid colour
/// during preparation so that its texels composite exactly.
#[derive(Debug)]
pub(crate) struct ImageSampler {
    prepared: Option<Box<Prepared>>,
    channels: usize,
}

/// The budgets preparing an image charges.
#[derive(Debug)]
pub(crate) struct ImageBudgets<'a> {
    /// Remaining preparation memory, including temporary capacity discovery.
    pub(crate) bytes: &'a mut Meter,
    /// Render work: one unit per texel the filled pixels read.
    pub(crate) work: &'a mut Meter,
    /// `function_work` of the colour conversions sampling makes.
    pub(crate) functions: &'a mut Meter,
}

/// An owned preparation buffer, charged before allocation.
fn reserved<T>(count: usize, bytes: &mut Meter) -> Result<Vec<T>> {
    let mut values = Vec::new();
    bytes.reserve(&mut values, count)?;
    Ok(values)
}

/// An owned copy, charged while the source remains alive.
fn copied<T: Copy>(values: &[T], bytes: &mut Meter) -> Result<Vec<T>> {
    let mut copy = reserved(values.len(), bytes)?;
    copy.extend_from_slice(values);
    Ok(copy)
}

#[derive(Debug)]
struct Prepared {
    image: Arc<Decoded>,
    geometry: Geometry,
    mask: Option<MaskSampling>,
    painted: Painted,
    /// Whether shape equals alpha, rather than the image's own shape.
    shape_is_alpha: bool,
    plan: Plan,
    /// Device columns covered by the prepared plan.
    columns: usize,
}

#[derive(Debug)]
enum Paint {
    /// A stencil mask: per code its texel, `[colour × alpha.., alpha,
    /// alpha]`, with no colour until one is given.
    Stencil(Vec<f64>),
    /// A colour image.
    Colors(Box<(Colors, Strategy)>),
}

impl Paint {
    /// Heap storage this source alone owns, excluding shared decoded samples.
    fn bytes(&self) -> usize {
        match self {
            Self::Stencil(table) => table.capacity() * size_of::<f64>(),
            Self::Colors(paint) => {
                size_of::<(Colors, Strategy)>() + paint.0.bytes() + paint.1.bytes()
            }
        }
    }

    /// Whether converted texels omit their constant alpha and shape lanes.
    fn opaque(&self, image: &DecodedImage) -> bool {
        match self {
            Self::Colors(paint) => match &paint.1 {
                Strategy::Table(_, opaque) => *opaque,
                Strategy::Indexed(palette) => palette.opaque,
                Strategy::Rgb(_) => matches!(image.samples, Samples::Bytes(_)),
                Strategy::Memo(layout) => layout.opaque(),
                Strategy::General => false,
            },
            Self::Stencil(_) => false,
        }
    }
}

/// One prepared image sampler: integer code averages where the colour
/// map is affine, otherwise converted texels through the same footprints.
#[derive(Debug)]
enum Plan {
    Codes(Box<codes::Codes>),
    Texels { sampling: Sampling, paint: Paint },
}

impl Plan {
    /// Finalizes one owner after stencil recolouring and budget admission.
    fn new(
        (mut paint, mut painted): (Paint, Painted),
        image: &DecodedImage,
        (geometry, columns): (&Geometry, (u32, u32)),
        mask: Option<&MaskSampling>,
        stencil: Option<(&[f64], Painted)>,
        pixels: usize,
        budgets: ImageBudgets<'_>,
    ) -> Result<(Self, Painted)> {
        // Only construction holds both the source paint and its copied code
        // lanes. The returned plan owns exactly the resources it executes.
        enum Selected {
            Codes(Box<codes::Codes>),
            Texels(Sampling),
        }
        let ImageBudgets {
            bytes,
            work,
            functions,
        } = budgets;
        let codes = match mask {
            None => match fast_lanes(&paint, image, bytes)? {
                Some(lanes) => codes::Codes::new(lanes, image, geometry, columns, bytes)?,
                None => None,
            },
            Some(_) => None,
        };
        let mut selected = match codes {
            Some(codes) => {
                bytes.charge_items::<codes::Codes>(1)?;
                Selected::Codes(Box::new(codes))
            }
            None => Selected::Texels(Sampling::new(geometry, columns, bytes)?),
        };
        if let Paint::Colors(paint) = &mut paint
            && let Strategy::Memo(layout) = &mut paint.1
        {
            // The memo holds at most a key per texel read.
            layout.bound(pixels.saturating_mul(geometry.cost()));
            let opaque = layout.opaque();
            let mask = mask.map(|mask| &mask.image.image);
            if let Some(indexed) = layout.fit(image, mask, work, bytes)? {
                let source = (&paint.0, usize::from(image.components), opaque);
                paint.1 = indexed.strategy(source, bytes, functions)?;
            }
        }
        // Keep work and function admission before stencil recolouring, as
        // when the caller applied its solid colour after preparation.
        let per_pixel = geometry.cost().saturating_add(mask.map_or(0, |mask| {
            mask.independent
                .as_ref()
                .map_or(geometry.cost(), |(geometry, _)| geometry.cost())
        }));
        work.charge_product(pixels, per_pixel)?;
        if let Paint::Colors(paint) = &paint {
            let (colors, strategy) = paint.as_ref();
            let planes = matches!(selected, Selected::Texels(_))
                && matches!(image.samples, Samples::Planar(_));
            if strategy.converts_while_painting() || planes {
                functions.charge_product(
                    pixels.saturating_mul(geometry.cost()),
                    colors.convert.cost(),
                )?;
            }
        }
        if let Some((color, channels)) = stencil
            && let Paint::Stencil(table) = &mut paint
        {
            let lanes = color.len() + 2;
            let mut replacement = reserved(2 * lanes, bytes)?;
            for alpha in stencil_opacity(image) {
                replacement.extend(color.iter().map(|v| v * alpha));
                replacement.extend([alpha, alpha]);
            }
            let old = std::mem::replace(table, replacement);
            let released = old.capacity() * size_of::<f64>();
            drop(old);
            bytes.refund(released);
            debug_assert_eq!(table.len(), 2 * lanes);
            selected = match selected {
                Selected::Codes(mut codes) => {
                    if let Some(lanes) = codes::Lanes::single(table, lanes, 2, false, bytes)? {
                        codes.set_lanes(lanes, bytes)?;
                        Selected::Codes(codes)
                    } else {
                        let separable = (*codes).into_separable(bytes);
                        bytes.refund(size_of::<codes::Codes>());
                        Selected::Texels(Sampling::Separable(separable))
                    }
                }
                Selected::Texels(plan) => Selected::Texels(plan),
            };
            painted = channels;
        }
        Ok((
            match selected {
                Selected::Codes(codes) => {
                    let released = paint.bytes();
                    drop(paint);
                    bytes.refund(released);
                    Self::Codes(codes)
                }
                Selected::Texels(sampling) => Self::Texels { sampling, paint },
            },
            painted,
        ))
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Codes(plan) => size_of::<codes::Codes>().saturating_add(plan.bytes()),
            Self::Texels { sampling, paint } => sampling.bytes().saturating_add(paint.bytes()),
        }
    }
}

#[derive(Debug)]
struct MaskSampling {
    image: Arc<Decoded>,
    opacity: Opacity,
    /// Whether the mask's opacity also scales shape: an explicit stencil
    /// mask, or alpha is shape.
    scales_shape: bool,
    /// A mask with its own grid or reconstruction; otherwise opacity
    /// averages over the image's footprints together with the texels.
    independent: Option<(Geometry, Sampling)>,
}

/// A decoded image in the store: the source and the request (layout and
/// orientation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DecodedKey {
    image: ImageId,
    request: DecodeRequest,
}

/// How stored samples relate to the image's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Orientation {
    /// Rows as the image stores them.
    Rows,
    /// Rows and columns exchanged, so that a quarter-turned placement
    /// reads its source rows contiguously.
    Transposed,
}

impl Orientation {
    /// The orientation an image and its mask are sampled in under
    /// `unit_to_device`: exchanged for a quarter turn.
    fn of(unit_to_device: Transform) -> Self {
        if unit_to_device.a == 0.0 && unit_to_device.d == 0.0 {
            Self::Transposed
        } else {
            Self::Rows
        }
    }
}

impl StoreKey for DecodedKey {
    type Value = Decoded;
}

impl ImageSampler {
    /// Prepares `image` drawn under `unit_to_device` (the image's unit
    /// square to device pixels), decoding what the region needs, and
    /// charges `budgets.work` for the texels `pixels` filled device pixels
    /// read and `budgets.functions` for the colour conversions sampling
    /// makes.
    ///
    /// # Errors
    ///
    /// Returns the decode's errors and a `Limit` error when decoding or
    /// sampling exceeds the budgets.
    pub(crate) fn prepare_with(
        image: &Image,
        unit_to_device: Transform,
        context: &PaintContext<'_>,
        pixels: usize,
        stencil: Option<(&[f64], Painted)>,
        budgets: &mut ImageBudgets<'_>,
    ) -> Result<Self> {
        if !unit_to_device.is_finite() {
            return Err(Error::limit("image transform overflow"));
        }
        let channels = if image.space.is_some() {
            context.target.channels()
        } else {
            stencil.map_or(0, |(color, _)| color.len())
        };
        let empty = Self::new(None, channels);
        let Some(inverse) = unit_to_device.inverse() else {
            return Ok(empty);
        };
        if image.space.as_deref().is_some_and(|space| !space.paints()) {
            return Ok(empty);
        }
        // A placement too thin for any of its sample space to be reached
        // finitely covers no visible part of a pixel (zero coverage).
        let extent = |source: &ImageSource| (f64::from(source.width()), f64::from(source.height()));
        let masks = match &image.mask {
            ImageMask::Stencil(mask) | ImageMask::Soft { mask, .. } => Some(&mask.source),
            ImageMask::None | ImageMask::ColorKey(_) | ImageMask::Embedded => None,
        };
        if !std::iter::once(&image.source)
            .chain(masks)
            .all(|source| geometry::reaches(inverse, extent(source)))
        {
            return Ok(empty);
        }
        let plain = matches!(image.mask, ImageMask::None) && !image.interpolate;
        let planes = planes_suit(
            (image.space.as_deref(), &image.decode, plain),
            unit_to_device,
            context,
        );
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
        let orientation = Orientation::of(unit_to_device);
        let decoded = decode(&image.source, (planes, orientation), context)?;
        let mask = mask
            .map(|mask| decode_request(&mask.source, (Layout::Native, orientation), context))
            .transpose()?;
        let mut memory = Meter::new(budgets.bytes.remaining(), "render memory budget exceeded");
        memory.charge(decoded_bytes(&decoded.image))?;
        memory.charge(mask.as_ref().map_or(0, |mask| decoded_bytes(&mask.image)))?;
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
                let converter =
                    Converter::new_metered(base, context.target, &conversion, &mut memory)?;
                Some((converter, rgb_target(base, context)))
            }
            None => None,
        };
        let columns = (context.region.x0, context.region.x1);
        let prepared = Prepared::new(
            &description,
            (decoded, mask),
            (inverse, columns, orientation),
            convert,
            stencil,
            pixels,
            ImageBudgets {
                bytes: &mut memory,
                work: budgets.work,
                functions: budgets.functions,
            },
        )?;
        Ok(Self::new(Some(Box::new(prepared)), channels))
    }

    /// Keeps the values of each component's codes through the conversion's
    /// first stage ([`texels::Curved`]) for texels converted while
    /// painting, charged to `bytes` where it has room for them. They are
    /// optional, admitted once the page's own storage is charged: without
    /// them texels convert directly, to the same values.
    pub(crate) fn admit_curves(&mut self, bytes: &mut Meter) {
        let Some(prepared) = self.prepared.as_deref_mut() else {
            return;
        };
        let image = &prepared.image.image;
        let samples = u64::from(image.width) * u64::from(image.height);
        if let Plan::Texels {
            paint: Paint::Colors(paint),
            ..
        } = &mut prepared.plan
            && paint.1.converts_while_painting()
        {
            paint.0.curved = curved(&paint.0, samples, bytes);
        }
    }

    fn new(prepared: Option<Box<Prepared>>, channels: usize) -> Self {
        Self { prepared, channels }
    }

    /// The colour every pixel takes, when each takes the same opaque one:
    /// an image of one texel sampled nearest, without a mask image, whose
    /// texel has alpha and shape 1. Each pixel's span then holds exactly
    /// these values.
    ///
    /// Decoding repairs keep the sampler, so preparation still reports
    /// them when an image would otherwise become a solid source.
    pub(crate) fn constant(
        &self,
        bytes: &mut Meter,
        functions: &mut Meter,
    ) -> Result<Option<(Box<[f64]>, Painted)>> {
        let Some(prepared) = self.prepared.as_ref() else {
            return Ok(None);
        };
        let image = &prepared.image.image;
        if !self.repairs().is_empty()
            || prepared.mask.is_some()
            || prepared.geometry.mode() != Mode::Nearest
            || (image.width, image.height) != (1, 1)
        {
            return Ok(None);
        }
        if let Plan::Codes(plan) = &prepared.plan {
            let mut colors = SpanColors {
                colors: reserved(self.channels, bytes)?,
                alpha: reserved(1, bytes)?,
                ..SpanColors::default()
            };
            plan.constant(image, &mut colors);
            let opaque = colors.alpha.iter().all(|&alpha| alpha == 1.0);
            let alpha_bytes = colors.alpha.capacity() * size_of::<f64>();
            drop(colors.alpha);
            bytes.refund(alpha_bytes);
            if opaque && colors.colors.len() == self.channels {
                return Ok(Some((colors.colors.into_boxed_slice(), prepared.painted)));
            }
            let released = colors.colors.capacity() * size_of::<f64>();
            drop(colors.colors);
            bytes.refund(released);
            return Ok(None);
        }
        let mut texel = [0.0; geometry::MAX_LANES];
        let lanes = self.channels + 2;
        let texel = &mut texel[..lanes];
        let Plan::Texels { paint, .. } = &prepared.plan else {
            return Ok(None);
        };
        match paint {
            Paint::Stencil(table) => {
                let code = usize::from(image.samples.get(0).unwrap_or(0));
                super::copy_values(texel, table.get(code * lanes..).unwrap_or(&[]));
            }
            Paint::Colors(paint) => {
                if let Strategy::Table(table, _) = &paint.1 {
                    let code = usize::from(image.samples.get(0).unwrap_or(0));
                    super::copy_values(
                        texel,
                        table.get(code * lanes..(code + 1) * lanes).unwrap_or(&[]),
                    );
                } else {
                    functions.charge(paint.0.convert.cost())?;
                    SampleTexels::new(image, &paint.0, None).get(0, texel);
                }
            }
        }
        if texel[self.channels..] == [1.0, 1.0] {
            let color = copied(&texel[..self.channels], bytes)?;
            Ok(Some((color.into_boxed_slice(), prepared.painted)))
        } else {
            Ok(None)
        }
    }

    /// Whether every pixel's shape equals its alpha. Otherwise (a soft mask
    /// or embedded opacity when alpha is not shape) the shape is the
    /// image's own, which [`Self::span_with_shape`] gives.
    pub(crate) fn shape_is_alpha(&self) -> bool {
        self.prepared
            .as_ref()
            .is_none_or(|prepared| prepared.shape_is_alpha)
    }

    /// Bytes the sampler holds for the render: the decoded samples of the
    /// image and its mask, texel tables and the footprint plan. They are
    /// the same whether the decoded image came from the store or was
    /// decoded for this render.
    pub(crate) fn bytes(&self) -> usize {
        self.prepared
            .as_ref()
            .map_or(0, |prepared| prepared.bytes())
    }

    /// Bytes the optional curve values hold ([`Self::admit_curves`]).
    #[cfg(test)]
    pub(crate) fn curve_bytes(&self) -> usize {
        match self.prepared.as_deref().map(|prepared| &prepared.plan) {
            Some(Plan::Texels {
                paint: Paint::Colors(paint),
                ..
            }) => paint.0.curved.as_ref().map_or(0, texels::Curved::bytes),
            _ => 0,
        }
    }

    /// The repairs decoding made, for the caller to report in source order.
    pub(crate) fn repairs(&self) -> &[Diagnostic] {
        self.prepared
            .as_ref()
            .map_or(&[], |prepared| prepared.image.diagnostics.as_slice())
    }

    /// Fills `out` with the colours and alpha of device pixels `x .. x +
    /// len` of row `y`: unpremultiplied working colours (none for an
    /// uncoloured stencil mask) and one alpha per pixel, before coverage.
    pub(crate) fn span(
        &self,
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        out: &mut SpanColors,
    ) {
        self.fill(pixels, scratch, out, None);
    }

    /// As [`Self::span`], also filling `shape` with each pixel's shape.
    pub(crate) fn span_with_shape(
        &self,
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        out: &mut SpanColors,
        shape: &mut Vec<f64>,
    ) {
        self.fill(pixels, scratch, out, Some(shape));
    }

    /// Writes the N16 words of device pixels `x .. x + out.len()` of row
    /// `y` into `out`, as a store at full opacity writes opaque colours,
    /// where every pixel of the span is opaque and its colour needs no
    /// other pass: an RGB image without a mask whose pixels take nearest
    /// samples, or exact area averages that all meet the image, also
    /// bilinear ones where the colours follow the codes. False, with
    /// nothing written, otherwise.
    pub(crate) fn words(
        &self,
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        out: &mut [[u16; 4]],
    ) -> bool {
        let Some(prepared) = &self.prepared else {
            return false;
        };
        if self.channels != 3 || prepared.mask.is_some() {
            return false;
        }
        match &prepared.plan {
            Plan::Codes(codes) => match &mut scratch.plan {
                sampling::Scratch::Separable(storage) => {
                    codes.words(&prepared.image.image, pixels, storage, out)
                }
                _ => false,
            },
            Plan::Texels { sampling, paint } => {
                if let (Sampling::Separable(plan), Paint::Colors(colors)) = (sampling, paint)
                    && let Strategy::Indexed(palette) = &colors.1
                    && let Some(words) = prepared.palette_words(plan, palette, pixels, out)
                {
                    return words;
                }
                prepared.texel_words((sampling, paint), pixels, scratch, out)
            }
        }
    }

    fn fill(
        &self,
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        let len = pixels.len;
        let channels = self.channels;
        out.channels = channels;
        let Some(prepared) = &self.prepared else {
            out.colors.clear();
            out.colors.resize(len * channels, 0.0);
            out.alpha.clear();
            out.alpha.resize(len, 0.0);
            if let Some(shape) = shape {
                shape.clear();
                shape.resize(len, 0.0);
            }
            out.painted = Painted(0);
            return;
        };
        out.painted = prepared.painted;
        let (plan, paint) = match &prepared.plan {
            Plan::Texels { sampling, paint } => (sampling, paint),
            Plan::Codes(codes) => {
                out.alpha.clear();
                if let sampling::Scratch::Separable(storage) = &mut scratch.plan {
                    codes.fill(&prepared.image.image, pixels, storage, out, shape);
                }
                return;
            }
        };
        let lanes = channels + 2;
        let mask = prepared.mask.as_ref();
        // A mask on the image's grid averages ahead of the texels; another
        // is sampled into the alphas first.
        let fused = mask.is_some_and(|mask| mask.independent.is_none());
        out.alpha.resize(len, 0.0);
        if let Some(mask) = mask
            && let Some((geometry, plan)) = &mask.independent
        {
            let texels = OpacityTexels {
                mask: &mask.image.image,
                opacity: &mask.opacity,
            };
            if let Some(storage) = &mut scratch.mask {
                plan.fill(geometry, pixels, storage, &texels, &mut out.alpha);
            }
        }
        let stride = lanes + usize::from(fused);
        let mut storage = std::mem::take(&mut scratch.summed);
        let summed = scratch::grown(&mut storage, len * stride);
        let mode = prepared.texel_mode(paint, pixels);
        // Rows held for texels converted otherwise must not serve these.
        if scratch.mode != mode {
            scratch.plan.reset();
            scratch.mode = mode;
        }
        prepared.texels((plan, paint), pixels, scratch, (lanes, mode), summed);
        let straight = Straight {
            summed,
            stride,
            channels,
            mask: mask.map(|mask| Weight {
                fused,
                scales_shape: mask.scales_shape,
            }),
        };
        match channels {
            1 => straight.write::<1>(out, shape),
            3 => straight.write::<3>(out, shape),
            4 => straight.write::<4>(out, shape),
            _ => straight.write::<0>(out, shape),
        }
        scratch.summed = storage;
    }
}

/// A span's summed texels, `stride` values a pixel, as straight colours.
struct Straight<'a> {
    summed: &'a [f64],
    stride: usize,
    channels: usize,
    mask: Option<Weight>,
}

/// How a mask's opacity weights each texel.
#[derive(Clone, Copy)]
struct Weight {
    /// Whether the opacity is the value ahead of the texel's, rather than
    /// the alpha sampled already.
    fused: bool,
    /// Whether it also scales shape.
    scales_shape: bool,
}

/// Where [`Straight::pixels`] takes each pixel's mask factor from: no mask,
/// the alpha sampled already, or the value ahead of the texel's.
const UNMASKED: u8 = 0;
const SAMPLED: u8 = 1;
const FUSED: u8 = 2;

impl Straight<'_> {
    /// Writes each pixel's straight colours, `C` of them (any number for
    /// 0), as `out.colors`, its alpha over `out.alpha` and its shape as
    /// `shape`.
    fn write<const C: usize>(&self, out: &mut SpanColors, shape: Option<&mut Vec<f64>>) {
        match self.mask {
            None => self.pixels::<C, UNMASKED>(out, shape),
            Some(Weight { fused: false, .. }) => self.pixels::<C, SAMPLED>(out, shape),
            Some(Weight { fused: true, .. }) => self.pixels::<C, FUSED>(out, shape),
        }
    }

    /// [`Self::write`] with the mask factor taken the `FACTOR` way, so that
    /// each case has a loop of its own; the outputs are sized once and each
    /// pixel's values written in place.
    fn pixels<const C: usize, const FACTOR: u8>(
        &self,
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        let channels = if C == 0 { self.channels } else { C };
        let SpanColors { colors, alpha, .. } = out;
        let count = alpha.len().min(self.summed.len() / self.stride);
        colors.clear();
        colors.resize(count * channels, 0.0);
        let mut shape = shape.map(|shape| {
            shape.clear();
            shape.resize(count, 0.0);
            shape.as_mut_slice()
        });
        let scales_shape = self.mask.is_some_and(|mask| mask.scales_shape);
        let ahead = usize::from(FACTOR == FUSED);
        // A stencil painted without a colour has no channels, only alpha.
        let mut colors = colors.chunks_exact_mut(channels.max(1));
        for (index, (texel, alpha)) in self.summed.chunks_exact(self.stride).zip(alpha).enumerate()
        {
            let factor = match FACTOR {
                FUSED => Some(texel[0]),
                SAMPLED => Some(*alpha),
                _ => None,
            };
            let texel = &texel[ahead..];
            let held = (texel[channels], texel[channels + 1]);
            let color = colors.next().unwrap_or_default();
            // A fixed count keeps each pixel's colour in registers.
            let (opacity, own) = match (texel.first_chunk::<C>(), color.first_chunk_mut::<C>()) {
                (Some(texel), Some(color)) if C > 0 => {
                    let mut value = *texel;
                    let held = straighten(&mut value, held, (factor, scales_shape));
                    *color = value;
                    held
                }
                _ => {
                    color.copy_from_slice(&texel[..channels]);
                    straighten(color, held, (factor, scales_shape))
                }
            };
            *alpha = opacity;
            if let Some(shape) = shape.as_deref_mut() {
                shape[index] = own;
            }
        }
    }
}

/// Weights a texel's `color`, opacity and (when `scales_shape`) shape by
/// a mask's `factor`, then divides the colour by the opacity: its straight
/// colour, opacity and shape.
#[inline]
fn straighten(
    color: &mut [f64],
    (mut opacity, mut own): (f64, f64),
    (factor, scales_shape): (Option<f64>, bool),
) -> (f64, f64) {
    if let Some(factor) = factor {
        for value in color.iter_mut() {
            *value *= factor;
        }
        opacity *= factor;
        if scales_shape {
            own *= factor;
        }
    }
    if opacity != 1.0 {
        for value in color.iter_mut() {
            *value = if opacity > 0.0 { *value / opacity } else { 0.0 };
        }
    }
    (opacity, own)
}

struct SamplingBuffers<'a> {
    storage: &'a mut sampling::Scratch,
    fused: &'a mut Vec<f64>,
}

impl Prepared {
    /// Writes the summed texels of the span's pixels, `texels.lanes()`
    /// values each, into `out`; with a mask on the image's grid, each
    /// pixel's mask opacity ahead of them.
    fn sample(
        &self,
        plan: &Sampling,
        pixels: Pixels<'_>,
        buffers: &mut SamplingBuffers<'_>,
        texels: &impl Texels,
        out: &mut [f64],
    ) {
        match self.mask.as_ref().filter(|mask| mask.independent.is_none()) {
            Some(mask) => {
                let fused = Fused {
                    texels,
                    mask: OpacityTexels {
                        mask: &mask.image.image,
                        opacity: &mask.opacity,
                    },
                    values: std::cell::RefCell::new(&mut *buffers.fused),
                };
                plan.fill(&self.geometry, pixels, buffers.storage, &fused, out);
            }
            None => plan.fill(&self.geometry, pixels, buffers.storage, texels, out),
        }
    }

    /// [`ImageSampler::words`] of nearest samples of an image indexed into
    /// `palette` under `plan`, or `None` where the placement or palette
    /// does not allow them. Out of line, so that the path of other sources
    /// stays as it was.
    #[inline(never)]
    fn palette_words(
        &self,
        plan: &separable::Separable,
        palette: &texels::Palette,
        pixels: Pixels<'_>,
        out: &mut [[u16; 4]],
    ) -> Option<bool> {
        let (row, cells) = plan.nearest_samples(pixels)?;
        entry_words(palette, (row, self.image.image.width), cells, out).then_some(true)
    }

    /// [`ImageSampler::words`] of opaque texels sampled nearest or
    /// averaged: the texels, then, where every pixel's alpha is 1, their
    /// words. Bilinear weights need not sum to exactly 1, so those pixels
    /// are filled instead.
    fn texel_words(
        &self,
        (plan, paint): (&Sampling, &Paint),
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        out: &mut [[u16; 4]],
    ) -> bool {
        let lanes = 5;
        if !paint.opaque(&self.image.image) || self.geometry.mode() == Mode::Bilinear {
            return false;
        }
        if self.nearest_words((plan, paint), pixels, scratch, out) {
            return true;
        }
        let len = out.len();
        let mut storage = std::mem::take(&mut scratch.summed);
        let values = scratch::grown(&mut storage, len * lanes);
        self.texels(
            (plan, paint),
            pixels,
            scratch,
            (lanes, TexelMode::Exact),
            values,
        );
        let texels = values.as_chunks::<5>().0;
        // A footprint that misses the image leaves its pixel transparent.
        let opaque = texels.iter().all(|texel| texel[3] == 1.0);
        if opaque {
            for (pixel, texel) in out.iter_mut().zip(texels) {
                *pixel = [
                    crate::kernel::opaque_word(texel[0]),
                    crate::kernel::opaque_word(texel[1]),
                    crate::kernel::opaque_word(texel[2]),
                    u16::MAX,
                ];
            }
        }
        scratch.summed = storage;
        opaque
    }

    /// [`Self::texel_words`] of upright nearest samples of an unmasked
    /// memoized image, written as its texel rows are read, without the
    /// texels' alpha and shape between: false, with nothing written, for
    /// other images and placements. (Unmasked table images placed upright
    /// sample by codes.) Out of line, so that the path of other sources
    /// stays as it was.
    #[inline(never)]
    fn nearest_words(
        &self,
        (plan, paint): (&Sampling, &Paint),
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        out: &mut [[u16; 4]],
    ) -> bool {
        let (Paint::Colors(paint), None) = (paint, &self.mask) else {
            return false;
        };
        let (Strategy::Memo(_), Some(memo)) = (&paint.1, scratch.memo.as_deref_mut()) else {
            return false;
        };
        let image = &self.image.image;
        MemoTexels::new(image, &paint.0, None, memo).is_some_and(|texels| {
            let texels = texels.converting(TexelMode::Exact);
            plan.words(&self.geometry, pixels, &mut scratch.plan, &texels, out)
        })
    }

    /// Writes the summed texels of the span's pixels, `lanes` values each.
    /// How the span's texels convert: each to a value of the same F32
    /// level as its exact colour where only those levels are kept
    /// ([`crate::source::Precision::Level`]) and every pixel copies one
    /// opaque texel unmasked (nearest samples of an opaque memoized image
    /// without a mask), as its colour; otherwise exactly.
    fn texel_mode(&self, paint: &Paint, pixels: Pixels<'_>) -> TexelMode {
        let copies = pixels.precision == crate::source::Precision::Level
            && self.mask.is_none()
            && self.geometry.mode() == Mode::Nearest
            && paint.opaque(&self.image.image);
        match paint {
            Paint::Colors(paint)
                if copies && matches!(paint.1, Strategy::Memo(_)) && paint.0.convert.rounds() =>
            {
                TexelMode::Rounded
            }
            _ => TexelMode::Exact,
        }
    }

    fn texels(
        &self,
        (plan, paint): (&Sampling, &Paint),
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        (lanes, mode): (usize, TexelMode),
        out: &mut [f64],
    ) {
        let ImageScratch {
            plan: storage,
            fused,
            memo,
            ..
        } = scratch;
        let mut buffers = SamplingBuffers { storage, fused };
        let image = &self.image.image;
        match paint {
            Paint::Stencil(table) => {
                self.table(plan, pixels, &mut buffers, (table, lanes, false), out);
            }
            Paint::Colors(paint) => match &paint.1 {
                Strategy::Table(table, opaque) => {
                    self.table(plan, pixels, &mut buffers, (table, lanes, *opaque), out);
                }
                Strategy::Indexed(palette) => {
                    let texels = TableTexels {
                        codes: WordCodes(&palette.indexes),
                        table: &palette.table,
                        lanes,
                        opaque: palette.opaque,
                    };
                    self.sample(plan, pixels, &mut buffers, &texels, out);
                }
                Strategy::Rgb(tables) => {
                    if let Samples::Bytes(bytes) = &image.samples {
                        let texels = RgbTexels {
                            pixels: bytes.as_chunks::<3>().0,
                            tables,
                        };
                        self.sample(plan, pixels, &mut buffers, &texels, out);
                    } else {
                        // Planes sampled by a placement that average codes
                        // do not cover convert each sample.
                        let texels = SampleTexels::new(image, &paint.0, None);
                        self.sample(plan, pixels, &mut buffers, &texels, out);
                    }
                }
                Strategy::Memo(_) => {
                    let colors = &paint.0;
                    let mask = self.mask.as_ref().map(|m| (&m.image.image, &m.opacity));
                    match memo
                        .as_deref_mut()
                        .and_then(|memo| MemoTexels::new(image, colors, mask, memo))
                    {
                        Some(texels) => {
                            let texels = texels.converting(mode);
                            self.sample(plan, pixels, &mut buffers, &texels, out);
                        }
                        None => self.general(plan, pixels, &mut buffers, colors, out),
                    }
                }
                Strategy::General => self.general(plan, pixels, &mut buffers, &paint.0, out),
            },
        }
    }

    /// Every texel computed from its sample, the Matte removed at the soft
    /// mask's opacity.
    fn general(
        &self,
        plan: &Sampling,
        pixels: Pixels<'_>,
        buffers: &mut SamplingBuffers<'_>,
        colors: &Colors,
        out: &mut [f64],
    ) {
        let matte = self
            .mask
            .as_ref()
            .filter(|_| colors.matte.is_some())
            .map(|mask| (&mask.image.image, &mask.opacity));
        let texels = SampleTexels::new(&self.image.image, colors, matte);
        self.sample(plan, pixels, buffers, &texels, out);
    }

    fn table(
        &self,
        plan: &Sampling,
        pixels: Pixels<'_>,
        buffers: &mut SamplingBuffers<'_>,
        (table, lanes, opaque): (&[f64], usize, bool),
        out: &mut [f64],
    ) {
        match &self.image.image.samples {
            Samples::Bytes(bytes) => {
                let texels = TableTexels {
                    codes: ByteCodes(bytes),
                    table,
                    lanes,
                    opaque,
                };
                self.sample(plan, pixels, buffers, &texels, out);
            }
            Samples::Packed(packed) => {
                let texels = TableTexels {
                    codes: PackedCodes(packed),
                    table,
                    lanes,
                    opaque,
                };
                self.sample(plan, pixels, buffers, &texels, out);
            }
            Samples::Words(_) | Samples::Mixed { .. } | Samples::Planar(_) => {}
        }
    }
}

/// Writes the words of nearest samples `cells` of source row `row` of an
/// image `width` samples wide through their palette entries: the words
/// [`Prepared::texel_words`] writes for those entries' texels. False,
/// with nothing written, where the palette has no words or a sample is
/// missing.
fn entry_words(
    palette: &texels::Palette,
    (row, width): (usize, u32),
    cells: &[u32],
    out: &mut [[u16; 4]],
) -> bool {
    let width = usize::try_from(width).unwrap_or(usize::MAX);
    let line = row
        .checked_mul(width)
        .and_then(|start| palette.indexes.get(start..start.checked_add(width)?));
    let Some(line) = line.filter(|_| cells.len() == out.len() && !palette.words.is_empty()) else {
        return false;
    };
    if cells
        .iter()
        .any(|&cell| separable::index(cell) >= line.len())
    {
        return false;
    }
    // Every index names an entry, which has its words.
    for (pixel, &cell) in out.iter_mut().zip(cells) {
        let id = line
            .get(separable::index(cell))
            .map_or(0, |&id| usize::from(id));
        *pixel = palette.words.get(id).copied().unwrap_or_default();
    }
    true
}

/// The image decoded for painting through the store: one-bit samples
/// packed, a JPEG's component planes when `planes`, and rows and columns
/// exchanged as `orientation` says.
fn decode(
    source: &ImageSource,
    (planes, orientation): (bool, Orientation),
    context: &PaintContext<'_>,
) -> Result<Arc<Decoded>> {
    let layout = if planes || source.components() == 1 && source.bits_per_component() == 1 {
        Layout::Native
    } else {
        Layout::Interleaved
    };
    decode_request(source, (layout, orientation), context)
}

/// Whether an image is sampled from a JPEG's component planes, converting
/// rows as it reads them: an unmasked DeviceRGB image without
/// interpolation, placed upright or turned by a quarter, whose decode array
/// keeps its texels affine in the codes, painted into an RGB blending
/// space.
fn planes_suit(
    (space, decode, plain): (Option<&ColorSpace>, &[[f64; 2]], bool),
    unit_to_device: Transform,
    context: &PaintContext<'_>,
) -> bool {
    let t = unit_to_device;
    plain
        && matches!(space, Some(ColorSpace::DeviceRgb))
        && (t.b == 0.0 && t.c == 0.0 || t.a == 0.0 && t.d == 0.0)
        && decode
            .iter()
            .flatten()
            .all(|value| (0.0..=1.0).contains(value))
        && rgb_target(&ColorSpace::DeviceRgb, context)
}

fn decode_request(
    source: &ImageSource,
    (layout, orientation): (Layout, Orientation),
    context: &PaintContext<'_>,
) -> Result<Arc<Decoded>> {
    let request =
        DecodeRequest::new(Reduce::Full, None, layout).with_orientation(match orientation {
            Orientation::Rows => pagura_image::Orientation::Rows,
            Orientation::Transposed => pagura_image::Orientation::Transposed,
        });
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

/// The geometry of `image`, stored in `orientation`, under the
/// device-to-unit-square `inverse`.
fn placement(
    image: &DecodedImage,
    inverse: Transform,
    orientation: Orientation,
) -> Result<Geometry> {
    let (width, height) = (image.width, image.height);
    match orientation {
        Orientation::Rows => Geometry::new((width, height), image.interpolate, inverse),
        Orientation::Transposed => {
            Ok(Geometry::new((height, width), image.interpolate, inverse)?.transposed())
        }
    }
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
    /// when it is the identity with clipping), charging its owned storage
    /// and table conversions before construction.
    fn new(
        description: &Description<'_>,
        (decoded, mask): (Arc<Decoded>, Option<Arc<Decoded>>),
        (inverse, columns, orientation): (Transform, (u32, u32), Orientation),
        convert: Option<(Converter, bool)>,
        stencil: Option<(&[f64], Painted)>,
        pixels: usize,
        budgets: ImageBudgets<'_>,
    ) -> Result<Self> {
        let ImageBudgets {
            bytes,
            work,
            functions,
        } = budgets;
        bytes.charge_items::<Self>(1)?;
        validate(&decoded.image)?;
        let geometry = placement(&decoded.image, inverse, orientation)?;
        let mask = match (mask, description.mask) {
            (Some(image), MaskDescription::Stencil | MaskDescription::Soft { .. }) => {
                validate(&image.image)?;
                let (decode, scales_shape) = match description.mask {
                    MaskDescription::Soft { decode, .. } => (decode, description.alpha_is_shape),
                    _ => ([0.0, 1.0], true),
                };
                // On the image's grid and sampled alike: the same samples
                // and the same reconstruction (a mask has its own
                // `Interpolate`, which an area average does not read).
                // Every value averages alone, so the opacity is the same
                // either way.
                let fused = (image.image.width, image.image.height)
                    == (decoded.image.width, decoded.image.height)
                    && (image.image.interpolate == decoded.image.interpolate
                        || geometry.mode() == Mode::Area);
                Some(MaskSampling {
                    opacity: Opacity::new(&image.image, decode, bytes)?,
                    independent: if fused {
                        None
                    } else {
                        let geometry = placement(&image.image, inverse, orientation)?;
                        let plan = Sampling::new(&geometry, columns, bytes)?;
                        Some((geometry, plan))
                    },
                    image,
                    scales_shape,
                })
            }
            _ => None,
        };
        let (paint, painted) = match convert {
            Some((convert, rgb)) => {
                let colors = colors(description, &decoded.image, mask.as_ref(), convert, bytes)?;
                // Sampled images select channels independently of their
                // values (11.7.4.3).
                let painted = colors.convert.painted();
                let strategy = Strategy::new(
                    &decoded.image,
                    &colors,
                    rgb,
                    mask.as_ref().map(|mask| &mask.image.image),
                    bytes,
                    functions,
                )?;
                bytes.charge_items::<(Colors, Strategy)>(1)?;
                (Paint::Colors(Box::new((colors, strategy))), painted)
            }
            None => (stencil_paint(&decoded.image, bytes)?, Painted(0)),
        };
        let shape_is_alpha = matches!(
            description.mask,
            MaskDescription::None | MaskDescription::Stencil
        ) && !matches!(decoded.image.mask, MaskKind::Embedded(_))
            || description.alpha_is_shape;
        let (plan, painted) = Plan::new(
            (paint, painted),
            &decoded.image,
            (&geometry, columns),
            mask.as_ref(),
            stencil,
            pixels,
            ImageBudgets {
                bytes,
                work,
                functions,
            },
        )?;
        Ok(Self {
            image: decoded,
            geometry,
            mask,
            painted,
            shape_is_alpha,
            plan,
            columns: usize::try_from(columns.1.saturating_sub(columns.0)).unwrap_or(usize::MAX),
        })
    }

    /// Bytes held for the render: decoded samples, tables and plans.
    fn bytes(&self) -> usize {
        let mask = self.mask.as_ref().map_or(0, |mask| {
            let plan = mask
                .independent
                .as_ref()
                .map_or(0, |(_, plan)| plan.bytes());
            decoded_bytes(&mask.image.image)
                .saturating_add(mask.opacity.bytes())
                .saturating_add(plan)
        });
        [decoded_bytes(&self.image.image), mask, self.plan.bytes()]
            .into_iter()
            .fold(size_of::<Self>(), usize::saturating_add)
    }
}

/// The lanes of sampling by average codes for `paint`, when its texels
/// follow the codes of `image` affinely.
fn fast_lanes(
    paint: &Paint,
    image: &DecodedImage,
    bytes: &mut Meter,
) -> Result<Option<codes::Lanes>> {
    let Some(bits) = image
        .component_bits(0)
        .filter(|bits| (1..=8).contains(bits))
    else {
        return Ok(None);
    };
    let codes = 1usize << bits;
    match paint {
        Paint::Stencil(table) => codes::Lanes::single(table, table.len() / 2, 2, false, bytes),
        Paint::Colors(paint) => match &paint.1 {
            Strategy::Table(table, opaque) => {
                let lanes = paint.0.lanes();
                codes::Lanes::single(table, lanes, codes, *opaque, bytes)
            }
            Strategy::Rgb(tables) => codes::Lanes::rgb(tables, bytes),
            Strategy::Memo(_) | Strategy::Indexed(..) | Strategy::General => Ok(None),
        },
    }
}

/// The texels of a stencil mask's two codes, without colour.
fn stencil_paint(image: &DecodedImage, bytes: &mut Meter) -> Result<Paint> {
    let mut table = reserved(4, bytes)?;
    table.extend(stencil_opacity(image).iter().flat_map(|&a| [a, a]));
    Ok(Paint::Stencil(table))
}

/// The canonical opacities of a stencil's two codes.
fn stencil_opacity(image: &DecodedImage) -> [f64; 2] {
    let inverted = matches!(image.kind, ImageKind::Stencil { inverted: true });
    [0u16, 1].map(|code| f64::from(u8::from((code == 1) == inverted)))
}

/// How a colour image's samples become texels.
fn colors(
    description: &Description<'_>,
    decoded: &DecodedImage,
    mask: Option<&MaskSampling>,
    convert: Converter,
    bytes: &mut Meter,
) -> Result<Colors> {
    let space = description
        .space
        .ok_or(Error::syntax("image missing colour space"))?;
    let components = usize::from(decoded.components);
    let decode = description
        .decode
        .get(..components)
        .ok_or(Error::syntax("missing Decode component"))?;
    let mut maxima = reserved(components, bytes)?;
    for i in 0..components {
        maxima.push(
            decoded
                .component_bits(i)
                .filter(|bits| (1..=16).contains(bits))
                .map(texels::maximum)
                .ok_or(Error::syntax("invalid image component precision"))?,
        );
    }
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
            Some(copied(matte, bytes)?)
        }
        _ => None,
    };
    Ok(Colors {
        decode: copied(decode, bytes)?,
        maxima,
        lookup: Lookup::of(space).map(|(lookup, _)| lookup),
        key: description.key.map(|key| copied(key, bytes)).transpose()?,
        embedded,
        matte,
        alpha_is_shape: description.alpha_is_shape,
        convert,
        // Optional, and admitted after everything the image must hold
        // ([`ImageSampler::admit_curves`]).
        curved: None,
    })
}

/// Room for each component's codes through the conversion's first stage
/// ([`Converter::curve`]), found as texels need them ([`texels::Curved`]),
/// charged to `bytes`. `None` where a code alone does not give the value
/// converted (a lookup, premultiplied opacity or Matte), a component has
/// more than eight bits, the conversion has no such stage, the image has
/// fewer `samples` than the room has entries, or `bytes` lacks the room:
/// its texels then convert directly.
fn curved(colors: &Colors, samples: u64, bytes: &mut Meter) -> Option<texels::Curved> {
    let components = colors.maxima.len();
    let entries = components * texels::CURVED_CODES;
    let size = texels::Curved::bytes_of(components);
    #[expect(clippy::cast_precision_loss, reason = "256 codes")]
    let most = (texels::CURVED_CODES - 1) as f64;
    let plain = colors.lookup.is_none()
        && !colors
            .embedded
            .is_some_and(|(_, premultiplied)| premultiplied)
        && colors.matte.is_none();
    if !plain
        || samples < u64::try_from(entries).unwrap_or(u64::MAX)
        || colors.maxima.iter().any(|&maximum| maximum > most)
        || size > bytes.remaining()
        || (0..components).any(|i| colors.convert.curve(i, 0.0).is_none())
    {
        return None;
    }
    let mut values = Vec::new();
    if bytes.reserve(&mut values, entries).is_err() {
        // The charge fit; only the allocation failed.
        bytes.refund(size);
        return None;
    }
    Some(texels::Curved::new(components, values))
}
