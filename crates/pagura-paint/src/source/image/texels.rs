//! What one source sample contributes (8.9.5.2, 8.9.6, 11.6.5.3): its
//! decoded components, an Indexed lookup, colour-key masking, embedded
//! opacity and Matte removal, converted to working colours and
//! premultiplied by its alpha, with alpha and shape.
//!
//! Tables and caches hold texels computed by the same per-sample function,
//! so they are exact by construction: a one-component image of at most
//! eight bits gets a table per code, a DeviceRGB image into an RGB blending
//! space one per channel (that conversion is the identity with clipping,
//! 10.4.2), images of up to four components of at most eight bits a cache
//! by sample tuple.

use super::geometry::{MAX_LANES, Texels};
use pagura_color::{ColorSpace, Converter, Painted};
use pagura_image::{DecodedImage, ImageKind, MaskKind, Samples};
use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The most components an image has (DeviceN's 32 colorants).
const MAX_COMPONENTS: usize = pagura_color::MAX_COMPONENTS;

/// A colour conversion into working colours, prepared once.
pub(super) trait Convert: Send + Sync + std::fmt::Debug {
    /// Values per converted colour.
    fn channels(&self) -> usize;
    /// Converts `components` into `output`, returning the channels painted.
    fn convert(&self, components: &[f64], output: &mut [f64]) -> Painted;
    /// The `function_work` units one conversion costs at most.
    fn cost(&self) -> usize;
}

impl Convert for Converter {
    fn channels(&self) -> usize {
        self.target().channels()
    }

    fn convert(&self, components: &[f64], output: &mut [f64]) -> Painted {
        Converter::convert(self, components, output)
    }

    fn cost(&self) -> usize {
        Converter::cost(self)
    }
}

/// An Indexed space's lookup into its base space (8.6.6.3).
#[derive(Debug, Clone)]
pub(super) struct Lookup {
    /// Base components per entry.
    components: usize,
    high: u8,
    bytes: std::sync::Arc<[u8]>,
    /// How a lookup byte maps to a base component.
    scale: LookupScale,
}

/// How lookup bytes become base-space components.
#[derive(Debug, Clone)]
enum LookupScale {
    /// `byte / 255`.
    Unit,
    /// An ICC-based base's `Range` of each component.
    Ranges([[f64; 2]; 4]),
    /// A Lab base: `L` in 0..100 and `a`, `b` in their ranges.
    Lab([f64; 4]),
}

impl Lookup {
    /// The lookup of `space`, if it is Indexed, and the base space.
    pub(super) fn of(space: &ColorSpace) -> Option<(Self, &ColorSpace)> {
        let ColorSpace::Indexed(indexed) = space else {
            return None;
        };
        let base = indexed.base.as_ref();
        let scale = match base {
            ColorSpace::Icc(icc) => LookupScale::Ranges(icc.range),
            ColorSpace::Calibrated(cie) => {
                cie.lab_range().map_or(LookupScale::Unit, LookupScale::Lab)
            }
            _ => LookupScale::Unit,
        };
        Some((
            Self {
                components: base.components(),
                high: indexed.high,
                bytes: std::sync::Arc::clone(&indexed.lookup),
                scale,
            },
            base,
        ))
    }

    /// Replaces the index in `values[0]` by its base components, returning
    /// their count.
    fn expand(&self, values: &mut [f64; MAX_COMPONENTS]) -> usize {
        let index = values[0].round().clamp(0.0, f64::from(self.high));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "index rounded and clamped to 0..255"
        )]
        let index = index as usize;
        let count = self.components.min(MAX_COMPONENTS);
        let start = index.saturating_mul(count);
        let entry = self
            .bytes
            .get(start..start.saturating_add(count))
            .unwrap_or(&[]);
        let byte = |i: usize| f64::from(entry.get(i).copied().unwrap_or(0));
        for (i, value) in values.iter_mut().enumerate().take(count) {
            let n = byte(i) / 255.0;
            *value = match &self.scale {
                LookupScale::Unit => n,
                LookupScale::Ranges(ranges) => {
                    let [low, high] = ranges.get(i).copied().unwrap_or([0.0, 1.0]);
                    low * (1.0 - byte(i) / 255.0) + high * (byte(i) / 255.0)
                }
                LookupScale::Lab(range) => match i {
                    0 => 100.0 * n,
                    1 => range[0] + n * (range[1] - range[0]),
                    _ => range[2] + n * (range[3] - range[2]),
                },
            };
        }
        count
    }
}

/// How a colour image's samples become texels.
#[derive(Debug)]
pub(super) struct Colors {
    /// Decode interval of each component.
    pub(super) decode: Vec<[f64; 2]>,
    /// The largest code of each component.
    pub(super) maxima: Vec<f64>,
    pub(super) lookup: Option<Lookup>,
    /// Colour-key ranges of raw codes (8.9.6.4).
    pub(super) key: Option<Vec<[u16; 2]>>,
    /// The largest code of embedded opacity, and whether colours are
    /// premultiplied by it.
    pub(super) embedded: Option<(f64, bool)>,
    /// The Matte colour of a soft mask with the image's dimensions.
    pub(super) matte: Option<Vec<f64>>,
    pub(super) alpha_is_shape: bool,
    pub(super) convert: Box<dyn Convert>,
}

/// Per-sample inputs other than the codes: embedded opacity and the soft
/// mask's opacity at the same sample, for Matte removal.
#[derive(Debug, Clone, Copy, Default)]
struct Extra {
    embedded: Option<u16>,
    mask: Option<f64>,
}

impl Colors {
    /// Values per texel: colour channels, alpha and shape.
    pub(super) fn lanes(&self) -> usize {
        self.convert.channels() + 2
    }

    /// The texel of one sample's `codes`.
    fn texel(&self, codes: &[u16], extra: Extra, out: &mut [f64]) {
        let mut values = [0.0; MAX_COMPONENTS];
        for (((value, &code), &[low, high]), &maximum) in values
            .iter_mut()
            .zip(codes)
            .zip(&self.decode)
            .zip(&self.maxima)
        {
            *value = low + f64::from(code) / maximum * (high - low);
        }
        let count = match &self.lookup {
            Some(lookup) => lookup.expand(&mut values),
            None => codes.len(),
        };
        let mut shape = 1.0;
        let mut alpha = 1.0;
        if let Some(key) = &self.key
            && key
                .iter()
                .zip(codes)
                .all(|(&[low, high], &code)| code >= low && code <= high)
            && key.len() == codes.len()
        {
            shape = 0.0;
            alpha = 0.0;
        }
        if let (Some((maximum, premultiplied)), Some(code)) = (self.embedded, extra.embedded) {
            let opacity = f64::from(code) / maximum;
            alpha *= opacity;
            if self.alpha_is_shape {
                shape *= opacity;
            }
            if premultiplied {
                for value in values.iter_mut().take(count) {
                    *value = if opacity > 0.0 { *value / opacity } else { 0.0 };
                }
            }
        }
        if let (Some(matte), Some(opacity)) = (&self.matte, extra.mask) {
            for (value, &matte) in values.iter_mut().zip(matte) {
                *value = if opacity > 0.0 {
                    matte + (*value - matte) / opacity
                } else {
                    0.0
                };
            }
        }
        let channels = self.convert.channels();
        let (color, rest) = out.split_at_mut(channels);
        self.convert.convert(&values[..count], color);
        for value in color.iter_mut() {
            *value *= alpha;
        }
        if let [a, s, ..] = rest {
            *a = alpha;
            *s = shape;
        }
    }

    /// The texel of the one-component code `code`, for a table.
    fn code_texel(&self, code: u16, out: &mut [f64]) {
        self.texel(&[code], Extra::default(), out);
    }
}

/// Opacities of a soft or stencil mask's samples (8.9.6.2–8.9.6.3).
#[derive(Debug)]
pub(super) struct Opacity {
    /// A stencil's painting code polarity, or a soft mask's decode.
    kind: OpacityKind,
    /// The opacity of each code of a one-component mask of at most eight
    /// bits.
    table: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Copy)]
enum OpacityKind {
    /// Sample 1 paints when `inverted`, sample 0 otherwise.
    Stencil { inverted: bool },
    /// Decoded through the interval and clipped to 0..1.
    Soft { decode: [f64; 2], maximum: f64 },
}

impl Opacity {
    /// The opacities of `mask`, decoded with `decode`.
    pub(super) fn new(mask: &DecodedImage, decode: [f64; 2]) -> Self {
        let kind = match mask.kind {
            ImageKind::Stencil { inverted } => OpacityKind::Stencil { inverted },
            ImageKind::Color { .. } | ImageKind::EmbeddedColor { .. } => OpacityKind::Soft {
                decode,
                maximum: maximum(mask.component_bits(0).unwrap_or(8)),
            },
        };
        let mut opacity = Self { kind, table: None };
        let bits = mask.component_bits(0).unwrap_or(16);
        if (1..=8).contains(&bits) && matches!(mask.samples, Samples::Bytes(_) | Samples::Packed(_))
        {
            opacity.table = Some((0..1u16 << bits).map(|code| opacity.of(code)).collect());
        }
        opacity
    }

    /// The opacity of one code.
    fn of(&self, code: u16) -> f64 {
        match self.kind {
            OpacityKind::Stencil { inverted } => f64::from(u8::from((code == 1) == inverted)),
            OpacityKind::Soft {
                decode: [low, high],
                maximum,
            } => (low + f64::from(code) / maximum * (high - low)).clamp(0.0, 1.0),
        }
    }

    /// The opacity of sample `index` of `mask`.
    pub(super) fn at(&self, mask: &DecodedImage, index: usize) -> f64 {
        let code = mask.samples.get(index).unwrap_or(0);
        match &self.table {
            Some(table) => table.get(usize::from(code)).copied().unwrap_or(0.0),
            None => self.of(code),
        }
    }
}

/// The largest code of a component of `bits` bits.
pub(super) fn maximum(bits: u8) -> f64 {
    f64::from(u16::MAX >> (16 - bits.clamp(1, 16)))
}

/// How a prepared image's texels are produced.
#[derive(Debug)]
pub(super) enum Strategy {
    /// A table per code of a one-component image of at most eight bits.
    Table(Vec<f64>),
    /// DeviceRGB 8-bit samples into an RGB blending space: one table per
    /// channel.
    Rgb(Box<[[f64; 256]; 3]>),
    /// Texels cached by their sample tuple.
    Cached(Box<Mutex<Cache>>),
    /// Every texel computed from its sample.
    General,
}

/// Texels of recently seen sample tuples, at most four codes of up to
/// eight bits each.
#[derive(Debug)]
pub(super) struct Cache {
    keys: [Option<u32>; 256],
    values: Vec<f64>,
}

impl Strategy {
    /// The strategy for `image` painted with `colors`, and the conversions
    /// preparing it costs.
    pub(super) fn new(image: &DecodedImage, colors: &Colors, rgb: bool) -> (Self, usize) {
        let components = usize::from(image.components);
        let bits: Vec<u8> = (0..components)
            .filter_map(|i| image.component_bits(i))
            .collect();
        let narrow = bits.len() == components && bits.iter().all(|b| (1..=8).contains(b));
        let per_pixel = colors.matte.is_some() || colors.embedded.is_some();
        let lanes = colors.lanes();
        if components == 1
            && narrow
            && !per_pixel
            && matches!(image.samples, Samples::Bytes(_) | Samples::Packed(_))
        {
            let codes = 1u16 << bits[0];
            let mut table = vec![0.0; usize::from(codes) * lanes];
            for (code, texel) in (0..codes).zip(table.chunks_exact_mut(lanes)) {
                colors.code_texel(code, texel);
            }
            return (Self::Table(table), usize::from(codes));
        }
        if rgb
            && components == 3
            && bits == [8, 8, 8]
            && !per_pixel
            && colors.key.is_none()
            && colors.lookup.is_none()
            && lanes == 5
            && matches!(image.samples, Samples::Bytes(_))
        {
            let mut tables = Box::new([[0.0; 256]; 3]);
            let mut texel = [0.0; 5];
            for code in 0..=255u8 {
                let code = u16::from(code);
                colors.texel(&[code, code, code], Extra::default(), &mut texel);
                for (table, &value) in tables.iter_mut().zip(&texel[..3]) {
                    table[usize::from(code)] = value;
                }
            }
            return (Self::Rgb(tables), 256);
        }
        if (1..=4).contains(&components) && narrow && !per_pixel {
            return (
                Self::Cached(Box::new(Mutex::new(Cache {
                    keys: [None; 256],
                    values: vec![0.0; 256 * lanes],
                }))),
                0,
            );
        }
        (Self::General, 0)
    }

    /// Whether texels convert colours while painting.
    pub(super) fn converts_while_painting(&self) -> bool {
        matches!(self, Self::Cached(_) | Self::General)
    }
}

/// The samples of a one-component image as codes.
pub(super) trait Codes {
    fn code(&self, index: usize) -> u16;
}

/// Codes stored one per byte.
pub(super) struct ByteCodes<'a>(pub(super) &'a [u8]);

impl Codes for ByteCodes<'_> {
    fn code(&self, index: usize) -> u16 {
        u16::from(self.0.get(index).copied().unwrap_or(0))
    }
}

/// One-bit codes in packed rows.
pub(super) struct PackedCodes<'a>(pub(super) &'a pagura_image::deferred::Packed);

impl Codes for PackedCodes<'_> {
    fn code(&self, index: usize) -> u16 {
        self.0.get(index).unwrap_or(0)
    }
}

/// Texels looked up by code.
pub(super) struct TableTexels<'a, C> {
    pub(super) codes: C,
    pub(super) table: &'a [f64],
    pub(super) lanes: usize,
}

impl<C: Codes> TableTexels<'_, C> {
    fn texel(&self, index: usize) -> &[f64] {
        let start = usize::from(self.codes.code(index)) * self.lanes;
        self.table.get(start..start + self.lanes).unwrap_or(&[])
    }
}

impl<C: Codes> Texels for TableTexels<'_, C> {
    fn lanes(&self) -> usize {
        self.lanes
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        out.copy_from_slice(self.texel(index));
    }

    fn add(&self, index: usize, weight: f64, sum: &mut [f64]) {
        for (sum, value) in sum.iter_mut().zip(self.texel(index)) {
            *sum += value * weight;
        }
    }
}

/// Opaque DeviceRGB texels from per-channel tables.
pub(super) struct RgbTexels<'a> {
    pub(super) pixels: &'a [[u8; 3]],
    pub(super) tables: &'a [[f64; 256]; 3],
}

impl RgbTexels<'_> {
    fn texel(&self, index: usize) -> [f64; 5] {
        let [red, green, blue] = self.pixels.get(index).copied().unwrap_or_default();
        [
            self.tables[0][usize::from(red)],
            self.tables[1][usize::from(green)],
            self.tables[2][usize::from(blue)],
            1.0,
            1.0,
        ]
    }
}

impl Texels for RgbTexels<'_> {
    fn lanes(&self) -> usize {
        5
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        out.copy_from_slice(&self.texel(index));
    }

    fn add(&self, index: usize, weight: f64, sum: &mut [f64]) {
        for (sum, value) in sum.iter_mut().zip(self.texel(index)) {
            *sum += value * weight;
        }
    }
}

/// Texels computed from their samples, optionally cached by sample tuple.
pub(super) struct SampleTexels<'a> {
    pub(super) image: &'a DecodedImage,
    pub(super) colors: &'a Colors,
    /// The soft mask whose opacity removes the Matte.
    pub(super) matte: Option<(&'a DecodedImage, &'a Opacity)>,
    pub(super) cache: Option<RefCell<MutexGuard<'a, Cache>>>,
}

impl<'a> SampleTexels<'a> {
    pub(super) fn new(
        image: &'a DecodedImage,
        colors: &'a Colors,
        matte: Option<(&'a DecodedImage, &'a Opacity)>,
        cache: Option<&'a Mutex<Cache>>,
    ) -> Self {
        Self {
            image,
            colors,
            matte,
            cache: cache
                .map(|cache| RefCell::new(cache.lock().unwrap_or_else(PoisonError::into_inner))),
        }
    }

    fn texel(&self, index: usize, out: &mut [f64]) {
        let n = usize::from(self.image.components).min(MAX_COMPONENTS);
        let mut codes = [0u16; MAX_COMPONENTS];
        let start = index.saturating_mul(n);
        for (c, code) in codes.iter_mut().enumerate().take(n) {
            *code = self.image.samples.get(start + c).unwrap_or(0);
        }
        let codes = &codes[..n];
        if let Some(cache) = &self.cache {
            let mut cache = cache.borrow_mut();
            let key = codes
                .iter()
                .fold(0u32, |key, &code| (key << 8) | u32::from(code));
            let slot = if n == 1 {
                usize::from(key.to_le_bytes()[0])
            } else {
                usize::from(key.wrapping_mul(0x9e37_79b1).to_be_bytes()[0])
            };
            let lanes = out.len();
            let range = slot * lanes..(slot + 1) * lanes;
            if cache.keys[slot] != Some(key) {
                let Some(values) = cache.values.get_mut(range.clone()) else {
                    return;
                };
                self.colors.texel(codes, Extra::default(), values);
                cache.keys[slot] = Some(key);
            }
            if let Some(values) = cache.values.get(range) {
                out.copy_from_slice(values);
            }
            return;
        }
        let embedded = match &self.image.mask {
            MaskKind::Embedded(alpha) => alpha.samples.get(index),
            _ => None,
        };
        let mask = self.matte.map(|(mask, opacity)| opacity.at(mask, index));
        self.colors.texel(codes, Extra { embedded, mask }, out);
    }
}

impl Texels for SampleTexels<'_> {
    fn lanes(&self) -> usize {
        self.colors.lanes()
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        self.texel(index, out);
    }

    fn add(&self, index: usize, weight: f64, sum: &mut [f64]) {
        let mut texel = [0.0; MAX_LANES];
        let texel = &mut texel[..sum.len()];
        self.texel(index, texel);
        for (sum, value) in sum.iter_mut().zip(texel.iter()) {
            *sum += value * weight;
        }
    }
}

/// Mask opacities as one-lane texels.
pub(super) struct OpacityTexels<'a> {
    pub(super) mask: &'a DecodedImage,
    pub(super) opacity: &'a Opacity,
}

impl Texels for OpacityTexels<'_> {
    fn lanes(&self) -> usize {
        1
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        if let Some(value) = out.first_mut() {
            *value = self.opacity.at(self.mask, index);
        }
    }

    fn add(&self, index: usize, weight: f64, sum: &mut [f64]) {
        if let Some(value) = sum.first_mut() {
            *value += self.opacity.at(self.mask, index) * weight;
        }
    }
}
