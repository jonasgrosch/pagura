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
use pagura_color::{ColorSpace, Converter};
use pagura_image::{DecodedImage, ImageKind, MaskKind, Samples};
use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The most components an image has (DeviceN's 32 colorants).
const MAX_COMPONENTS: usize = pagura_color::MAX_COMPONENTS;

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
    pub(super) convert: Converter,
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
        self.convert.target().channels() + 2
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
        let channels = self.convert.target().channels();
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
    /// Texels converted once per distinct sample (its codes, with the soft
    /// mask's opacity code where a Matte is removed and the embedded
    /// opacity code), kept in a memo.
    Memo(Box<Mutex<Memo>>),
    /// Every texel computed from its sample.
    General,
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
        if let Some(key) = Key::new(image, colors) {
            let samples = u64::from(image.width) * u64::from(image.height);
            let entries = usize::try_from(samples)
                .unwrap_or(usize::MAX)
                .min(Memo::CAP);
            let opaque = colors.key.is_none() && colors.embedded.is_none();
            let held = if opaque { lanes - 2 } else { lanes };
            return (
                Self::Memo(Box::new(Mutex::new(Memo::new(
                    key,
                    (held, opaque),
                    entries,
                )))),
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
            Self::Memo(memo) => memo.lock().unwrap_or_else(PoisonError::into_inner).bound(),
            Self::General => 0,
        }
    }

    /// Whether texels convert colours while painting.
    pub(super) fn converts_while_painting(&self) -> bool {
        matches!(self, Self::Memo(_) | Self::General)
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

/// Texels computed from their samples.
pub(super) struct SampleTexels<'a> {
    pub(super) image: &'a DecodedImage,
    pub(super) colors: &'a Colors,
    /// The soft mask whose opacity removes the Matte.
    pub(super) matte: Option<(&'a DecodedImage, &'a Opacity)>,
    /// One texel's values while it is weighted.
    scratch: RefCell<[f64; MAX_LANES]>,
}

impl<'a> SampleTexels<'a> {
    pub(super) fn new(
        image: &'a DecodedImage,
        colors: &'a Colors,
        matte: Option<(&'a DecodedImage, &'a Opacity)>,
    ) -> Self {
        Self {
            image,
            colors,
            matte,
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

/// Texels with the opacity of a soft or stencil mask on their grid ahead of
/// their values, so that one footprint averages both: `[opacity, values..]`.
pub(super) struct Fused<'a, T> {
    pub(super) texels: &'a T,
    pub(super) mask: OpacityTexels<'a>,
    /// The texels of a row, read before they are interleaved.
    pub(super) values: RefCell<Vec<f64>>,
}

impl<T: Texels> Texels for Fused<'_, T> {
    fn lanes(&self) -> usize {
        self.texels.lanes() + 1
    }

    fn opaque(&self) -> bool {
        self.texels.opaque()
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        if let Some((opacity, values)) = out.split_first_mut() {
            self.mask.get(index, std::slice::from_mut(opacity));
            self.texels.get(index, values);
        }
    }

    fn add(&self, index: usize, weight: f64, sum: &mut [f64]) {
        if let Some((opacity, values)) = sum.split_first_mut() {
            self.mask.add(index, weight, std::slice::from_mut(opacity));
            self.texels.add(index, weight, values);
        }
    }

    fn row(&self, row: (u32, u32), cells: (u32, u32), out: &mut [f64]) {
        let count = usize::try_from(cells.1.saturating_sub(cells.0)).unwrap_or(0);
        let Some(stride) = out.len().checked_div(count).filter(|&n| n > 1) else {
            return;
        };
        let mut values = self.values.borrow_mut();
        values.clear();
        values.resize(count * (stride - 1), 0.0);
        self.texels.row(row, cells, &mut values);
        let mut opacities = [0.0; 256];
        let mut start = 0;
        while start < count {
            let take = (count - start).min(opacities.len());
            let first = cells
                .0
                .saturating_add(u32::try_from(start).unwrap_or(u32::MAX));
            let end = first.saturating_add(u32::try_from(take).unwrap_or(u32::MAX));
            self.mask.row(row, (first, end), &mut opacities[..take]);
            for ((pixel, &opacity), value) in out[start * stride..]
                .chunks_exact_mut(stride)
                .zip(&opacities[..take])
                .zip(values[start * (stride - 1)..].chunks_exact(stride - 1))
            {
                if let Some((first, rest)) = pixel.split_first_mut() {
                    *first = opacity;
                    copy_texel(rest, value);
                }
            }
            start += take;
        }
    }
}

/// The key of a sample of at most eight bits per code: its codes, then the
/// soft mask's opacity code where a Matte is removed, then the embedded
/// opacity code, eight bits each and at most six of them.
#[derive(Debug, Clone, Copy)]
pub(super) struct Key {
    components: usize,
    matte: bool,
    embedded: bool,
}

impl Key {
    /// The key of `image`'s samples painted with `colors`, when every code
    /// it holds has at most eight bits and they fit 64 bits.
    fn new(image: &DecodedImage, colors: &Colors) -> Option<Self> {
        let components = usize::from(image.components);
        let narrow = (0..components).all(|i| image.component_bits(i).is_some_and(|b| b <= 8));
        let embedded = match &image.mask {
            MaskKind::Embedded(alpha) => {
                if alpha.bits_per_component > 8 || !matches!(alpha.samples, Samples::Bytes(_)) {
                    return None;
                }
                true
            }
            _ => false,
        };
        let matte = colors.matte.is_some();
        let bytes = components + usize::from(matte) + usize::from(embedded);
        (narrow && bytes <= 6 && matches!(image.samples, Samples::Bytes(_))).then_some(Self {
            components,
            matte,
            embedded,
        })
    }
}

/// Texels by sample key: open addressing into slots that hold a key and
/// its `held` values together, so that a lookup reads one place. Keys have
/// at most 48 bits, so each is held exactly as an f64. The memo holds at
/// most [`Memo::CAP`] texels; a sample past them is converted without being
/// kept. Every texel held is its key's, so what the memo holds never
/// changes a value.
#[derive(Debug)]
pub(super) struct Memo {
    key: Key,
    /// Whether every texel has alpha and shape 1, which it then does not
    /// hold.
    opaque: bool,
    held: usize,
    /// The most texels the image's samples can make.
    bound: usize,
    /// Slots of `1 + held` values: the key (or [`FREE`]), then the texel.
    slots: Vec<f64>,
    len: usize,
    /// A texel converted past the cap.
    spare: Vec<f64>,
}

/// A free slot: no key is negative.
const FREE: f64 = -1.0;

impl Memo {
    /// The most texels kept.
    pub(super) const CAP: usize = 1 << 20;

    fn new(key: Key, (held, opaque): (usize, bool), bound: usize) -> Self {
        Self {
            key,
            opaque,
            held,
            bound,
            slots: Vec::new(),
            len: 0,
            spare: Vec::new(),
        }
    }

    /// Whether every texel has alpha and shape 1, which the memo then does
    /// not hold.
    pub(super) fn opaque(&self) -> bool {
        self.opaque
    }

    /// The most bytes the memo holds: slots for its texels at most half
    /// full, grown by doubling.
    fn bound(&self) -> usize {
        self.bound
            .min(Self::CAP)
            .saturating_mul(4)
            .saturating_mul((1 + self.held) * size_of::<f64>())
    }

    /// The `held` values of the texel of `key`, converted by `convert` into
    /// a texel of every lane unless held.
    #[inline]
    fn get(&mut self, key: u64, convert: impl FnOnce(&mut [f64])) -> &[f64] {
        let stride = 1 + self.held;
        if self.slots.is_empty() {
            self.slots = vec![FREE; stride << 12];
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "keys have at most 48 bits, which f64 holds exactly"
        )]
        let wanted = key as f64;
        let mut slot = self.slot(key, wanted);
        if self.slots.get(slot * stride).copied() != Some(wanted) {
            let mut texel = [0.0; MAX_LANES];
            convert(&mut texel[..self.held + 2]);
            if self.len >= Self::CAP {
                self.spare.clear();
                self.spare.extend_from_slice(&texel[..self.held]);
                return &self.spare;
            }
            if (self.len + 1) * 2 > self.slots.len() / stride {
                self.grow();
                slot = self.slot(key, wanted);
            }
            let at = slot * stride;
            if let Some(entry) = self.slots.get_mut(at..at + stride) {
                entry[0] = wanted;
                entry[1..].copy_from_slice(&texel[..self.held]);
                self.len += 1;
            }
        }
        let at = slot * stride + 1;
        self.slots.get(at..at + self.held).unwrap_or(&[])
    }

    /// The slot of `key` (held as `wanted`), or the free slot it goes into.
    #[inline]
    fn slot(&self, key: u64, wanted: f64) -> usize {
        let stride = 1 + self.held;
        let mask = self.slots.len() / stride - 1;
        let hash = key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 29;
        let mut slot = usize::try_from(hash).unwrap_or(0) & mask;
        while self
            .slots
            .get(slot * stride)
            .is_some_and(|&k| k != wanted && k != FREE)
        {
            slot = (slot + 1) & mask;
        }
        slot
    }

    fn grow(&mut self) {
        let stride = 1 + self.held;
        let size = self.slots.len() * 2;
        let slots = std::mem::replace(&mut self.slots, vec![FREE; size]);
        for entry in slots.chunks_exact(stride) {
            if entry[0] >= 0.0 {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a held key, a non-negative integer below 2^48"
                )]
                let key = entry[0] as u64;
                let at = self.slot(key, entry[0]) * stride;
                if let Some(target) = self.slots.get_mut(at..at + stride) {
                    target.copy_from_slice(entry);
                }
            }
        }
    }
}

/// Texels of eight-bit samples through their memo.
pub(super) struct MemoTexels<'a> {
    samples: &'a [u8],
    colors: &'a Colors,
    /// The soft mask's codes on the image's grid and their opacities, where
    /// a Matte is removed.
    matte: Option<(&'a [u8], &'a Opacity)>,
    /// The embedded opacity codes.
    embedded: Option<&'a [u8]>,
    memo: RefCell<MutexGuard<'a, Memo>>,
}

impl<'a> MemoTexels<'a> {
    /// The texels of `image` through `memo`, with the soft mask `mask` for
    /// Matte removal; `None` where the samples are not bytes.
    pub(super) fn new(
        image: &'a DecodedImage,
        colors: &'a Colors,
        mask: Option<(&'a DecodedImage, &'a Opacity)>,
        memo: &'a Mutex<Memo>,
    ) -> Option<Self> {
        let Samples::Bytes(samples) = &image.samples else {
            return None;
        };
        let memo = memo.lock().unwrap_or_else(PoisonError::into_inner);
        let matte = if memo.key.matte {
            let (mask, opacity) = mask?;
            let Samples::Bytes(codes) = &mask.samples else {
                return None;
            };
            Some((codes.as_slice(), opacity))
        } else {
            None
        };
        let embedded = match (&image.mask, memo.key.embedded) {
            (MaskKind::Embedded(alpha), true) => match &alpha.samples {
                Samples::Bytes(codes) => Some(codes.as_slice()),
                _ => return None,
            },
            _ => None,
        };
        Some(Self {
            samples,
            colors,
            matte,
            embedded,
            memo: RefCell::new(memo),
        })
    }

    /// The key of sample `index`.
    #[inline]
    fn key(&self, index: usize, components: usize) -> u64 {
        let start = index.saturating_mul(components);
        let codes = self.samples.get(start..start + components).unwrap_or(&[]);
        let mut key = codes.iter().fold(0u64, |key, &c| (key << 8) | u64::from(c));
        if let Some((mask, _)) = self.matte {
            key = (key << 8) | u64::from(mask.get(index).copied().unwrap_or(0));
        }
        if let Some(alpha) = self.embedded {
            key = (key << 8) | u64::from(alpha.get(index).copied().unwrap_or(0));
        }
        key
    }

    /// The texel of sample `index`, written into `out` (its held values).
    #[inline]
    fn write(&self, memo: &mut Memo, index: usize, out: &mut [f64]) {
        let components = memo.key.components;
        let key = self.key(index, components);
        let values = memo.get(key, |texel| self.convert(index, components, texel));
        for (out, &value) in out.iter_mut().zip(values) {
            *out = value;
        }
    }

    /// Converts sample `index` into a texel of every lane.
    fn convert(&self, index: usize, components: usize, texel: &mut [f64]) {
        let start = index.saturating_mul(components);
        let mut codes = [0u16; MAX_COMPONENTS];
        for (code, &byte) in codes
            .iter_mut()
            .zip(self.samples.get(start..start + components).unwrap_or(&[]))
        {
            *code = u16::from(byte);
        }
        let extra = Extra {
            embedded: self
                .embedded
                .map(|alpha| u16::from(alpha.get(index).copied().unwrap_or(0))),
            mask: self.matte.map(|(mask, opacity)| {
                opacity.code(u16::from(mask.get(index).copied().unwrap_or(0)))
            }),
        };
        self.colors
            .texel(&codes[..components.min(MAX_COMPONENTS)], extra, texel);
    }
}

impl Texels for MemoTexels<'_> {
    fn lanes(&self) -> usize {
        self.colors.lanes()
    }

    fn opaque(&self) -> bool {
        self.memo.borrow().opaque
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
        let start = sample_index(u64::from(row) * u64::from(width) + u64::from(first));
        let components = memo.key.components;
        let mut previous = None;
        let mut last = [0.0; MAX_LANES];
        for (index, values) in (start..start + count).zip(out.chunks_exact_mut(stride)) {
            let key = self.key(index, components);
            if previous != Some(key) {
                let held = memo.get(key, |texel| self.convert(index, components, texel));
                copy_texel(&mut last, held);
                previous = Some(key);
            }
            copy_texel(values, &last);
        }
    }
}

/// Copies the first values of `from` into `to`, as many as both hold: a
/// fixed-size move for the common counts, where a copy call per texel
/// costs more than the copy.
#[inline]
pub(super) fn copy_texel(to: &mut [f64], from: &[f64]) {
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
