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
    SampleTexels, Strategy, TableTexels,
};

/// An image prepared for one render: decoded (through the store) and ready
/// to sample. A stencil mask samples alpha only; the executor colours it
/// with its paint, or [`ImageSampler::with_stencil_color`] does.
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

#[derive(Debug)]
struct Prepared {
    image: Arc<Decoded>,
    geometry: Geometry,
    paint: Paint,
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
    Stencil { table: Vec<f64>, opacity: [f64; 2] },
    /// A colour image.
    Colors(Box<(Colors, Strategy)>),
}

/// One prepared image sampler: integer code averages where the colour
/// map is affine, otherwise converted texels through the same footprints.
#[derive(Debug)]
enum Plan {
    Codes(Box<codes::Codes>),
    Texels(Sampling),
}

impl Plan {
    fn bytes(&self) -> usize {
        match self {
            Self::Codes(plan) => size_of::<codes::Codes>().saturating_add(plan.bytes()),
            Self::Texels(plan) => plan.bytes(),
        }
    }
}

#[derive(Debug)]
struct MaskSampling {
    image: Arc<Decoded>,
    geometry: Geometry,
    opacity: Opacity,
    /// Whether the mask's opacity also scales shape: an explicit stencil
    /// mask, or alpha is shape.
    scales_shape: bool,
    plan: Sampling,
    /// Whether the mask is on the image's grid, so that its opacity
    /// averages over the image's footprints together with the texels.
    fused: bool,
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
                let converter = Converter::new(base, context.target, &conversion)?;
                Some((converter, rgb_target(base, context)))
            }
            None => None,
        };
        let columns = (context.region.x0, context.region.x1);
        let (mut prepared, conversions) = Prepared::new(
            &description,
            (decoded, mask),
            (inverse, columns, orientation),
            convert,
        )?;
        // The prepared source stays alive while capacity discovery uses
        // temporary storage; the planner charges its retained bytes next.
        let held = prepared.bytes();
        budgets.bytes.charge(held)?;
        let fitted = prepared.fit_memo(&mut budgets);
        budgets.bytes.refund(held);
        fitted?;
        prepared.charge(pixels, conversions, &mut budgets)?;
        Ok(Self::new(Some(Box::new(prepared)), channels))
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
    pub(crate) fn constant(&self) -> Option<(Box<[f64]>, Painted)> {
        let prepared = self.prepared.as_ref()?;
        let image = &prepared.image.image;
        if !self.repairs().is_empty()
            || prepared.mask.is_some()
            || prepared.geometry.mode() != Mode::Nearest
            || (image.width, image.height) != (1, 1)
        {
            return None;
        }
        let mut colors = SpanColors::default();
        if let Plan::Codes(plan) = &prepared.plan {
            plan.constant(image, &mut colors);
            let opaque = colors.alpha.iter().all(|&alpha| alpha == 1.0);
            return (opaque && colors.colors.len() == self.channels)
                .then(|| (colors.colors.into_boxed_slice(), prepared.painted));
        }
        let mut texel = [0.0; geometry::MAX_LANES];
        let lanes = self.channels + 2;
        let texel = &mut texel[..lanes];
        match &prepared.paint {
            Paint::Stencil { table, .. } => {
                let code = usize::from(image.samples.get(0).unwrap_or(0));
                texels::copy_texel(texel, table.get(code * lanes..).unwrap_or(&[]));
            }
            Paint::Colors(paint) => {
                SampleTexels::new(image, &paint.0, None).get(0, texel);
            }
        }
        (texel[self.channels..] == [1.0, 1.0])
            .then(|| (texel[..self.channels].into(), prepared.painted))
    }

    /// Gives a stencil mask its colour, `color` in working colours painting
    /// `painted`, so that spans carry the exactly composited colour: each
    /// texel's colour is the colour times its alpha before averaging.
    /// Other images are unchanged.
    #[must_use]
    pub(crate) fn with_stencil_color(mut self, color: &[f64], painted: Painted) -> Self {
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
            let old = std::mem::replace(&mut prepared.plan, Plan::Texels(Sampling::Points));
            prepared.plan = match old {
                Plan::Codes(mut codes) => match codes::Lanes::single(table, lanes, 2, false) {
                    Some(lanes) => {
                        codes.set_lanes(lanes);
                        Plan::Codes(codes)
                    }
                    None => Plan::Texels(Sampling::Separable((*codes).into_separable())),
                },
                Plan::Texels(plan) => Plan::Texels(plan),
            };
            prepared.painted = painted;
            self.channels = color.len();
        }
        self
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
    /// samples. False, with `out` unspecified, otherwise.
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
            Plan::Texels(plan) => prepared.texel_words(plan, pixels, scratch, out),
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
        let plan = match &prepared.plan {
            Plan::Texels(plan) => plan,
            Plan::Codes(codes) => {
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
        let fused = mask.is_some_and(|mask| mask.fused);
        out.alpha.resize(len, 0.0);
        if let Some(mask) = mask.filter(|mask| !mask.fused) {
            let texels = OpacityTexels {
                mask: &mask.image.image,
                opacity: &mask.opacity,
            };
            if let Some(storage) = &mut scratch.mask {
                mask.plan
                    .fill(&mask.geometry, pixels, storage, &texels, &mut out.alpha);
            }
        }
        let stride = lanes + usize::from(fused);
        let mut summed = std::mem::take(&mut scratch.summed);
        summed.resize(len * stride, 0.0);
        prepared.texels(plan, pixels, scratch, lanes, &mut summed);
        out.colors.resize(len * channels, 0.0);
        let mut shape = shape;
        if let Some(shape) = shape.as_deref_mut() {
            shape.clear();
        }
        let scales_shape = mask.is_some_and(|mask| mask.scales_shape);
        for i in 0..len {
            let base = i * stride;
            let texel = &summed[base..base + stride];
            let (factor, texel) = match (mask, fused) {
                (Some(_), true) => (Some(texel[0]), &texel[1..]),
                (Some(_), false) => (Some(out.alpha[i]), texel),
                (None, _) => (None, texel),
            };
            let mut color = [0.0; geometry::MAX_LANES];
            let color = &mut color[..channels];
            texels::copy_texel(color, texel);
            let (mut alpha, mut own) = (texel[channels], texel[channels + 1]);
            if let Some(factor) = factor {
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
            texels::copy_texel(&mut out.colors[i * channels..], color);
            out.alpha[i] = alpha;
            if let Some(shape) = shape.as_deref_mut() {
                shape.push(own);
            }
        }
        scratch.summed = summed;
    }
}

struct SamplingBuffers<'a> {
    storage: &'a mut sampling::Scratch,
    fused: &'a mut Vec<f64>,
}

impl Prepared {
    /// Sizes a sparse memo from its source keys without reducing the
    /// conversion reuse of photographic images.
    fn fit_memo(&mut self, budgets: &mut ImageBudgets<'_>) -> Result<()> {
        if let Paint::Colors(paint) = &mut self.paint
            && let Strategy::Memo(layout) = &mut paint.1
        {
            layout.fit(
                &self.image.image,
                self.mask.as_ref().map(|mask| &mask.image.image),
                budgets.work,
                budgets.bytes,
            )?;
        }
        Ok(())
    }

    /// Whether converted texels omit their constant alpha and shape lanes.
    fn opaque(&self) -> bool {
        match &self.paint {
            Paint::Colors(paint) => match &paint.1 {
                Strategy::Table(_, opaque) => *opaque,
                Strategy::Rgb(_) => matches!(self.image.image.samples, Samples::Bytes(_)),
                Strategy::Memo(layout) => layout.opaque(),
                Strategy::General => false,
            },
            Paint::Stencil { .. } => false,
        }
    }

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
        match self.mask.as_ref().filter(|mask| mask.fused) {
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

    /// [`ImageSampler::words`] of opaque texels sampled nearest: the
    /// texels, then their words.
    fn texel_words(
        &self,
        plan: &Sampling,
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        out: &mut [[u16; 4]],
    ) -> bool {
        let lanes = 5;
        if !self.opaque() || !plan.nearest() {
            return false;
        }
        let len = out.len();
        let mut values = std::mem::take(&mut scratch.summed);
        values.resize(len * lanes, 0.0);
        self.texels(plan, pixels, scratch, lanes, &mut values);
        for (pixel, texel) in out.iter_mut().zip(values.as_chunks::<5>().0) {
            *pixel = [
                crate::kernel::opaque_word(texel[0]),
                crate::kernel::opaque_word(texel[1]),
                crate::kernel::opaque_word(texel[2]),
                u16::MAX,
            ];
        }
        scratch.summed = values;
        true
    }

    /// Writes the summed texels of the span's pixels, `lanes` values each.
    fn texels(
        &self,
        plan: &Sampling,
        pixels: Pixels<'_>,
        scratch: &mut ImageScratch,
        lanes: usize,
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
        match &self.paint {
            Paint::Stencil { table, .. } => {
                self.table(plan, pixels, &mut buffers, (table, lanes, false), out);
            }
            Paint::Colors(paint) => match &paint.1 {
                Strategy::Table(table, opaque) => {
                    self.table(plan, pixels, &mut buffers, (table, lanes, *opaque), out);
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
                        Some(texels) => self.sample(plan, pixels, &mut buffers, &texels, out),
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
    /// when it is the identity with clipping). Returns the conversions
    /// preparing tables made.
    fn new(
        description: &Description<'_>,
        (decoded, mask): (Arc<Decoded>, Option<Arc<Decoded>>),
        (inverse, columns, orientation): (Transform, (u32, u32), Orientation),
        convert: Option<(Converter, bool)>,
    ) -> Result<(Self, usize)> {
        validate(&decoded.image)?;
        let geometry = placement(&decoded.image, inverse, orientation)?;
        let mask = match (mask, description.mask) {
            (Some(image), MaskDescription::Stencil | MaskDescription::Soft { .. }) => {
                validate(&image.image)?;
                let (decode, scales_shape) = match description.mask {
                    MaskDescription::Soft { decode, .. } => (decode, description.alpha_is_shape),
                    _ => ([0.0, 1.0], true),
                };
                let geometry = placement(&image.image, inverse, orientation)?;
                // On the image's grid and sampled alike: the same samples
                // and the same reconstruction (a mask has its own
                // `Interpolate`).
                let fused = (image.image.width, image.image.height)
                    == (decoded.image.width, decoded.image.height)
                    && image.image.interpolate == decoded.image.interpolate;
                Some(MaskSampling {
                    opacity: Opacity::new(&image.image, decode),
                    plan: Sampling::new(&geometry, columns)?,
                    geometry,
                    image,
                    scales_shape,
                    fused,
                })
            }
            _ => None,
        };
        let (paint, conversions, painted) = match convert {
            Some((convert, rgb)) => {
                let colors = colors(description, &decoded.image, mask.as_ref(), convert)?;
                // Sampled images select channels independently of their
                // values (11.7.4.3).
                let painted = colors.convert.painted();
                let (strategy, conversions) = Strategy::new(
                    &decoded.image,
                    &colors,
                    rgb,
                    mask.as_ref().map(|mask| &mask.image.image),
                );
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
        let codes = match (&mask, fast_lanes(&paint, &decoded.image)) {
            (None, Some(lanes)) => codes::Codes::new(lanes, &decoded.image, &geometry, columns)?,
            _ => None,
        };
        let plan = match codes {
            Some(codes) => Plan::Codes(Box::new(codes)),
            None => Plan::Texels(Sampling::new(&geometry, columns)?),
        };
        Ok((
            Self {
                image: decoded,
                geometry,
                paint,
                mask,
                painted,
                shape_is_alpha,
                plan,
                columns: usize::try_from(columns.1.saturating_sub(columns.0)).unwrap_or(usize::MAX),
            },
            conversions,
        ))
    }

    /// Bytes held for the render: decoded samples, tables and plans.
    fn bytes(&self) -> usize {
        let tables = match &self.paint {
            Paint::Stencil { table, .. } => size_of_val::<[f64]>(table),
            Paint::Colors(paint) => paint.1.bytes(),
        };
        let mask = self
            .mask
            .as_ref()
            .map_or(0, |mask| decoded_bytes(&mask.image.image));
        [
            decoded_bytes(&self.image.image),
            mask,
            tables,
            self.plan.bytes(),
            self.mask.as_ref().map_or(0, |mask| mask.plan.bytes()),
        ]
        .into_iter()
        .fold(size_of::<Self>(), usize::saturating_add)
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
            let planes = matches!(self.plan, Plan::Texels(_))
                && matches!(self.image.image.samples, Samples::Planar(_));
            if strategy.converts_while_painting() || planes {
                budgets
                    .functions
                    .charge_product(pixels.saturating_mul(self.geometry.cost()), cost)?;
            }
        }
        Ok(())
    }
}

/// The lanes of sampling by average codes for `paint`, when its texels
/// follow the codes of `image` affinely.
fn fast_lanes(paint: &Paint, image: &DecodedImage) -> Option<codes::Lanes> {
    let codes = 1usize
        << image
            .component_bits(0)
            .filter(|bits| (1..=8).contains(bits))?;
    match paint {
        Paint::Stencil { table, .. } => codes::Lanes::single(table, table.len() / 2, 2, false),
        Paint::Colors(paint) => match &paint.1 {
            Strategy::Table(table, opaque) => {
                let lanes = paint.0.lanes();
                codes::Lanes::single(table, lanes, codes, *opaque)
            }
            Strategy::Rgb(tables) => codes::Lanes::rgb(tables),
            Strategy::Memo(_) | Strategy::General => None,
        },
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
    convert: Converter,
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
