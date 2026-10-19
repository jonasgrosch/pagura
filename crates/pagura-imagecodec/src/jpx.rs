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
mod io;
mod mq;
mod packed;
mod packet;
mod progression;
mod segments;
mod termination;
mod tier1;
mod wavelet;

use pagura_codec::features::JPX as UNSUPPORTED;
use pagura_core::{Diagnostics, Error, Limits, Position, Recovery, Result};

use crate::budget::{Budget, Messages};

/// The `Limit` messages of a JPEG 2000 decode.
const MESSAGES: Messages = Messages {
    work: "JPEG 2000 work budget exceeded",
    memory: "JPEG 2000 working memory budget exceeded",
    output: "JPEG 2000 output budget exceeded",
};

/// Colour information carried by a JP2 file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpxColor {
    /// No colour box (raw codestream).
    Unspecified,
    /// An enumerated colour space (`colr` method 1), e.g. 16 = sRGB, 17 =
    /// greyscale, 18 = sYCC.
    Enumerated(u32),
    /// A restricted or full ICC profile (`colr` methods 2 and 3).
    Icc(Vec<u8>),
}

/// One decoded component.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// A decoded JPEG 2000 image.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Metadata interpretation requested by the caller (PDF 7.4.9).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Ignore `colr` specifications; return `JpxColor::Unspecified`.
    /// Palette expansion is retained because it creates image channels.
    pub ignore_color: bool,
    /// Ignore `cdef`; retain channel order and report no opacity.
    /// This also permits callers to disregard unsupported channel associations.
    pub ignore_opacity: bool,
    /// Do not apply a palette (`pclr`) and its component mapping: return the
    /// stored indices for a color space of the caller's that interprets them,
    /// such as a PDF `Indexed` space (7.4.9), which would otherwise expand
    /// them twice.
    pub ignore_palette: bool,
}

/// Meaning of the opacity samples, T.800 I.5.3.6, Table I.16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpacityKind {
    /// Colour samples have not been multiplied by opacity.
    Straight,
    /// Colour samples contain the stored premultiplied values (equation I-5).
    Premultiplied,
}

/// Placement of a component or palette-derived channel (T.800 B.2, G.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentGeometry {
    /// Horizontal and vertical reference-grid spacing, from SIZ.
    pub step: [u32; 2],
    /// Reference-grid position of the first decoded sample, equation B-1.
    pub origin: [u32; 2],
}

/// Origin and interpretation of a whole-image opacity channel (I.5.3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
pub struct Decoded {
    /// Existing image representation; samples retain their stored values.
    pub image: JpxImage,
    /// Image-area origin `(XOsiz, YOsiz)` on the reference grid.
    pub reference_origin: [u32; 2],
    /// Geometry in the same order as `image.components`, after mapping/reordering.
    pub component_geometry: Vec<ComponentGeometry>,
    /// Whole-image opacity, if interpreted from `cdef`.
    pub opacity: Option<Opacity>,
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
    decode_bounded(data, limits.pixels, &mut budget, options, diagnostics)
        .map_err(|error| error.at(Position::Buffer(0)))
}

fn decode_bounded(
    data: &[u8],
    pixels: usize,
    budget: &mut Budget,
    options: &Options,
    diagnostics: &mut Diagnostics,
) -> Result<Decoded> {
    if data.starts_with(&[255, 79]) {
        codestream::decode(data, 0, pixels, budget, diagnostics)
    } else {
        let (stream, base, mut header) = boxes::parse(data, pixels, budget, options, diagnostics)?;
        if options.ignore_palette {
            header.use_pdf_palette()?;
        }
        let mut decoded = codestream::decode(stream, base, pixels, budget, diagnostics)?;
        header.apply(&mut decoded, budget)?;
        Ok(decoded)
    }
}

fn unsupported(message: &'static str, offset: usize) -> Error {
    Error::unsupported(UNSUPPORTED, message).at(Position::Buffer(offset))
}

#[cfg(test)]
mod tests;
