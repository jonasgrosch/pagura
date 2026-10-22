//! What one source sample contributes (8.9.5.2, 8.9.6, 11.6.5.3): its
//! decoded components, an Indexed lookup, colour-key masking, embedded
//! opacity and Matte removal, converted to working colours and
//! premultiplied by its alpha, with alpha and shape.
//!
//! Tables and caches hold texels computed by the same per-sample function,
//! so they are exact by construction: a one-component image of at most
//! eight bits gets a table per code, a DeviceRGB image into an RGB blending
//! space one per channel (that conversion is the identity with clipping,
//! 10.4.2), an eight-bit image with a Matte a table per code and opacity
//! code filled as samples need it, images of up to four components of at
//! most eight bits a cache by sample tuple.

use super::geometry::{MAX_LANES, Texels, sample_index, sample_row};
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
    /// The channels every colour paints; they do not depend on the values.
    fn painted(&self) -> Painted;
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

    fn painted(&self) -> Painted {
        Converter::painted(self)
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
        self.code(mask.samples.get(index).unwrap_or(0))
    }

    /// The opacity of `code`, through the table when there is one.
    fn code(&self, code: u16) -> f64 {
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
    /// A table per code of a one-component image of at most eight bits,
    /// and whether every entry is opaque.
    Table(Vec<f64>, bool),
    /// DeviceRGB 8-bit samples into an RGB blending space: one table per
    /// channel.
    Rgb(Box<[[f64; 256]; 3]>),
    /// Texels cached by their sample tuple.
    Cached(Box<Mutex<Cache>>),
    /// Eight-bit samples without their soft mask's Matte, by code and
    /// opacity code.
    Unmatte(Box<Mutex<Unmatted>>),
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
            let opaque = opaque_table(&table, lanes);
            return (Self::Table(table, opaque), usize::from(codes));
        }
        if rgb
            && components == 3
            && bits == [8, 8, 8]
            && !per_pixel
            && colors.key.is_none()
            && colors.lookup.is_none()
            && lanes == 5
            && matches!(image.samples, Samples::Bytes(_) | Samples::Planar(_))
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
        let channels = lanes - 2;
        if colors.matte.is_some()
            && colors.embedded.is_none()
            && colors.key.is_none()
            && colors.lookup.is_none()
            && bits.len() == components
            && bits.iter().all(|&b| b == 8)
            && matches!(image.samples, Samples::Bytes(_))
            && (components == 1 && channels <= 4 || components == 3 && rgb && channels == 3)
        {
            return (
                Self::Unmatte(Box::new(Mutex::new(Unmatted::new(channels)))),
                0,
            );
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

    /// Bytes the strategy's tables hold.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Table(table, _) => size_of_val::<[f64]>(table),
            Self::Rgb(_) => size_of::<[[f64; 256]; 3]>(),
            Self::Cached(cache) => cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values
                .len()
                .saturating_mul(size_of::<f64>())
                .saturating_add(size_of::<Cache>()),
            // The most the table holds once every code meets every
            // opacity code.
            Self::Unmatte(memo) => memo.lock().unwrap_or_else(PoisonError::into_inner).bound(),
            Self::General => 0,
        }
    }

    /// Whether texels convert colours while painting.
    pub(super) fn converts_while_painting(&self) -> bool {
        matches!(self, Self::Cached(_) | Self::Unmatte(_) | Self::General)
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
    /// Whether every entry's alpha and shape are 1.
    pub(super) opaque: bool,
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

    fn opaque(&self) -> bool {
        self.opaque
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        for (out, value) in out.iter_mut().zip(self.texel(index)) {
            *out = *value;
        }
    }

    fn add(&self, index: usize, weight: f64, sum: &mut [f64]) {
        for (sum, value) in sum.iter_mut().zip(self.texel(index)) {
            *sum += value * weight;
        }
    }
}

/// Whether every texel of `table`, `lanes` values each, has alpha and
/// shape 1.
pub(super) fn opaque_table(table: &[f64], lanes: usize) -> bool {
    lanes >= 2
        && table
            .chunks_exact(lanes)
            .all(|texel| texel[lanes - 2..] == [1.0, 1.0])
}

/// Opaque DeviceRGB texels from per-channel tables.
pub(super) struct RgbTexels<'a> {
    pub(super) pixels: &'a [[u8; 3]],
    pub(super) tables: &'a [[f64; 256]; 3],
}

impl RgbTexels<'_> {
    fn texel(&self, index: usize) -> [f64; 3] {
        let [red, green, blue] = self.pixels.get(index).copied().unwrap_or_default();
        [
            self.tables[0][usize::from(red)],
            self.tables[1][usize::from(green)],
            self.tables[2][usize::from(blue)],
        ]
    }
}

impl Texels for RgbTexels<'_> {
    fn lanes(&self) -> usize {
        5
    }

    fn opaque(&self) -> bool {
        true
    }

    fn row(&self, (row, width): (u32, u32), (first, end): (u32, u32), out: &mut [f64]) {
        let base = u64::from(row) * u64::from(width);
        let (start, stop) = (
            sample_index(base + u64::from(first)),
            sample_index(base + u64::from(end)),
        );
        match self.pixels.get(start..stop) {
            Some(pixels) if out.len() == pixels.len() * 3 => {
                for (out, &[red, green, blue]) in out.as_chunks_mut::<3>().0.iter_mut().zip(pixels)
                {
                    *out = [
                        self.tables[0][usize::from(red)],
                        self.tables[1][usize::from(green)],
                        self.tables[2][usize::from(blue)],
                    ];
                }
            }
            _ => sample_row(self, (row, width), (first, end), out),
        }
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        for (out, value) in out.iter_mut().zip(self.texel(index)) {
            *out = value;
        }
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
    /// One texel's values while it is weighted.
    scratch: RefCell<[f64; MAX_LANES]>,
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
            scratch: RefCell::new([0.0; MAX_LANES]),
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
        let mut scratch = self.scratch.borrow_mut();
        let texel = &mut scratch[..sum.len()];
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

    fn row(&self, (row, width): (u32, u32), (first, end): (u32, u32), out: &mut [f64]) {
        let base = u64::from(row) * u64::from(width);
        let (start, stop) = (
            sample_index(base + u64::from(first)),
            sample_index(base + u64::from(end)),
        );
        match (&self.mask.samples, &self.opacity.table) {
            (Samples::Bytes(bytes), Some(table)) if out.len() == stop.saturating_sub(start) => {
                for (out, &code) in out.iter_mut().zip(bytes.get(start..stop).unwrap_or(&[])) {
                    *out = table.get(usize::from(code)).copied().unwrap_or(0.0);
                }
            }
            _ => sample_row(self, (row, width), (first, end), out),
        }
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

/// The texels of eight-bit samples without their soft mask's Matte
/// (11.6.5.3), each computed once per code and opacity code: every channel
/// of a one-component image's code, or one channel per code of a DeviceRGB
/// image into an RGB blending space, whose channels convert independently
/// (10.4.2). Their alpha and shape are 1.
#[derive(Debug)]
pub(super) struct Unmatted {
    /// Values per code.
    channels: usize,
    /// Per opacity code, the values of the codes seen with it.
    blocks: Vec<Option<Box<Block>>>,
}

/// The values of the codes seen with one opacity code: `channels` per
/// code, NaN until known.
#[derive(Debug)]
struct Block {
    values: Vec<f64>,
}

impl Unmatted {
    fn new(channels: usize) -> Self {
        Self {
            channels,
            blocks: std::iter::repeat_with(|| None).take(256).collect(),
        }
    }

    /// Bytes the table holds once every code has met every opacity code.
    fn bound(&self) -> usize {
        let block = size_of::<Block>().saturating_add(256 * self.channels * size_of::<f64>());
        size_of::<Self>().saturating_add(256 * (size_of::<Option<Box<Block>>>() + block))
    }

    /// The values of the codes seen with opacity code `mask`.
    fn block(&mut self, mask: u8) -> Option<&mut Block> {
        let channels = self.channels;
        let block = self
            .blocks
            .get_mut(usize::from(mask))?
            .get_or_insert_with(|| {
                Box::new(Block {
                    values: vec![f64::NAN; 256 * channels],
                })
            });
        Some(block)
    }
}

impl Block {
    /// The `channels` values of `code`, computed by `compute` into a texel
    /// unless known.
    #[inline]
    fn values(&mut self, code: u8, channels: usize, compute: impl FnOnce(&mut [f64])) -> &[f64] {
        let at = usize::from(code) * channels;
        let Some(values) = self.values.get_mut(at..at + channels) else {
            return &[];
        };
        // A value that computes to NaN is computed again, alike.
        if values.first().is_some_and(|value| value.is_nan()) {
            let mut texel = [0.0; MAX_LANES];
            compute(&mut texel[..channels + 2]);
            values.copy_from_slice(&texel[..channels]);
        }
        values
    }
}

/// Texels of eight-bit samples without the Matte, from [`Unmatted`].
pub(super) struct UnmatteTexels<'a> {
    samples: &'a [u8],
    components: usize,
    /// The soft mask's samples, on the image's grid.
    mask: &'a [u8],
    opacity: &'a Opacity,
    colors: &'a Colors,
    memo: RefCell<MutexGuard<'a, Unmatted>>,
}

impl<'a> UnmatteTexels<'a> {
    /// The texels of `image`, whose soft mask `mask` has its grid, or `None`
    /// when either's samples are not bytes.
    pub(super) fn new(
        image: &'a DecodedImage,
        colors: &'a Colors,
        (mask, opacity): (&'a DecodedImage, &'a Opacity),
        memo: &'a Mutex<Unmatted>,
    ) -> Option<Self> {
        let (Samples::Bytes(samples), Samples::Bytes(codes)) = (&image.samples, &mask.samples)
        else {
            return None;
        };
        let pixels = u64::from(image.width) * u64::from(image.height);
        (u64::try_from(codes.len()).ok() == Some(pixels)).then(|| Self {
            samples,
            components: usize::from(image.components),
            mask: codes,
            opacity,
            colors,
            memo: RefCell::new(memo.lock().unwrap_or_else(PoisonError::into_inner)),
        })
    }

    /// Writes the colour of sample `index` into `out`.
    #[inline]
    fn write(&self, memo: &mut Unmatted, index: usize, out: &mut [f64]) {
        let mask = self.mask.get(index).copied().unwrap_or(0);
        let channels = memo.channels;
        if let Some(block) = memo.block(mask) {
            self.unmatte((block, channels), (mask, index), out);
        }
    }

    /// Writes the colour of sample `index`, whose opacity code is `mask`,
    /// into `out`, from `block`, the values of that opacity code.
    #[inline]
    fn unmatte(
        &self,
        (block, channels): (&mut Block, usize),
        (mask, index): (u8, usize),
        out: &mut [f64],
    ) {
        let compute = |code: u8, texel: &mut [f64]| {
            let codes = [u16::from(code); 3];
            let extra = Extra {
                embedded: None,
                mask: Some(self.opacity.code(u16::from(mask))),
            };
            self.colors
                .texel(&codes[..self.components.min(3)], extra, texel);
        };
        if self.components == 1 {
            let code = self.samples.get(index).copied().unwrap_or(0);
            let values = block.values(code, channels, |texel| compute(code, texel));
            for (out, &value) in out.iter_mut().zip(values) {
                *out = value;
            }
        } else {
            let start = index.saturating_mul(3);
            let codes = self.samples.get(start..start + 3).unwrap_or(&[0; 3]);
            for (channel, (out, &code)) in out.iter_mut().zip(codes).enumerate() {
                let values = block.values(code, channels, |texel| compute(code, texel));
                *out = values.get(channel).copied().unwrap_or(0.0);
            }
        }
    }
}

impl Texels for UnmatteTexels<'_> {
    fn lanes(&self) -> usize {
        self.colors.lanes()
    }

    fn opaque(&self) -> bool {
        true
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        self.write(&mut self.memo.borrow_mut(), index, out);
    }

    fn add(&self, index: usize, weight: f64, sum: &mut [f64]) {
        let mut texel = [0.0; MAX_LANES];
        let texel = &mut texel[..sum.len()];
        self.write(&mut self.memo.borrow_mut(), index, texel);
        for (sum, value) in sum.iter_mut().zip(texel.iter()) {
            *sum += value * weight;
        }
    }

    fn row(&self, (row, width): (u32, u32), (first, end): (u32, u32), out: &mut [f64]) {
        let count = usize::try_from(end.saturating_sub(first)).unwrap_or(0);
        let Some(stride) = out.len().checked_div(count).filter(|&n| n > 0) else {
            return;
        };
        let mut memo = self.memo.borrow_mut();
        let channels = memo.channels;
        let start = sample_index(u64::from(row) * u64::from(width) + u64::from(first));
        let masks = self.mask.get(start..start + count).unwrap_or(&[]);
        let mut pixels = out.chunks_exact_mut(stride);
        // Runs of one opacity code share their block.
        let mut cell = 0;
        while let Some(&mask) = masks.get(cell) {
            let run = masks.get(cell..).map_or(1, |rest| {
                rest.iter().take_while(|&&code| code == mask).count()
            });
            let Some(block) = memo.block(mask) else {
                return;
            };
            for (index, values) in (start + cell..start + cell + run).zip(pixels.by_ref()) {
                self.unmatte((block, channels), (mask, index), values);
            }
            cell += run.max(1);
        }
    }
}
