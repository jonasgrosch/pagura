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
mod separable;
#[cfg(test)]
mod tests;
mod texels;

use super::{PaintContext, SpanColors};
use contour::Contour;
use geometry::{Geometry, Mode, Texels};
use pagura_color::{ColorSpace, Converter, Painted, Process};
use pagura_core::{Built, Diagnostic, Error, Meter, Result, StoreKey};
use pagura_image::{
    DecodeRequest, Decoded, DecodedImage, ImageId, ImageKind, ImageSource, Layout, MaskKind,
    Reduce, Samples,
};
use pagura_interpret::{Image, ImageMask};
use pagura_raster::Transform;
use separable::{RowCache, Rows, Separable};
use std::sync::Arc;
use texels::{
    ByteCodes, Colors, Convert, Fused, Lookup, MemoTexels, Opacity, OpacityTexels, PackedCodes,
    RgbTexels, SampleTexels, Strategy, TableTexels,
};

/// An image prepared for one render: decoded (through the store) and ready
/// to sample. A stencil mask samples alpha only; the executor colours it
/// with its paint, or [`ImageSampler::with_stencil_color`] does.
#[derive(Debug)]
pub struct ImageSampler {
    prepared: Option<Box<Prepared>>,
    channels: usize,
    /// The colour of every pixel, opaque, when all are alike.
    uniform: Option<Vec<f64>>,
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
    /// Sampling by average codes, where the texels allow it.
    codes: Option<codes::Codes>,
    /// Separable sampling of converted texels otherwise, where the
    /// placement is upright.
    separable: Option<Separable>,
    /// Contour sampling where a turned placement minifies.
    contour: Option<Contour>,
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
    /// Separable sampling of the opacities, where the placement is upright.
    separable: Option<Separable>,
    /// Contour sampling where a turned placement minifies.
    contour: Option<Contour>,
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
                let converter: Box<dyn Convert> =
                    Box::new(Converter::new(base, context.target, &conversion)?);
                Some((converter, rgb_target(base, context)))
            }
            None => None,
        };
        let columns = (context.region.x0, context.region.x1);
        let (prepared, conversions) = Prepared::new(
            &description,
            (decoded, mask),
            (inverse, columns, orientation),
            convert,
        )?;
        prepared.charge(pixels, conversions, &mut budgets)?;
        Ok(Self::new(Some(Box::new(prepared)), channels))
    }

    fn new(prepared: Option<Box<Prepared>>, channels: usize) -> Self {
        let mut sampler = Self {
            prepared,
            channels,
            uniform: None,
        };
        sampler.uniform = sampler.find_uniform();
        sampler
    }

    /// The colour every pixel takes, when each takes the same opaque one:
    /// an image of one texel sampled nearest, without a mask image, whose
    /// texel has alpha and shape 1. Each pixel's span then holds exactly
    /// these values.
    fn find_uniform(&self) -> Option<Vec<f64>> {
        let prepared = self.prepared.as_ref()?;
        let image = &prepared.image.image;
        if prepared.mask.is_some()
            || prepared.geometry.mode() != Mode::Nearest
            || (image.width, image.height) != (1, 1)
        {
            return None;
        }
        let mut colors = SpanColors::default();
        let mut shape = Vec::new();
        if prepared.shape_is_alpha {
            self.span(0, 0, 1, &mut colors);
        } else {
            self.span_with_shape(0, 0, 1, &mut colors, &mut shape);
        }
        let opaque = colors.alpha.iter().chain(&shape).all(|&value| value == 1.0);
        (opaque && colors.colors.len() == self.channels).then_some(colors.colors)
    }

    /// The colour of every pixel, opaque, when the image paints one colour
    /// alike everywhere, and the channels it paints.
    pub(crate) fn uniform(&self) -> Option<(&[f64], Painted)> {
        let painted = self.prepared.as_ref()?.painted;
        Some((self.uniform.as_deref()?, painted))
    }

    /// Gives a stencil mask its colour, `color` in working colours painting
    /// `painted`, so that spans carry the exactly composited colour: each
    /// texel's colour is the colour times its alpha before averaging.
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
            // Rows kept from the colourless table are stale.
            if let Some(separable) = prepared.separable.as_mut() {
                separable.forget();
            }
            if let Some(codes) = prepared.codes.as_mut() {
                match codes::Lanes::single(table, lanes, 2, false) {
                    Some(fast) => codes.set_lanes(fast),
                    None => prepared.codes = None,
                }
            }
            prepared.painted = painted;
            self.channels = color.len();
        }
        self.uniform = self.find_uniform();
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

    /// Bytes the sampler holds for the render: the decoded samples of the
    /// image and its mask, texel tables and the footprint plan. They are
    /// the same whether the decoded image came from the store or was
    /// decoded for this render.
    pub fn bytes(&self) -> usize {
        self.prepared
            .as_ref()
            .map_or(0, |prepared| prepared.bytes())
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
        if let Some(codes) = &prepared.codes {
            codes.fill(&prepared.image.image, (x, y, len), out, shape);
            return;
        }
        let lanes = channels + 2;
        let fused = prepared.mask.as_ref().is_some_and(|mask| mask.fused);
        if fused {
            return prepared.fused((x, y, len), lanes, out, shape);
        }
        if let Some(mask) = &prepared.mask {
            out.alpha.resize(len, 0.0);
            let texels = OpacityTexels {
                mask: &mask.image.image,
                opacity: &mask.opacity,
            };
            sample(
                (mask.separable.as_ref(), mask.contour.as_ref(), &mask.geometry),
                (x, y, len),
                &texels,
                &mut out.alpha,
            );
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
            // Moved value by value: a few values each, where a copy call
            // costs more than the move.
            for (to, from) in (i * channels..).zip(base..base + channels) {
                out.colors[to] = out.colors[from];
            }
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
    /// Writes the summed texels of the span's pixels, `texels.lanes()`
    /// values each, into `out`; with a mask on the image's grid, each
    /// pixel's mask opacity ahead of them.
    fn sample(&self, span: (u32, u32, usize), texels: &impl Texels, out: &mut [f64]) {
        let plan = (
            self.separable.as_ref(),
            self.contour.as_ref(),
            &self.geometry,
        );
        match self.mask.as_ref().filter(|mask| mask.fused) {
            Some(mask) => {
                let fused = Fused {
                    texels,
                    mask: OpacityTexels {
                        mask: &mask.image.image,
                        opacity: &mask.opacity,
                    },
                    values: std::cell::RefCell::new(Vec::new()),
                };
                sample(plan, span, &fused, out);
            }
            None => sample(plan, span, texels, out),
        }
    }

    /// [`ImageSampler::fill`] of an image whose mask is on its grid: the
    /// opacity of each pixel averaged with its texels.
    fn fused(
        &self,
        (x, y, len): (u32, u32, usize),
        lanes: usize,
        out: &mut SpanColors,
        shape: Option<&mut Vec<f64>>,
    ) {
        let channels = lanes - 2;
        let stride = lanes + 1;
        let scales_shape = self.mask.as_ref().is_some_and(|mask| mask.scales_shape);
        out.colors.resize(len * stride, 0.0);
        self.texels((x, y, len), lanes, &mut out.colors);
        let mut shape = shape;
        if let Some(shape) = shape.as_deref_mut() {
            shape.clear();
        }
        for i in 0..len {
            let base = i * stride;
            let Some((&factor, values)) = out.colors[base..base + stride].split_first() else {
                continue;
            };
            let mut color = [0.0; geometry::MAX_LANES];
            let color = &mut color[..channels];
            texels::copy_texel(color, values);
            let (mut alpha, mut own) = (values[channels], values[channels + 1]);
            for value in color.iter_mut() {
                *value *= factor;
            }
            alpha *= factor;
            if scales_shape {
                own *= factor;
            }
            if alpha != 1.0 {
                for value in color.iter_mut() {
                    *value = if alpha > 0.0 { *value / alpha } else { 0.0 };
                }
            }
            texels::copy_texel(&mut out.colors[i * channels..], color);
            out.alpha.push(alpha);
            if let Some(shape) = shape.as_deref_mut() {
                shape.push(own);
            }
        }
        out.colors.truncate(len * channels);
    }

    /// Writes the summed texels of the span's pixels, `lanes` values each.
    fn texels(&self, span: (u32, u32, usize), lanes: usize, out: &mut [f64]) {
        let image = &self.image.image;
        match &self.paint {
            Paint::Stencil { table, .. } => self.table(span, (table, lanes, false), out),
            Paint::Colors(paint) => match &paint.1 {
                Strategy::Table(table, opaque) => self.table(span, (table, lanes, *opaque), out),
                Strategy::Rgb(tables) => {
                    if let Samples::Bytes(bytes) = &image.samples {
                        let texels = RgbTexels {
                            pixels: bytes.as_chunks::<3>().0,
                            tables,
                        };
                        self.sample(span, &texels, out);
                    } else {
                        // Planes sampled by a placement that average codes
                        // do not cover convert each sample.
                        let texels = SampleTexels::new(image, &paint.0, None);
                        self.sample(span, &texels, out);
                    }
                }
                Strategy::Memo(memo) => {
                    let colors = &paint.0;
                    let mask = self.mask.as_ref().map(|m| (&m.image.image, &m.opacity));
                    match MemoTexels::new(image, colors, mask, memo) {
                        Some(texels) => self.sample(span, &texels, out),
                        None => self.general(span, colors, out),
                    }
                }
                Strategy::General => self.general(span, &paint.0, out),
            },
        }
    }

    /// Every texel computed from its sample, the Matte removed at the soft
    /// mask's opacity.
    fn general(&self, span: (u32, u32, usize), colors: &Colors, out: &mut [f64]) {
        let matte = self
            .mask
            .as_ref()
            .filter(|_| colors.matte.is_some())
            .map(|mask| (&mask.image.image, &mask.opacity));
        let texels = SampleTexels::new(&self.image.image, colors, matte);
        self.sample(span, &texels, out);
    }

    fn table(
        &self,
        span: (u32, u32, usize),
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
                self.sample(span, &texels, out);
            }
            Samples::Packed(packed) => {
                let texels = TableTexels {
                    codes: PackedCodes(packed),
                    table,
                    lanes,
                    opaque,
                };
                self.sample(span, &texels, out);
            }
            Samples::Words(_) | Samples::Mixed { .. } | Samples::Planar(_) => {}
        }
    }
}

/// The separable sampling of texels placed by `geometry` for device columns
/// `columns`, where the placement is upright and not interpolated: an
/// interpolated pixel adds its four texels by their weights' products, as
/// any placement's does.
///
/// # Errors
///
/// A `Limit` error when the plan cannot be allocated.
fn texel_rows(geometry: &Geometry, columns: (u32, u32)) -> Result<Option<Separable>> {
    if geometry.mode() == Mode::Bilinear {
        return Ok(None);
    }
    Separable::new(geometry, columns)
}

/// The contour sampling of a turned placement that minifies, over device
/// columns `columns`, of texels of at most `lanes` values.
fn turned(geometry: &Geometry, columns: (u32, u32), lanes: usize) -> Option<Contour> {
    (geometry.mode() == Mode::Area).then(|| Contour::new(geometry, columns, lanes))
}

/// Writes the summed texels of device pixels `x .. x + len` of row `y`,
/// `texels.lanes()` values each, into `out`: separably where the placement
/// is upright, by contours where a turned placement minifies, pixel by
/// pixel through `geometry` otherwise. The alpha and shape of opaque
/// texels are 1 where a pixel's footprint meets the image and 0 elsewhere.
fn sample(
    (separable, contour, geometry): (Option<&Separable>, Option<&Contour>, &Geometry),
    (x, y, len): (u32, u32, usize),
    texels: &impl Texels,
    out: &mut [f64],
) {
    let Some(separable) = separable else {
        match contour {
            Some(contour) => contour.fill((x, y, len), texels, out),
            None => geometry.fill((x, y, len), texels, out),
        }
        return;
    };
    let lanes = texels.lanes();
    let opaque = texels.opaque();
    let rows = TexelRows {
        texels,
        width: geometry.size().0,
        values: lanes - if opaque { 2 } else { 0 },
    };
    let n = rows.values;
    if separable.nearest() {
        separable.gather(&rows, (x, y, len), |pixels| {
            let sources = pixels.chunks_exact(n.max(1));
            for (pixel, source) in out.chunks_exact_mut(lanes).zip(sources) {
                if let Some((values, rest)) = pixel.split_at_mut_checked(n) {
                    values.copy_from_slice(source);
                    rest.fill(1.0);
                }
            }
        });
        return;
    }
    out.fill(0.0);
    let finish = |out: &mut [f64], covered: &[bool]| {
        if lanes > n {
            for (pixel, &covered) in out.chunks_exact_mut(lanes).zip(covered) {
                if let Some(rest) = pixel.get_mut(n..) {
                    rest.fill(f64::from(u8::from(covered)));
                }
            }
        }
    };
    let into = Some((&mut *out, lanes));
    separable.average(&rows, (x, y, len), into, (|average| average, finish));
}

/// Rows of an image's texels, `values` per sample: every lane, or the
/// colour lanes of opaque texels; the image is `width` samples wide.
struct TexelRows<'a, T> {
    texels: &'a T,
    width: u32,
    values: usize,
}

impl<T: Texels> Rows for TexelRows<'_, T> {
    type Value = f64;

    fn width(&self) -> usize {
        self.values
    }

    fn row<'c>(
        &'c self,
        y: usize,
        (first, end): (usize, usize),
        cache: &'c mut RowCache<f64>,
    ) -> &'c [f64] {
        let (Ok(row), Ok(start), Ok(stop)) =
            (u32::try_from(y), u32::try_from(first), u32::try_from(end))
        else {
            return &[];
        };
        cache.get((y, (first, end)), self.values, |out, _| {
            self.texels.row((row, self.width), (start, stop), out);
            true
        })
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
        convert: Option<(Box<dyn Convert>, bool)>,
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
                let fused = (image.image.width, image.image.height)
                    == (decoded.image.width, decoded.image.height);
                Some(MaskSampling {
                    opacity: Opacity::new(&image.image, decode),
                    separable: texel_rows(&geometry, columns)?,
                    contour: turned(&geometry, columns, 1),
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
        let codes = match (&mask, fast_lanes(&paint, &decoded.image)) {
            (None, Some(lanes)) => codes::Codes::new(lanes, &decoded.image, &geometry, columns)?,
            _ => None,
        };
        let separable = match codes {
            Some(_) => None,
            None => texel_rows(&geometry, columns)?,
        };
        let lanes = match &paint {
            Paint::Stencil { table, .. } => table.len() / 2,
            Paint::Colors(paint) => paint.0.lanes(),
        };
        let contour = turned(&geometry, columns, lanes + 1);
        Ok((
            Self {
                image: decoded,
                geometry,
                paint,
                mask,
                painted,
                shape_is_alpha,
                codes,
                separable,
                contour,
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
            self.codes.as_ref().map_or(0, codes::Codes::bytes),
            self.separable.as_ref().map_or(0, Separable::bytes),
            self.mask
                .as_ref()
                .and_then(|mask| mask.separable.as_ref())
                .map_or(0, Separable::bytes),
            self.contour.as_ref().map_or(0, Contour::bytes),
            self.mask
                .as_ref()
                .and_then(|mask| mask.contour.as_ref())
                .map_or(0, Contour::bytes),
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
            let planes =
                self.codes.is_none() && matches!(self.image.image.samples, Samples::Planar(_));
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
