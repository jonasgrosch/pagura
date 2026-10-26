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
//! The bounded baseline JPX container follows T.801 M.8/M.11 and Cor.4;
//! other Part 2 coding/layout extensions remain explicit boundaries. PDF
//! integration follows clause 7.4.9 and Table 87.
//! Unsupported JPX extensions and sample representations return
//! `Unsupported(filter.JPXDecode)`; structural truncations remain explicit errors.
//! ICC bytes, enumerated colour identifiers and CIELab encoding parameters
//! are preserved for the caller.
//!
//! `Limits::stream_bytes` caps encoded input, `pixels` caps the reference-grid
//! image, `decoded_bytes` conservatively caps cumulative allocations (including
//! scratch buffers), `objects` caps tiles, and `operations` caps parsing,
//! tag-tree and coefficient-pass work. All allocations are charged first.

mod boxes;
mod channels;
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

use pagura_core::{Error, Feature, Limits, Position, Result};

/// Stable tag for JPEG 2000 modes not yet implemented.
const UNSUPPORTED: Feature = Feature::new("filter.JPXDecode");

/// Colour information carried by a JP2 file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum JpxColor {
    /// No colour box (raw codestream).
    Unspecified,
    /// An enumerated colour space (`colr` method 1), e.g. 16 = sRGB, 17 =
    /// greyscale, 18 = sYCC.
    Enumerated(u32),
    /// CIELab (`colr` enumeration 14) with explicit encoding parameters.
    /// The default encoding remains [`Self::Enumerated`] with value 14.
    Lab(LabParameters),
    /// A restricted or full ICC profile (`colr` methods 2 and 3).
    Icc(Vec<u8>),
    /// A method whose colour interpretation is unavailable. Its payload
    /// remains available to callers (T.801 M.11.7.2).
    Other {
        /// The `colr` specification method.
        method: u8,
        /// Method-specific bytes after the three-byte header.
        data: Vec<u8>,
    },
    /// PDF colour choices in precedence/approximation order (7.4.9).
    /// A standalone JP2 decode retains the first specification instead.
    Alternatives(Vec<ColorSpecification>),
}

/// Explicit CIELab encoding fields (T.801 M.11.7.4.1, Table M.30).
/// These are source metadata; their colour interpretation is the caller's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LabParameters {
    ranges: [u32; 3],
    offsets: [u32; 3],
    illuminant: u32,
}

impl LabParameters {
    /// Preserves the three unsigned ranges, offsets and illuminant code.
    pub const fn new(ranges: [u32; 3], offsets: [u32; 3], illuminant: u32) -> Self {
        Self {
            ranges,
            offsets,
            illuminant,
        }
    }

    /// Ranges for L*, a* and b*, in that order.
    pub const fn ranges(&self) -> [u32; 3] {
        self.ranges
    }

    /// Offsets in the unsigned sample domain, for L*, a* and b*.
    pub const fn offsets(&self) -> [u32; 3] {
        self.offsets
    }

    /// The illuminant identifier from Table M.29.
    pub const fn illuminant(&self) -> u32 {
        self.illuminant
    }
}

/// One embedded colour specification and its selection metadata
/// (T.801 M.11.7.2). Equal priorities retain their file order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ColorSpecification {
    /// Signed precedence; higher values are preferred.
    pub precedence: i8,
    /// Approximation quality; lower values are preferred. JP2 uses zero.
    pub approximation: u8,
    /// Colour interpretation. Parser-produced alternatives never nest.
    pub color: JpxColor,
}

impl JpxColor {
    /// Whether this description contains CIELab, including ranked alternatives.
    /// Its encoded channels require a declared physical interpretation rather
    /// than an inferred device-colour fallback.
    pub fn has_lab(&self) -> bool {
        match self {
            Self::Enumerated(14) | Self::Lab(_) => true,
            Self::Alternatives(choices) => choices.iter().any(|choice| choice.color.has_lab()),
            _ => false,
        }
    }

    /// Heap storage retained by the colour metadata.
    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::Icc(data) | Self::Other { data, .. } => data.capacity(),
            Self::Alternatives(choices) => choices.iter().fold(
                choices
                    .capacity()
                    .saturating_mul(size_of::<ColorSpecification>()),
                |total, choice| total.saturating_add(choice.color.retained_bytes()),
            ),
            Self::Unspecified | Self::Enumerated(_) | Self::Lab(_) => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ColorSelection {
    First,
    Pdf,
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
        &mut pagura_core::Diagnostics::new(pagura_core::Recovery::Strict),
    )
}

/// Decodes JPEG 2000 with reported, bounded tile-part count recovery (T.800 A.4.2).
/// Lengths, ordered part numbers, packet syntax and EOC remain authoritative.
///
/// # Errors
/// As [`decode_with_options`]; strict diagnostics reject malformed counts.
pub fn decode_with_diagnostics(
    data: &[u8],
    limits: &Limits,
    options: &Options,
    diagnostics: &mut pagura_core::Diagnostics,
) -> Result<Decoded> {
    decode_selected(
        data,
        limits,
        options,
        diagnostics,
        false,
        ColorSelection::First,
    )
}

pub(crate) fn decode_pdf(
    data: &[u8],
    limits: &Limits,
    options: &Options,
    diagnostics: &mut pagura_core::Diagnostics,
    indexed: bool,
) -> Result<Decoded> {
    decode_selected(
        data,
        limits,
        options,
        diagnostics,
        indexed,
        ColorSelection::Pdf,
    )
}

fn decode_selected(
    data: &[u8],
    limits: &Limits,
    options: &Options,
    diagnostics: &mut pagura_core::Diagnostics,
    indexed: bool,
    selection: ColorSelection,
) -> Result<Decoded> {
    if data.len() > limits.stream_bytes {
        return Err(Error::limit("JPEG 2000 input budget exceeded").at(Position::Buffer(0)));
    }
    let mut budget = io::Budget::new(limits);
    budget.diagnostics = pagura_core::Diagnostics::new(diagnostics.mode());
    let result = decode_bounded(data, &mut budget, options, indexed, selection)
        .map_err(|error| error.at(Position::Buffer(0)));
    diagnostics.absorb(budget.diagnostics.take())?;
    result
}

/// The channels a decode of `data` with `options` would return, with
/// their precisions, placement, opacity and colour but no samples: the
/// container's header boxes and the codestream's main header are read and
/// validated as [`decode_pdf`] reads them, and nothing is reconstructed.
/// Repairs are checked against `mode` but not reported; the decode reports
/// them.
pub(crate) fn describe_pdf(
    data: &[u8],
    limits: &Limits,
    options: &Options,
    mode: pagura_core::Recovery,
    indexed: bool,
) -> Result<Decoded> {
    if data.len() > limits.stream_bytes {
        return Err(Error::limit("JPEG 2000 input budget exceeded").at(Position::Buffer(0)));
    }
    let mut budget = io::Budget::new(limits);
    budget.diagnostics = pagura_core::Diagnostics::new(mode);
    describe_bounded(data, &mut budget, options, indexed)
        .map_err(|error| error.at(Position::Buffer(0)))
}

fn describe_bounded(
    data: &[u8],
    budget: &mut io::Budget<'_>,
    options: &Options,
    indexed: bool,
) -> Result<Decoded> {
    if data.starts_with(&[255, 79]) {
        return codestream::describe(data, 0, budget);
    }
    let (stream, base, mut header) = boxes::parse(data, budget, options, ColorSelection::Pdf)?;
    if indexed {
        header.use_pdf_palette(budget)?;
    }
    let mut decoded = codestream::describe(stream, base, budget)?;
    header.apply(&mut decoded, budget)?;
    Ok(decoded)
}

fn decode_bounded(
    data: &[u8],
    budget: &mut io::Budget<'_>,
    options: &Options,
    indexed: bool,
    selection: ColorSelection,
) -> Result<Decoded> {
    if data.starts_with(&[255, 79]) {
        codestream::decode(data, 0, budget)
    } else {
        let (stream, base, mut header) = boxes::parse(data, budget, options, selection)?;
        if indexed {
            header.use_pdf_palette(budget)?;
        }
        let mut decoded = codestream::decode(stream, base, budget)?;
        header.apply(&mut decoded, budget)?;
        Ok(decoded)
    }
}

fn unsupported(message: &'static str, offset: usize) -> Error {
    Error::unsupported(UNSUPPORTED, message).at(Position::Buffer(offset))
}

#[cfg(test)]
mod tests;
