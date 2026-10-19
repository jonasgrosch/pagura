//! JPEG 2000 decoding (ISO/IEC 15444-1) for `JPXDecode` (clause 7.4.9):
//! JP2 files and raw codestreams.
//!
//! Supports 1–16-bit components, multiple tiles and packet-boundary tile-parts,
//! default and explicit precincts, all five progression orders and POC changes,
//! PPM/PPT packed headers, partial code-block bit planes across quality layers,
//! ROI maxshift, MQ and raw bypass coding, context reset, pass termination,
//! predictable termination, vertical-causal contexts and segmentation symbols,
//! 5/3 and 9/7 synthesis, RCT/ICT, palettes and straight or premultiplied opacity.
//! Part 1 mechanics follow ITU-T T.800 (06/2019), Annexes A–I.
//! Part 2 extensions remain pending primary T.801 review. PDF integration
//! follows clause 7.4.9 and Table 87.
//! Unsupported JPX extensions and sample representations return
//! `Unsupported(filter.JPXDecode)`; structural truncations remain explicit errors.
//! ICC bytes and enumerated colour identifiers are preserved for the caller.
//!
//! Budgets (ADR 0016): `Limits::pixels` caps the reference-grid image,
//! `decode_work` caps parsing, tag-tree, coefficient-pass, synthesis and
//! transform work, `decode_bytes` caps working allocations (coefficient
//! planes, code-block data, tag trees and scratch buffers) and
//! `decoded_bytes` caps the returned samples and metadata. Every allocation
//! is charged before it is made. The tile count is bounded by T.800 A.4.2.

mod boxes;
mod codestream;
mod coding;
mod marker;
mod mq;
mod packed;
mod packet;
mod progression;
mod segments;
mod termination;
mod tier1;
mod wavelet;

use pagura_codec::features::JPX as UNSUPPORTED;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostics, Error, Limits, Position, Recovery, Result};
use std::num::NonZeroU16;

use crate::budget::{Budget, Messages};

/// The `Limit` messages of a JPEG 2000 decode.
const MESSAGES: Messages = Messages {
    work: "JPEG 2000 work budget exceeded",
    memory: "JPEG 2000 working memory budget exceeded",
    output: "JPEG 2000 output budget exceeded",
};

/// Colour information carried by a JP2 file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum JpxColor {
    /// No colour box (raw codestream).
    Unspecified,
    /// An enumerated colour space (`colr` method 1).
    Enumerated(EnumeratedColorSpace),
    /// A restricted or full ICC profile (`colr` methods 2 and 3).
    Icc(Vec<u8>),
}

/// An enumerated color space, the `EnumCS` field of a JP2 color
/// specification box (T.800 I.5.3.3). Values without a constant here are
/// kept as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumeratedColorSpace(u32);

impl EnumeratedColorSpace {
    /// CMYK, a T.801 enumeration that ISO 32000-2 7.4.9 admits.
    pub const CMYK: Self = Self(12);
    /// sRGB (T.800 I.5.3.3).
    pub const SRGB: Self = Self(16);
    /// Grayscale (T.800 I.5.3.3).
    pub const GRAYSCALE: Self = Self(17);
    /// sYCC (T.800 I.5.3.3).
    pub const SYCC: Self = Self(18);
    /// CIEJab, a T.801 enumeration that ISO 32000-2 7.4.9 excludes.
    pub const CIEJAB: Self = Self(19);

    /// The color space with `EnumCS` value `value`.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The `EnumCS` value.
    pub const fn value(self) -> u32 {
        self.0
    }
}

/// One decoded component.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JpxComponent {
    /// Bit depth of the samples.
    pub bits: u8,
    /// Whether samples are signed.
    pub signed: bool,
    /// Width in samples (components may be subsampled).
    pub width: u32,
    /// Height in samples.
    pub height: u32,
    /// Samples, row-major, offset to unsigned when `signed`.
    pub samples: Vec<u16>,
}

impl JpxComponent {
    /// An unsigned component of `width × height` samples of `bits` bits.
    pub const fn new(bits: u8, width: u32, height: u32, samples: Vec<u16>) -> Self {
        Self {
            bits,
            signed: false,
            width,
            height,
            samples,
        }
    }
}

/// A decoded JPEG 2000 image.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JpxImage {
    /// Image width on the reference grid.
    pub width: u32,
    /// Image height on the reference grid.
    pub height: u32,
    /// Components after any palette and channel definitions.
    pub components: Vec<JpxComponent>,
    /// Colour information.
    pub color: JpxColor,
    /// Index of an opacity component (`cdef`), if any.
    pub alpha: Option<usize>,
}

impl JpxImage {
    /// An image without opacity.
    pub const fn new(
        width: u32,
        height: u32,
        components: Vec<JpxComponent>,
        color: JpxColor,
    ) -> Self {
        Self {
            width,
            height,
            components,
            color,
            alpha: None,
        }
    }
}

/// Metadata interpretation requested by the caller (PDF 7.4.9).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// Ignore `colr` specifications; return `JpxColor::Unspecified`.
    /// Palette expansion is retained because it creates image channels.
    pub ignore_color: bool,
    /// Do not interpret `cdef`: keep the channel order and report no
    /// opacity. The definitions are still returned in
    /// [`Decoded::channel_definitions`], so callers can apply a policy of
    /// their own, including to associations this decoder does not support.
    pub ignore_opacity: bool,
    /// Do not apply a palette (`pclr`) and its component mapping: return the
    /// stored indices for a color space of the caller's that interprets them,
    /// such as a PDF `Indexed` space (7.4.9). The channel definitions of a
    /// palette describe its output and are dropped with it.
    pub ignore_palette: bool,
}

/// One entry of a JP2 channel definition box (T.800 I.5.3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChannelDefinition {
    /// `Cn`: the channel, an index into the channels after palette mapping.
    pub channel: u16,
    /// `Typ`: what the channel's samples are.
    pub kind: ChannelType,
    /// `Asoc`: what the channel belongs to.
    pub association: Association,
}

/// The `Typ` of a channel definition (T.800 I.5.3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChannelType {
    /// Color samples (0).
    Color,
    /// Opacity (1).
    Opacity,
    /// Premultiplied opacity (2).
    PremultipliedOpacity,
    /// Not specified (65535).
    Unspecified,
    /// A reserved value.
    Reserved(u16),
}

impl From<u16> for ChannelType {
    fn from(value: u16) -> Self {
        match value {
            0 => Self::Color,
            1 => Self::Opacity,
            2 => Self::PremultipliedOpacity,
            u16::MAX => Self::Unspecified,
            value => Self::Reserved(value),
        }
    }
}

/// The `Asoc` of a channel definition (T.800 I.5.3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Association {
    /// The whole image (0).
    Image,
    /// The color of the color space with this number, counted from one
    /// (1 to 65534).
    Color(NonZeroU16),
    /// No color (65535).
    None,
}

impl From<u16> for Association {
    fn from(value: u16) -> Self {
        match NonZeroU16::new(value) {
            None => Self::Image,
            Some(_) if value == u16::MAX => Self::None,
            Some(color) => Self::Color(color),
        }
    }
}

/// Meaning of the opacity samples, T.800 I.5.3.6, Table I.16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpacityKind {
    /// Colour samples have not been multiplied by opacity.
    Straight,
    /// Colour samples contain the stored premultiplied values (T.800 equation I-5).
    Premultiplied,
}

/// Placement of a component or palette-derived channel (T.800 B.2, G.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComponentGeometry {
    /// Horizontal and vertical reference-grid spacing, from SIZ.
    pub step: [u32; 2],
    /// Reference-grid position of the first decoded sample, T.800 equation B-1.
    pub origin: [u32; 2],
}

impl ComponentGeometry {
    /// The placement of samples `step` apart from `origin`.
    pub const fn new(step: [u32; 2], origin: [u32; 2]) -> Self {
        Self { step, origin }
    }
}

/// Origin and interpretation of a whole-image opacity channel (T.800 I.5.3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Opacity {
    /// Straight or premultiplied opacity.
    pub kind: OpacityKind,
    /// Channel index in the returned image (also `JpxImage::alpha`).
    pub channel: usize,
    /// Channel index named by `cdef`, before colour-channel reordering.
    pub source_channel: usize,
    /// Original codestream component, before palette mapping.
    pub source_component: usize,
    /// Palette column, when the opacity was generated by `pclr`/`cmap`.
    pub palette_column: Option<u8>,
}

/// Decoded samples and their original reference-grid and channel metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Decoded {
    /// Existing image representation; samples retain their stored values.
    pub image: JpxImage,
    /// Image-area origin `(XOsiz, YOsiz)` on the reference grid.
    pub reference_origin: [u32; 2],
    /// Geometry in the same order as `image.components`, after mapping/reordering.
    pub component_geometry: Vec<ComponentGeometry>,
    /// Whole-image opacity, if interpreted from `cdef`.
    pub opacity: Option<Opacity>,
    /// The `cdef` entries as read, empty without a channel definition box.
    /// Their channel indices name the channels before any reordering.
    pub channel_definitions: Vec<ChannelDefinition>,
}

impl Decoded {
    /// An image whose components have the given placement, without opacity
    /// or channel definitions.
    pub const fn new(
        image: JpxImage,
        reference_origin: [u32; 2],
        component_geometry: Vec<ComponentGeometry>,
    ) -> Self {
        Self {
            image,
            reference_origin,
            component_geometry,
            opacity: None,
            channel_definitions: Vec::new(),
        }
    }

    /// Keeps the channels `order` lists, in that order: channel `i` becomes
    /// the former channel `order[i]`, and channels not listed are dropped.
    /// The samples are moved, not copied. `image.alpha` and `opacity` follow
    /// their channel, or are cleared when it is dropped. One slot per channel
    /// is allocated to track the moves.
    ///
    /// # Errors
    ///
    /// An `InvalidArgument` error if `order` lists a channel twice or names
    /// one that does not exist, or if the channel geometry does not match
    /// the channels.
    pub fn select_channels(&mut self, order: &[usize]) -> Result<()> {
        let components = &mut self.image.components;
        let count = components.len();
        if self.component_geometry.len() != count || order.len() > count {
            return Err(Error::invalid_argument(
                "JPEG 2000 channel selection does not match the channels",
            ));
        }
        // `at` maps each place to the former channel now there and `place`
        // each former channel to where it is now: inverse permutations of
        // 0..count, so indexing either with the other's values is in range.
        let mut at: Vec<usize> = (0..count).collect();
        let mut place = at.clone();
        for (target, &source) in order.iter().enumerate() {
            let from = place.get(source).copied().ok_or(Error::invalid_argument(
                "JPEG 2000 channel selection names a missing channel",
            ))?;
            // The channels placed so far are order[..target].
            if from < target {
                return Err(Error::invalid_argument(
                    "JPEG 2000 channel selection names a channel twice",
                ));
            }
            components.swap(target, from);
            self.component_geometry.swap(target, from);
            at.swap(target, from);
            place[at[target]] = target;
            place[at[from]] = from;
        }
        components.truncate(order.len());
        self.component_geometry.truncate(order.len());
        let new_index = |channel: usize| order.iter().position(|&source| source == channel);
        self.image.alpha = self.image.alpha.and_then(new_index);
        self.opacity = self.opacity.and_then(|opacity| {
            new_index(opacity.channel).map(|channel| Opacity { channel, ..opacity })
        });
        Ok(())
    }
}

/// Decodes a JP2 file or raw JPEG 2000 codestream with default metadata handling.
///
/// # Errors
/// Returns syntax errors for corrupt data, `Unsupported` for features not
/// implemented, and limit errors beyond `limits`.
pub fn decode(data: &[u8], limits: &Limits) -> Result<JpxImage> {
    Ok(decode_with_options(data, limits, &Options::default())?.image)
}

/// Decodes samples and placement/opacity metadata (T.800 B.2, G.4, I.5.3.6).
/// Premultiplied colour samples are returned unchanged; this function never
/// unpremultiplies. Colour and channel metadata may be ignored independently
/// when interpreting the PDF dictionary (ISO 32000-2, 7.4.9).
///
/// # Errors
/// Returns syntax errors for corrupt data, `Unsupported` for features not
/// implemented, and limit errors beyond `limits`. Per-colour or multiple
/// opacity channels require a richer output model and return `Unsupported`.
pub fn decode_with_options(data: &[u8], limits: &Limits, options: &Options) -> Result<Decoded> {
    decode_with_diagnostics(
        data,
        limits,
        options,
        &mut Diagnostics::new(Recovery::Strict),
    )
}

/// Decodes as [`decode_with_options`], with reported repairs: tile-parts
/// beyond their declared count, which their order and lengths still delimit
/// ([`crate::repairs::JPX_TILE_PART_COUNT`], T.800 A.4.2), and a component
/// mapping box outside the JP2 header that supplies the mapping its palette
/// lacks ([`crate::repairs::JPX_MAPPING_LOCATION`]).
///
/// # Errors
/// As [`decode_with_options`], including stable repair tags when strict
/// `diagnostics` refuse a repair.
pub fn decode_with_diagnostics(
    data: &[u8],
    limits: &Limits,
    options: &Options,
    diagnostics: &mut Diagnostics,
) -> Result<Decoded> {
    let mut budget = Budget::new(limits, MESSAGES);
    let mut input = cursor(data);
    if is_codestream(data) {
        codestream::decode(input, limits.pixels, &mut budget, diagnostics)
    } else {
        let (stream, mut header) =
            boxes::parse(&mut input, limits.pixels, &mut budget, options, diagnostics)?;
        if options.ignore_palette {
            header.ignore_palette()?;
        }
        let mut decoded = codestream::decode(stream, limits.pixels, &mut budget, diagnostics)?;
        header.apply(&mut decoded, &mut budget)?;
        Ok(decoded)
    }
}

/// Whether `data` is a raw codestream, which starts with the SOC marker
/// (T.800 A.4.1), rather than a JP2 file, which starts with a signature box
/// (T.800 I.5.1).
pub fn is_codestream(data: &[u8]) -> bool {
    data.starts_with(&marker::Marker::SOC.bytes())
}

/// A cursor over JPEG 2000 input. Its positions are offsets in `data`.
fn cursor(data: &[u8]) -> Cursor<'_> {
    Cursor::new(data).with_message("truncated JPEG 2000 data")
}

/// Rejects bytes left over at the end of a marker segment or box.
fn finish(fields: &Cursor<'_>) -> Result<()> {
    if fields.at_end() {
        Ok(())
    } else {
        Err(fields.error("extra JPEG 2000 field data"))
    }
}

fn syntax(message: &'static str, position: Position) -> Error {
    Error::syntax(message).at(position)
}

fn unsupported(message: &'static str, position: Position) -> Error {
    Error::unsupported(UNSUPPORTED, message).at(position)
}

/// The number of samples in a `width × height` grid.
fn area(width: u32, height: u32) -> Result<usize> {
    u32_to_usize(width)
        .checked_mul(u32_to_usize(height))
        .ok_or(Error::limit("JPEG 2000 sample count overflow"))
}

#[cfg(test)]
mod tests;
