//! What one source sample contributes (8.9.5.2, 8.9.6, 11.6.5.3): its
//! decoded components, an Indexed lookup, colour-key masking, embedded
//! opacity and Matte removal, converted to working colours and
//! premultiplied by its alpha, with alpha and shape.
//!
//! Tables and caches hold texels computed by the same per-sample function,
//! so they are exact by construction: a one-component image of at most
//! eight bits gets a table per code, a DeviceRGB image into an RGB blending
//! space one per channel (that conversion is the identity with clipping,
//! 10.4.2), and images of up to four components of at most eight bits a
//! bounded memo by sample tuple, with opacity codes for embedded alpha
//! and Matte removal.

use super::geometry::{MAX_LANES, Texels, sample_index, sample_row};
use super::scratch::allocate;
use crate::source::copy_values;
use pagura_color::{ColorSpace, Converter};
use pagura_core::{Meter, Result};
use pagura_image::{DecodedImage, ImageKind, MaskKind, Samples};
use std::cell::RefCell;

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

    /// Heap storage owned by this prepared colour description.
    pub(super) fn bytes(&self) -> usize {
        self.decode.capacity() * size_of::<[f64; 2]>()
            + self.maxima.capacity() * size_of::<f64>()
            + self
                .key
                .as_ref()
                .map_or(0, |key| key.capacity() * size_of::<[u16; 2]>())
            + self
                .matte
                .as_ref()
                .map_or(0, |matte| matte.capacity() * size_of::<f64>())
            + self.convert.owned_bytes()
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
    pub(super) fn new(mask: &DecodedImage, decode: [f64; 2], bytes: &mut Meter) -> Result<Self> {
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
            let mut table = super::reserved(1usize << bits, bytes)?;
            table.extend((0..1u16 << bits).map(|code| opacity.of(code)));
            opacity.table = Some(table);
        }
        Ok(opacity)
    }

    /// Heap bytes held by its opacity table.
    pub(super) fn bytes(&self) -> usize {
        self.table
            .as_ref()
            .map_or(0, |table| table.capacity() * size_of::<f64>())
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
    Memo(MemoLayout),
    /// Every texel computed from its sample.
    General,
}

impl Strategy {
    /// The strategy for `image`, reserving its tables and conversions
    /// before they are built.
    pub(super) fn new(
        image: &DecodedImage,
        colors: &Colors,
        rgb: bool,
        mask: Option<&DecodedImage>,
        bytes: &mut Meter,
        functions: &mut Meter,
    ) -> Result<Self> {
        let components = usize::from(image.components);
        let narrow = (0..components).all(|i| {
            image
                .component_bits(i)
                .is_some_and(|b| (1..=8).contains(&b))
        });
        let per_pixel = colors.matte.is_some() || colors.embedded.is_some();
        let lanes = colors.lanes();
        if components == 1
            && narrow
            && !per_pixel
            && matches!(image.samples, Samples::Bytes(_) | Samples::Packed(_))
        {
            let codes = 1u16 << image.component_bits(0).unwrap_or(8);
            let mut table = super::reserved(usize::from(codes) * lanes, bytes)?;
            table.resize(usize::from(codes) * lanes, 0.0);
            functions.charge_product(usize::from(codes), colors.convert.cost())?;
            for (code, texel) in (0..codes).zip(table.chunks_exact_mut(lanes)) {
                colors.code_texel(code, texel);
            }
            let opaque = opaque_table(&table, lanes);
            return Ok(Self::Table(table, opaque));
        }
        if rgb
            && components == 3
            && (0..3).all(|i| image.component_bits(i) == Some(8))
            && !per_pixel
            && colors.key.is_none()
            && colors.lookup.is_none()
            && lanes == 5
            && matches!(image.samples, Samples::Bytes(_) | Samples::Planar(_))
        {
            bytes.charge_items::<[[f64; 256]; 3]>(1)?;
            let mut tables = Box::new([[0.0; 256]; 3]);
            let mut texel = [0.0; 5];
            functions.charge_product(256, colors.convert.cost())?;
            for code in 0..=255u8 {
                let code = u16::from(code);
                colors.texel(&[code, code, code], Extra::default(), &mut texel);
                for (table, &value) in tables.iter_mut().zip(&texel[..3]) {
                    table[usize::from(code)] = value;
                }
            }
            return Ok(Self::Rgb(tables));
        }
        if let Some(key) = Key::new(image, colors, mask) {
            let samples = u64::from(image.width) * u64::from(image.height);
            let entries = usize::try_from(samples).unwrap_or(usize::MAX);
            let opaque = colors.key.is_none() && colors.embedded.is_none();
            let held = if opaque { lanes - 2 } else { lanes };
            let replacing = colors.convert.arithmetic();
            return Ok(Self::Memo(MemoLayout::new(
                key,
                (held, opaque),
                entries,
                replacing,
            )));
        }
        Ok(Self::General)
    }

    /// Bytes the strategy's tables hold.
    pub(super) fn bytes(&self) -> usize {
        match self {
            Self::Table(table, _) => table.capacity() * size_of::<f64>(),
            Self::Rgb(_) => size_of::<[[f64; 256]; 3]>(),
            Self::Memo(_) | Self::General => 0,
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
        self.table
            .get(start..start + self.lanes)
            .unwrap_or(&[0.0; MAX_LANES])
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
        copy_values(out, self.texel(index));
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
    pub(super) values: RefCell<&'a mut Vec<f64>>,
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
        let mut storage = self.values.borrow_mut();
        let values = super::scratch::grown(&mut storage, count * (stride - 1));
        self.texels.row(row, cells, values);
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
                    copy_values(rest, value);
                }
            }
            start += take;
        }
    }
}

/// The key of a sample of at most eight bits per code: its codes, then the
/// soft mask's opacity code where a Matte is removed, then the embedded
/// opacity code, eight bits each and at most six of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Key {
    components: usize,
    matte: bool,
    embedded: bool,
}

impl Key {
    /// The key of `image`'s samples painted with `colors`, when every code
    /// it holds has at most eight bits and they fit 64 bits.
    fn new(image: &DecodedImage, colors: &Colors, mask: Option<&DecodedImage>) -> Option<Self> {
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
        if matte && !mask.is_some_and(|mask| matches!(mask.samples, Samples::Bytes(_))) {
            return None;
        }
        let bytes = components + usize::from(matte) + usize::from(embedded);
        (narrow && bytes <= 6 && matches!(image.samples, Samples::Bytes(_))).then_some(Self {
            components,
            matte,
            embedded,
        })
    }
}

/// The exact inputs of a sample key, shared by capacity planning and use.
struct SampleKeys<'a> {
    components: usize,
    count: usize,
    colors: &'a [u8],
    matte: Option<&'a [u8]>,
    embedded: Option<&'a [u8]>,
}

impl<'a> SampleKeys<'a> {
    fn new(key: Key, image: &'a DecodedImage, mask: Option<&'a DecodedImage>) -> Option<Self> {
        let Samples::Bytes(colors) = &image.samples else {
            return None;
        };
        let matte = if key.matte {
            let Samples::Bytes(codes) = &mask?.samples else {
                return None;
            };
            Some(codes.as_slice())
        } else {
            None
        };
        let embedded = match (&image.mask, key.embedded) {
            (MaskKind::Embedded(alpha), true) => match &alpha.samples {
                Samples::Bytes(codes) => Some(codes.as_slice()),
                _ => return None,
            },
            _ => None,
        };
        Some(Self {
            components: key.components,
            count: sample_index(u64::from(image.width) * u64::from(image.height)),
            colors,
            matte,
            embedded,
        })
    }

    /// The colour and opacity codes of sample `index`.
    #[inline]
    fn get(&self, index: usize) -> u64 {
        let start = index.saturating_mul(self.components);
        let codes = self
            .colors
            .get(start..start + self.components)
            .unwrap_or(&[]);
        let mut key = codes.iter().fold(0u64, |key, &c| (key << 8) | u64::from(c));
        for codes in [self.matte, self.embedded].into_iter().flatten() {
            key = (key << 8) | u64::from(codes.get(index).copied().unwrap_or(0));
        }
        key
    }

    /// Counts at most 2048 distinct keys. A photographic source ends the
    /// scan as soon as it exceeds that count and keeps its original bound.
    fn distinct(&self, work: &mut Meter, bytes: &mut Meter) -> Result<Option<usize>> {
        const COUNT: usize = 2048;
        const EMPTY: u64 = u64::MAX;
        let mut slots = Vec::new();
        bytes.reserve(&mut slots, COUNT * 2)?;
        let allocated = slots.capacity() * size_of::<u64>();
        let result = (|| {
            work.charge(COUNT * 2)?;
            slots.resize(COUNT * 2, EMPTY);
            let mut distinct = 0;
            let mut previous = None;
            for index in 0..self.count {
                work.charge(1)?;
                let key = self.get(index);
                if previous == Some(key) {
                    continue;
                }
                previous = Some(key);
                let mut slot = hash_slot(key, slots.len() - 1);
                loop {
                    work.charge(1)?;
                    let held = slots[slot];
                    if held == key {
                        break;
                    }
                    if held == EMPTY {
                        if distinct == COUNT {
                            return Ok(None);
                        }
                        slots[slot] = key;
                        distinct += 1;
                        break;
                    }
                    slot = (slot + 1) & (slots.len() - 1);
                }
            }
            Ok(Some(distinct))
        })();
        drop(slots);
        bytes.refund(allocated);
        result
    }
}

/// The memo's first probe, shared with capacity discovery. The mask is
/// applied before narrowing: on 32-bit targets the shifted hash can exceed
/// `usize`, and every such key would otherwise probe from slot 0.
#[inline]
fn hash_slot(key: u64, mask: usize) -> usize {
    let hash = key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 29;
    let mask = u64::try_from(mask).unwrap_or(u64::MAX);
    usize::try_from(hash & mask).unwrap_or(0)
}

/// Texels by sample key: open addressing into slots that hold a key and
/// its `held` values together, so that a lookup reads one place. Keys have
/// at most 48 bits, so each is held exactly as an f64. The memo holds at
/// most one texel per 16 samples of its image (at least 4096, at most
/// [`MemoLayout::CAP`]), so that what it reserves stays near the decoded samples'
/// own size; a sample past them is converted without being kept.
///
/// An arithmetic conversion costs less than a lookup that misses the
/// cache, as one into a large table of a photograph does. Its memo
/// replaces instead: at most [`MemoLayout::REPLACING`] slots, each holding
/// the texel of the last key that reached it, so the table stays in a
/// core's cache and keeps the colours that repeat nearby. Every texel held
/// is its key's, so what the memo holds never changes a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MemoLayout {
    key: Key,
    opaque: bool,
    held: usize,
    limit: usize,
    /// Whether a key takes its first slot from the one held there, rather
    /// than a free slot while fewer than `limit` are held.
    replacing: bool,
}

/// Mutable memo values, allocated in the caller's render scratch.
#[derive(Debug)]
pub(super) struct Memo {
    layout: MemoLayout,
    slots: Vec<f64>,
    len: usize,
    /// The texel converted last, of every lane.
    spare: [f64; MAX_LANES],
}

/// A free slot: no key is negative.
const FREE: f64 = -1.0;

impl MemoLayout {
    /// The most texels kept.
    pub(super) const CAP: usize = 1 << 20;

    /// The most slots of a replacing memo: about half a megabyte of RGB or
    /// CMYK texels.
    pub(super) const REPLACING: usize = 1 << 14;

    /// The memo of an image of `samples` samples, replacing for an
    /// arithmetic conversion.
    fn new(key: Key, (held, opaque): (usize, bool), samples: usize, replacing: bool) -> Self {
        Self {
            key,
            opaque,
            held,
            limit: (samples / 16).clamp(4096, Self::CAP),
            replacing,
        }
    }

    /// Whether every texel has alpha and shape 1, which the memo then does
    /// not hold.
    pub(super) fn opaque(&self) -> bool {
        self.opaque
    }

    /// Bytes of the fixed table, whose slots stay at most half full.
    pub(super) fn bytes(self) -> usize {
        size_of::<Memo>().saturating_add(self.values().saturating_mul(size_of::<f64>()))
    }

    fn values(self) -> usize {
        let slots = (2 * self.limit).next_power_of_two();
        let slots = if self.replacing {
            slots.min(Self::REPLACING)
        } else {
            slots
        };
        slots.saturating_mul(1 + self.held)
    }

    /// Tightens only a source proven to have few distinct sample keys.
    pub(super) fn fit(
        &mut self,
        image: &DecodedImage,
        mask: Option<&DecodedImage>,
        work: &mut Meter,
        bytes: &mut Meter,
    ) -> Result<()> {
        if let Some(samples) = SampleKeys::new(self.key, image, mask) {
            // The pixel count already bounds small sources; discovering
            // their keys would spend more temporary storage than it saves.
            if samples.count <= 2048 {
                self.limit = self.limit.min(samples.count.max(1));
            } else if let Some(count) = samples.distinct(work, bytes)? {
                self.limit = self.limit.min(count.max(1));
            }
        }
        Ok(())
    }

    /// Allocates the full fixed table after its bytes have been charged.
    pub(super) fn make(self) -> Result<Memo> {
        let mut slots = allocate(self.values())?;
        slots.resize(self.values(), FREE);
        Ok(Memo {
            layout: self,
            slots,
            len: 0,
            spare: [0.0; MAX_LANES],
        })
    }
}

impl Memo {
    /// Actual retained capacities.
    pub(super) fn bytes(&self) -> usize {
        size_of::<Self>() + self.slots.capacity() * size_of::<f64>()
    }

    /// Invalidates conversions between renders without reallocating.
    pub(super) fn reset(&mut self) {
        self.slots.fill(FREE);
        self.len = 0;
    }

    /// The `held` values of the texel of `key`, converted by `convert` into
    /// a texel of every lane unless held.
    #[inline]
    fn get(&mut self, key: u64, convert: impl FnOnce(&mut [f64])) -> &[f64] {
        let held = self.layout.held;
        let stride = 1 + held;
        #[expect(
            clippy::cast_precision_loss,
            reason = "keys have at most 48 bits, which f64 holds exactly"
        )]
        let wanted = key as f64;
        let slot = if self.layout.replacing {
            hash_slot(key, self.slots.len() / stride - 1)
        } else {
            self.slot(key, wanted)
        };
        let at = slot * stride;
        if self.slots.get(at).copied() != Some(wanted) {
            convert(&mut self.spare[..held + usize::from(self.layout.opaque) * 2]);
            if !self.layout.replacing {
                if self.len >= self.layout.limit {
                    return &self.spare[..held];
                }
                self.len += 1;
            }
            if let Some(entry) = self.slots.get_mut(at..at + stride) {
                entry[0] = wanted;
                copy_values(&mut entry[1..], &self.spare[..held]);
            }
        }
        self.slots.get(at + 1..at + stride).unwrap_or(&[])
    }

    /// The slot of `key` (held as `wanted`), or the free slot it goes into.
    #[inline]
    fn slot(&self, key: u64, wanted: f64) -> usize {
        let stride = 1 + self.layout.held;
        let mask = self.slots.len() / stride - 1;
        let mut slot = hash_slot(key, mask);
        while self
            .slots
            .get(slot * stride)
            .is_some_and(|&k| k != wanted && k != FREE)
        {
            slot = (slot + 1) & mask;
        }
        slot
    }
}

/// Texels of eight-bit samples through their memo.
pub(super) struct MemoTexels<'a> {
    samples: SampleKeys<'a>,
    colors: &'a Colors,
    /// The soft mask's codes on the image's grid and their opacities, where
    /// a Matte is removed.
    matte: Option<&'a Opacity>,
    memo: RefCell<&'a mut Memo>,
}

impl<'a> MemoTexels<'a> {
    /// The texels of `image` through `memo`, with the soft mask `mask` for
    /// Matte removal; `None` where the samples are not bytes.
    pub(super) fn new(
        image: &'a DecodedImage,
        colors: &'a Colors,
        mask: Option<(&'a DecodedImage, &'a Opacity)>,
        memo: &'a mut Memo,
    ) -> Option<Self> {
        let samples = SampleKeys::new(memo.layout.key, image, mask.map(|mask| mask.0))?;
        let matte = mask.map(|mask| mask.1);
        Some(Self {
            samples,
            colors,
            matte,
            memo: RefCell::new(memo),
        })
    }

    /// The texel of sample `index`, written into `out` (its held values).
    #[inline]
    fn write(&self, memo: &mut Memo, index: usize, out: &mut [f64]) {
        let components = memo.layout.key.components;
        let key = self.samples.get(index);
        let values = memo.get(key, |texel| self.convert(index, components, texel));
        for (out, &value) in out.iter_mut().zip(values) {
            *out = value;
        }
    }

    /// Converts sample `index` into a texel of every lane.
    fn convert(&self, index: usize, components: usize, texel: &mut [f64]) {
        let start = index.saturating_mul(components);
        let mut codes = [0u16; MAX_COMPONENTS];
        for (code, &byte) in codes.iter_mut().zip(
            self.samples
                .colors
                .get(start..start + components)
                .unwrap_or(&[]),
        ) {
            *code = u16::from(byte);
        }
        let extra = Extra {
            embedded: self
                .samples
                .embedded
                .map(|alpha| u16::from(alpha.get(index).copied().unwrap_or(0))),
            mask: self.samples.matte.zip(self.matte).map(|(mask, opacity)| {
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
        self.memo.borrow().layout.opaque
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
        let components = memo.layout.key.components;
        let mut previous = None;
        let mut last = [0.0; MAX_LANES];
        for (index, values) in (start..start + count).zip(out.chunks_exact_mut(stride)) {
            let key = self.samples.get(index);
            if previous != Some(key) {
                let held = memo.get(key, |texel| self.convert(index, components, texel));
                copy_values(&mut last, held);
                previous = Some(key);
            }
            copy_values(values, &last);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, MemoLayout, SampleKeys};
    use pagura_core::Meter;

    #[test]
    fn small_sources_need_no_capacity_discovery_allocation() {
        let image = crate::source::image::tests::image(
            (1, 1),
            3,
            8,
            pagura_image::Samples::Bytes(vec![1, 2, 3]),
        );
        let mut layout = MemoLayout::new(
            Key {
                components: 3,
                matte: false,
                embedded: false,
            },
            (3, true),
            1,
            false,
        );
        layout
            .fit(
                &image,
                None,
                &mut Meter::new(0, "test"),
                &mut Meter::new(0, "test"),
            )
            .unwrap();
        assert_eq!(layout.limit, 1);
    }

    #[test]
    fn capacity_discovery_includes_both_opacities_and_refunds_temporary_bytes() {
        let colors = vec![17; 6000 * 3];
        let matte: Vec<_> = (0..6000).map(|n| u8::from(n % 2 != 0)).collect();
        let embedded: Vec<_> = (0..6000).map(|n| [0, 1, 2][n % 3]).collect();
        let keys = SampleKeys {
            components: 3,
            count: 6000,
            colors: &colors,
            matte: Some(&matte),
            embedded: Some(&embedded),
        };
        let mut work = Meter::new(100_000, "test");
        let mut bytes = Meter::new(32_768 + 7, "test");
        bytes.charge(7).unwrap();
        assert_eq!(keys.distinct(&mut work, &mut bytes).unwrap(), Some(6));
        assert!(work.used() >= keys.count);
        assert_eq!(bytes.used(), 7);

        let mut tiny = Meter::new(0, "test");
        assert!(keys.distinct(&mut tiny, &mut bytes).is_err());
        assert_eq!(tiny.used(), 0);
        assert_eq!(bytes.used(), 7);
        let mut tiny = Meter::new(32_767, "test");
        let before = work.used();
        assert!(keys.distinct(&mut work, &mut tiny).is_err());
        assert_eq!(work.used(), before);
    }

    #[test]
    fn capacity_discovery_keeps_the_photographic_bound() {
        let colors: Vec<_> = (0..3000u16)
            .flat_map(|n| [n.to_be_bytes()[0], n.to_be_bytes()[1], 17])
            .collect();
        let image = crate::source::image::tests::image(
            (3000, 1),
            3,
            8,
            pagura_image::Samples::Bytes(colors),
        );
        let key = Key {
            components: 3,
            matte: false,
            embedded: false,
        };
        let mut layout = MemoLayout::new(key, (3, true), 1_000_000, false);
        let before = layout;
        let mut work = Meter::new(100_000, "test");
        let mut bytes = Meter::new(32_768, "test");
        layout.fit(&image, None, &mut work, &mut bytes).unwrap();
        assert_eq!(layout, before);
        assert_eq!(bytes.used(), 0);
        assert!(work.used() < 4096 + 3 * 3000);
    }

    /// The memo reserves less than eight bytes a sample of its image, and
    /// past its limit it converts every sample anew, alike.
    #[test]
    fn the_memo_stays_near_its_samples_and_converts_past_its_limit() {
        let key = Key {
            components: 3,
            matte: false,
            embedded: false,
        };
        assert!(MemoLayout::new(key, (3, true), 1_000_000, false).bytes() <= 8_000_000);
        assert!(MemoLayout::new(key, (4, true), 64, false).bytes() <= (8 << 12) * 5 * 8);
        let mut memo = MemoLayout::new(key, (3, true), 0, false).make().unwrap();
        let texel = |key: u64| {
            #[expect(clippy::cast_precision_loss, reason = "small test keys")]
            let value = key as f64;
            [value, value + 0.5, value * 2.0]
        };
        for round in 0..2 {
            for k in 0..10_000u64 {
                let held = memo.get(k, |out| out[..3].copy_from_slice(&texel(k)));
                assert_eq!(held, texel(k), "key {k} round {round}");
            }
        }
        assert_eq!(memo.len, memo.layout.limit);
    }

    /// A replacing memo stays within its slots however large its image,
    /// holds the texel of the last key in each slot, and converts a key
    /// again once another has taken its slot.
    #[test]
    fn a_replacing_memo_stays_small_and_holds_the_last_key_of_each_slot() {
        let key = Key {
            components: 3,
            matte: false,
            embedded: false,
        };
        let layout = MemoLayout::new(key, (3, true), 1 << 30, true);
        assert_eq!(layout.values(), MemoLayout::REPLACING * 4);
        let mut memo = layout.make().unwrap();
        let texel = |key: u64| {
            #[expect(clippy::cast_precision_loss, reason = "small test keys")]
            let value = key as f64;
            [value, value + 0.5, value * 2.0]
        };
        let keys = 0..3 * u64::try_from(MemoLayout::REPLACING).unwrap();
        let mut converted = 0;
        for round in 0..2 {
            for k in keys.clone() {
                for _ in 0..2 {
                    let held = memo.get(k, |out| {
                        converted += 1;
                        out[..3].copy_from_slice(&texel(k));
                    });
                    assert_eq!(held, texel(k), "key {k} round {round}");
                }
            }
        }
        let count = usize::try_from(keys.end).unwrap();
        assert!(converted > count && converted < 2 * count, "{converted}");
    }
}
