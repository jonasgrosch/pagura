//! JP2 fields, T.800 I.4–I.5 and PDF 7.4.9; Part 2 extensions need T.801.

use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result, ResultExt};

use crate::budget::Budget;

use super::{
    Association, ChannelDefinition, ChannelType, Decoded, EnumeratedColorSpace, JpxColor,
    JpxComponent, JpxImage, Opacity, OpacityKind, Options, area, finish, syntax, unsupported,
};

/// A box type (T.800 I.4, Table I.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BoxType([u8; 4]);

impl BoxType {
    /// JPEG 2000 signature box (T.800 I.5.1).
    const SIGNATURE: Self = Self(*b"jP  ");
    /// File type box (T.800 I.5.2).
    const FILE_TYPE: Self = Self(*b"ftyp");
    /// JP2 header superbox (T.800 I.5.3).
    const HEADER: Self = Self(*b"jp2h");
    /// Image header box (T.800 I.5.3.1).
    const IMAGE_HEADER: Self = Self(*b"ihdr");
    /// Bits per component box (T.800 I.5.3.2).
    const BIT_DEPTHS: Self = Self(*b"bpcc");
    /// Color specification box (T.800 I.5.3.3).
    const COLOR: Self = Self(*b"colr");
    /// Palette box (T.800 I.5.3.4).
    const PALETTE: Self = Self(*b"pclr");
    /// Component mapping box (T.800 I.5.3.5).
    const COMPONENT_MAPPING: Self = Self(*b"cmap");
    /// Channel definition box (T.800 I.5.3.6).
    const CHANNEL_DEFINITION: Self = Self(*b"cdef");
    /// Resolution superbox (T.800 I.5.3.7).
    const RESOLUTION: Self = Self(*b"res ");
    /// Capture resolution box (T.800 I.5.3.7.1).
    const CAPTURE_RESOLUTION: Self = Self(*b"resc");
    /// Default display resolution box (T.800 I.5.3.7.2).
    const DISPLAY_RESOLUTION: Self = Self(*b"resd");
    /// Contiguous codestream box (T.800 I.5.4).
    const CODESTREAM: Self = Self(*b"jp2c");
}

/// The contents of the signature box (T.800 I.5.1).
const SIGNATURE: [u8; 4] = [0x0d, 0x0a, 0x87, 0x0a];
/// The JP2 brand in the file type box (T.800 I.5.2).
const JP2_BRAND: [u8; 4] = *b"jp2 ";
/// The image header's compression type of JPEG 2000 codestreams (T.800 I.5.3.1).
const JPEG_2000_COMPRESSION: u8 = 7;
/// Color specification methods (T.800 I.5.3.3): an enumerated color space,
/// a restricted ICC profile, and an ICC profile (the T.801 extension of
/// method 2).
const ENUMERATED_METHOD: u8 = 1;
const RESTRICTED_ICC_METHOD: u8 = 2;
const ICC_METHOD: u8 = 3;
/// The ICC profile file signature, at header bytes 36 to 39 (ICC.1).
const ICC_SIGNATURE: &[u8; 4] = b"acsp";

struct BoxData<'a> {
    kind: BoxType,
    body: Cursor<'a>,
    /// Where the box starts.
    position: Position,
    /// Whether the length is the 8-byte `XLBox` (T.800 I.4).
    extended: bool,
    /// Whether the box runs to the end of its container (`LBox` 0).
    to_end: bool,
}

fn next_box<'a>(reader: &mut Cursor<'a>, budget: &mut Budget) -> Result<BoxData<'a>> {
    let position = reader.position();
    budget.work(1).at(position)?;
    let length = reader.u32()?;
    let kind = BoxType(reader.take_array()?);
    let (length, header) = match length {
        0 => (reader.remaining(), 0),
        1 => (
            usize::try_from(reader.uint(8)?)
                .map_err(|_| Error::limit("JP2 box length exceeds address space"))?,
            16,
        ),
        n => (u32_to_usize(n), 8),
    };
    let body = length
        .checked_sub(header)
        .ok_or_else(|| syntax("JP2 box is shorter than its header", position))?;
    Ok(BoxData {
        kind,
        body: reader.take_cursor(body)?,
        position,
        extended: header == 16,
        to_end: header == 0,
    })
}

#[derive(Default)]
pub(super) struct Header {
    dimensions: Option<Dimensions>,
    depths: Option<Vec<u8>>,
    color: Option<JpxColor>,
    palette: Option<Palette>,
    mapping: Option<Vec<Mapping>>,
    channels: Vec<ChannelDefinition>,
    /// Whether to reorder the channels and report opacity by `channels`.
    interpret_channels: bool,
    /// Where the boxes the codestream is checked against start.
    positions: Positions,
}

/// Where the header's boxes start, for errors found once the codestream is
/// decoded.
#[derive(Clone, Copy, Default)]
struct Positions {
    header: Position,
    dimensions: Position,
    depths: Position,
    palette: Position,
    mapping: Position,
    channels: Position,
}

/// The image header box fields readers check (T.800 I.5.3.1).
#[derive(Clone, Copy)]
struct Dimensions {
    width: u32,
    height: u32,
    components: u16,
    /// `BPC`: the common bit depth, or 255 when a bit depth box gives each.
    depth: u8,
}

/// One component mapping entry (T.800 I.5.3.5).
#[derive(Clone, Copy)]
struct Mapping {
    /// `CMP`: the codestream component.
    component: u16,
    /// `MTYP` and `PCOL`: how the channel comes from the component.
    source: Source,
}

/// How a channel comes from a codestream component (T.800 I.5.3.5).
#[derive(Clone, Copy)]
enum Source {
    /// The component's samples (`MTYP` 0, `PCOL` 0).
    Direct,
    /// A palette column looked up by the component's samples (`MTYP` 1).
    Palette(u8),
}

/// Where a channel came from before palette mapping.
#[derive(Clone, Copy)]
struct Origin {
    component: usize,
    palette_column: Option<u8>,
}

struct Palette {
    entries: usize,
    depths: Vec<u8>,
    values: Vec<u16>,
}

/// Reads a JP2 file up to its first codestream box and returns that box's
/// contents with the header that applies to it.
pub(super) fn parse<'a>(
    input: &mut Cursor<'a>,
    pixels: usize,
    budget: &mut Budget,
    options: &Options,
    diagnostics: &mut Diagnostics,
) -> Result<(Cursor<'a>, Header)> {
    let signature = next_box(input, budget)?;
    if signature.kind != BoxType::SIGNATURE
        || signature.body.rest() != SIGNATURE
        || signature.extended
    {
        return Err(syntax("invalid JP2 signature", signature.position));
    }
    let file_type = next_box(input, budget)?;
    let brands = file_type.body.rest();
    if file_type.kind != BoxType::FILE_TYPE || brands.len() < 12 || !brands.len().is_multiple_of(4)
    {
        return Err(syntax("invalid JP2 file type box", file_type.position));
    }
    budget.work(brands.len() / 4).at(file_type.position)?;
    if !brands
        .get(8..)
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .contains(&JP2_BRAND)
    {
        return Err(unsupported(
            "JPX file extensions are not implemented",
            file_type.position,
        ));
    }
    let mut header = None;
    let mut codestream = None;
    while !input.at_end() {
        let b = next_box(input, budget)?;
        match b.kind {
            BoxType::HEADER => {
                if header.is_some() || codestream.is_some() {
                    return Err(syntax("misplaced or repeated JP2 header", b.position));
                }
                header = Some(parse_header(b.body, b.position, pixels, budget, options)?);
            }
            // T.800 I.5.4: readers use the first codestream, after the header.
            BoxType::CODESTREAM if codestream.is_none() => {
                let header = header
                    .take()
                    .ok_or_else(|| syntax("misplaced or repeated JP2 codestream", b.position))?;
                codestream = Some((b.body, header));
            }
            BoxType::COMPONENT_MAPPING => {
                misplaced_mapping(b, header.as_mut(), budget, diagnostics)?;
            }
            BoxType::SIGNATURE | BoxType::FILE_TYPE => {
                return Err(syntax("repeated JP2 signature or file type", b.position));
            }
            _ => {} // Unknown length-delimited metadata is skipped; T.800 I.8 permits unknown metadata.
        }
    }
    codestream.ok_or_else(|| syntax("missing JP2 codestream", input.position()))
}

/// A component mapping box belongs in the JP2 header (T.800 I.5.3.5). One
/// outside it, before the codestream, is used as a reported repair only when
/// it is the sole mapping and the header has the palette it maps; any other
/// stays an error.
fn misplaced_mapping(
    b: BoxData<'_>,
    header: Option<&mut Header>,
    budget: &mut Budget,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let header = header
        .filter(|h| h.palette.is_some() && h.mapping.is_none())
        .ok_or_else(|| syntax("misplaced or repeated JP2 component mapping", b.position))?;
    let mut fields = b.body;
    if fields.at_end() || !fields.remaining().is_multiple_of(4) {
        return Err(syntax("invalid JP2 component mapping", b.position));
    }
    diagnostics.repair(
        Diagnostic::new(
            crate::repairs::JPX_MAPPING_LOCATION,
            "JP2 component mapping outside the header used for its palette (T.800 I.5.3.4, I.5.3.5)",
        )
        .at(b.position),
    )?;
    header.mapping = Some(read_mapping(&mut fields, budget).at(b.position)?);
    header.positions.mapping = b.position;
    finish(&fields)
}

fn parse_header(
    mut reader: Cursor<'_>,
    position: Position,
    pixels: usize,
    budget: &mut Budget,
    options: &Options,
) -> Result<Header> {
    let mut header = Header::default();
    header.positions.header = position;
    let mut color_seen = false;
    let mut color_ended = false;
    let mut resolution_seen = false;
    while !reader.at_end() {
        let b = next_box(&mut reader, budget)?;
        if b.to_end {
            return Err(syntax(
                "zero-length JP2 child in a bounded superbox",
                b.position,
            ));
        }
        if header.dimensions.is_none() && b.kind != BoxType::IMAGE_HEADER {
            return Err(syntax("JP2 image header must be first", b.position));
        }
        if b.kind == BoxType::COLOR {
            if color_ended {
                return Err(syntax(
                    "noncontiguous JP2 colour specifications",
                    b.position,
                ));
            }
            color_seen = true;
        } else if color_seen {
            color_ended = true;
        }
        if b.kind == BoxType::RESOLUTION {
            if std::mem::replace(&mut resolution_seen, true) {
                return Err(syntax("repeated JP2 resolution superbox", b.position));
            }
            parse_resolution(b.body, b.position, budget)?;
        } else {
            header.read_box(b, pixels, budget, options)?;
        }
    }
    if header.dimensions.is_none() || !color_seen {
        return Err(syntax("incomplete JP2 header", position));
    }
    Ok(header)
}

impl Header {
    /// Reads one box of the JP2 header superbox other than the resolution
    /// superbox. Unknown boxes are skipped (T.800 I.8).
    fn read_box(
        &mut self,
        b: BoxData<'_>,
        pixels: usize,
        budget: &mut Budget,
        options: &Options,
    ) -> Result<()> {
        let mut fields = b.body;
        match b.kind {
            BoxType::IMAGE_HEADER => {
                if self.dimensions.is_some() {
                    return Err(syntax("repeated JP2 image header", b.position));
                }
                self.dimensions = Some(read_dimensions(&mut fields, pixels).at(b.position)?);
                self.positions.dimensions = b.position;
            }
            BoxType::BIT_DEPTHS => {
                if self.depths.is_some() {
                    return Err(syntax("repeated JP2 bit depths", b.position));
                }
                let bytes = fields.take_rest();
                let mut depths = budget.filled(bytes.len(), 0).at(b.position)?;
                depths.copy_from_slice(bytes);
                self.depths = Some(depths);
                self.positions.depths = b.position;
            }
            BoxType::COLOR => {
                if fields.remaining() < 3 {
                    return Err(syntax("short JP2 colour specification", b.position));
                }
                // T.800 I.3.3: the first JP2 method is authoritative; others are ignored.
                if options.ignore_color || self.color.is_some() {
                    fields.take_rest();
                } else {
                    self.color = Some(parse_color(&mut fields, budget).at(b.position)?);
                }
            }
            BoxType::PALETTE => {
                if self.palette.is_some() {
                    return Err(syntax("repeated JP2 palette", b.position));
                }
                self.palette = Some(parse_palette(&mut fields, budget).at(b.position)?);
                self.positions.palette = b.position;
            }
            BoxType::COMPONENT_MAPPING => {
                if self.mapping.is_some()
                    || fields.at_end()
                    || !fields.remaining().is_multiple_of(4)
                {
                    return Err(syntax("invalid JP2 component mapping", b.position));
                }
                self.mapping = Some(read_mapping(&mut fields, budget).at(b.position)?);
                self.positions.mapping = b.position;
            }
            BoxType::CHANNEL_DEFINITION => {
                // read_channels rejects an empty definition.
                if !self.channels.is_empty() {
                    return Err(syntax("repeated JP2 channel definition", b.position));
                }
                self.channels = read_channels(&mut fields, budget).at(b.position)?;
                self.interpret_channels = !options.ignore_opacity;
                self.positions.channels = b.position;
            }
            _ => return Ok(()),
        }
        finish(&fields)
    }
}

fn read_channels(fields: &mut Cursor<'_>, budget: &mut Budget) -> Result<Vec<ChannelDefinition>> {
    let count = usize::from(fields.u16()?);
    if count == 0 {
        return Err(fields.error("empty JP2 channel definition"));
    }
    let mut channels = budget.output_with_capacity(count)?;
    for _ in 0..count {
        channels.push(ChannelDefinition {
            channel: fields.u16()?,
            kind: ChannelType::from(fields.u16()?),
            association: Association::from(fields.u16()?),
        });
    }
    Ok(channels)
}

fn read_mapping(fields: &mut Cursor<'_>, budget: &mut Budget) -> Result<Vec<Mapping>> {
    let mut mapping = budget.with_capacity(fields.remaining() / 4)?;
    while !fields.at_end() {
        let start = fields.position();
        let component = fields.u16()?;
        let source = match fields.take_array()? {
            [0, 0] => Source::Direct,
            [1, column] => Source::Palette(column),
            _ => {
                return Err(syntax("invalid JP2 mapping type or palette column", start));
            }
        };
        mapping.push(Mapping { component, source });
    }
    Ok(mapping)
}

fn read_dimensions(fields: &mut Cursor<'_>, pixels: usize) -> Result<Dimensions> {
    let start = fields.position();
    let height = fields.u32()?;
    let width = fields.u32()?;
    let components = fields.u16()?;
    let depth = fields.u8()?;
    let compression = fields.u8()?;
    let unknown_color = fields.u8()?;
    let intellectual_property = fields.u8()?;
    if width == 0
        || height == 0
        || components == 0
        || compression != JPEG_2000_COMPRESSION
        || unknown_color > 1
        || intellectual_property > 1
    {
        return Err(syntax("invalid JP2 image header", start));
    }
    if area(width, height)? > pixels {
        return Err(Error::limit("JP2 pixel budget exceeded"));
    }
    Ok(Dimensions {
        width,
        height,
        components,
        depth,
    })
}

fn parse_color(fields: &mut Cursor<'_>, budget: &mut Budget) -> Result<JpxColor> {
    let method = fields.u8()?;
    fields.skip(2)?; // T.800 I.5.3.3: readers ignore PREC and APPROX.
    match method {
        ENUMERATED_METHOD => {
            // Every enumeration is returned; whether a format admits it, as
            // 7.4.9 does not admit CIEJab, is the caller's decision.
            Ok(JpxColor::Enumerated(EnumeratedColorSpace::new(
                fields.u32()?,
            )))
        }
        RESTRICTED_ICC_METHOD | ICC_METHOD => {
            let mut header = fields.clone();
            let bytes = fields.take_rest();
            if bytes.len() < 128 {
                return Err(fields.error("truncated JP2 ICC profile"));
            }
            let start = header.position();
            if u32_to_usize(header.u32()?) != bytes.len()
                || bytes.get(36..40) != Some(ICC_SIGNATURE)
            {
                return Err(syntax("invalid JP2 ICC profile header", start));
            }
            let mut profile = budget.output(bytes.len(), 0)?;
            profile.copy_from_slice(bytes);
            Ok(JpxColor::Icc(profile))
        }
        _ => Err(unsupported(
            "vendor JPX colour spaces are not implemented",
            fields.position(),
        )),
    }
}

fn parse_palette(fields: &mut Cursor<'_>, budget: &mut Budget) -> Result<Palette> {
    let entries = usize::from(fields.u16()?);
    let columns = usize::from(fields.u8()?);
    if entries == 0 || entries > 1024 || columns == 0 {
        return Err(fields.error("empty JP2 palette"));
    }
    let mut depths = budget.filled(columns, 0)?;
    depths.copy_from_slice(fields.take(columns)?);
    let mut values = budget.filled(entries * columns, 0)?;
    for row in values.chunks_exact_mut(columns) {
        for (value, &depth) in row.iter_mut().zip(&depths) {
            let bits = (depth & 127) + 1;
            if bits > 38 {
                return Err(fields.error("invalid JP2 palette precision"));
            }
            if bits > 16 {
                return Err(unsupported(
                    "JP2 palette precision exceeds 16 bits",
                    fields.position(),
                ));
            }
            let raw = if bits <= 8 {
                u16::from(fields.u8()?)
            } else {
                fields.u16()?
            };
            let mask = u16::MAX >> (16 - bits);
            if raw & !mask != 0 {
                return Err(fields.error("nonzero JP2 palette sample padding"));
            }
            // Signed palette entries have the same unsigned-offset convention as components.
            *value = (raw & mask) ^ if depth & 128 == 0 { 0 } else { 1 << (bits - 1) };
        }
    }
    Ok(Palette {
        entries,
        depths,
        values,
    })
}

fn parse_resolution(mut reader: Cursor<'_>, position: Position, budget: &mut Budget) -> Result<()> {
    let (mut capture, mut display) = (false, false);
    while !reader.at_end() {
        let b = next_box(&mut reader, budget)?;
        if b.to_end {
            return Err(syntax(
                "zero-length JP2 child in a bounded superbox",
                b.position,
            ));
        }
        let seen = match b.kind {
            BoxType::CAPTURE_RESOLUTION => &mut capture,
            BoxType::DISPLAY_RESOLUTION => &mut display,
            _ => continue,
        };
        if std::mem::replace(seen, true) {
            return Err(syntax("repeated JP2 resolution", b.position));
        }
        let mut fields = b.body;
        let vertical_numerator = fields.u16()?;
        let vertical_denominator = fields.u16()?;
        let horizontal_numerator = fields.u16()?;
        let horizontal_denominator = fields.u16()?;
        fields.skip(2)?; // Decimal exponents; resolution does not alter decoded pixels.
        finish(&fields)?;
        if [
            vertical_numerator,
            vertical_denominator,
            horizontal_numerator,
            horizontal_denominator,
        ]
        .contains(&0)
        {
            return Err(syntax("zero JP2 resolution ratio", b.position));
        }
    }
    if !capture && !display {
        return Err(syntax("empty JP2 resolution superbox", position));
    }
    Ok(())
}

impl Header {
    /// Drops the palette with its mapping and channel definitions, for
    /// [`Options::ignore_palette`]. A palette and a mapping still come as a
    /// pair (T.800 I.5.3.4, I.5.3.5).
    pub(super) fn ignore_palette(&mut self) -> Result<()> {
        match (&self.palette, &self.mapping) {
            (Some(_), None) => Err(syntax(
                "JP2 palette and mapping disagree",
                self.positions.palette,
            )),
            (None, Some(_)) => Err(syntax(
                "JP2 palette and mapping disagree",
                self.positions.mapping,
            )),
            (None, None) => Ok(()),
            (Some(_), Some(_)) => {
                self.palette = None;
                self.mapping = None;
                self.channels = Vec::new();
                self.interpret_channels = false;
                Ok(())
            }
        }
    }

    fn check_image(&self, image: &JpxImage) -> Result<()> {
        if let Some(dimensions) = self.dimensions {
            if (
                dimensions.width,
                dimensions.height,
                usize::from(dimensions.components),
            ) != (image.width, image.height, image.components.len())
            {
                return Err(syntax(
                    "JP2 header disagrees with codestream dimensions",
                    self.positions.dimensions,
                ));
            }
            if dimensions.depth == 255 {
                let depths = self.depths.as_ref().ok_or_else(|| {
                    syntax(
                        "JP2 variable bit depths are missing",
                        self.positions.dimensions,
                    )
                })?;
                if depths.len() != image.components.len() {
                    return Err(syntax(
                        "incorrect JP2 bit depth count",
                        self.positions.depths,
                    ));
                }
                for (&d, c) in depths.iter().zip(&image.components) {
                    check_depth(d, c, self.positions.depths)?;
                }
            } else {
                if self.depths.is_some() {
                    return Err(syntax(
                        "unexpected JP2 variable bit depths",
                        self.positions.depths,
                    ));
                }
                for c in &image.components {
                    check_depth(dimensions.depth, c, self.positions.dimensions)?;
                }
            }
        }
        Ok(())
    }

    /// Checks the decoded codestream against the header and applies the
    /// palette, component mapping, channel definitions and color to it.
    pub(super) fn apply(self, decoded: &mut Decoded, budget: &mut Budget) -> Result<()> {
        let position = self.positions.header;
        self.apply_at(decoded, budget).at(position)
    }

    fn apply_at(self, decoded: &mut Decoded, budget: &mut Budget) -> Result<()> {
        self.check_image(&decoded.image)?;
        let positions = self.positions;
        let origins = match (self.palette, self.mapping) {
            (Some(palette), Some(mapping)) => {
                map_components(decoded, &palette, &mapping, budget).at(positions.mapping)?
            }
            (None, None) => {
                let count = decoded.image.components.len();
                let mut origins = budget.with_capacity(count)?;
                origins.extend((0..count).map(|component| Origin {
                    component,
                    palette_column: None,
                }));
                origins
            }
            (Some(_), None) => {
                return Err(syntax(
                    "JP2 palette and mapping disagree",
                    positions.palette,
                ));
            }
            (None, Some(_)) => {
                return Err(syntax(
                    "JP2 palette and mapping disagree",
                    positions.mapping,
                ));
            }
        };
        decoded.image.color = self.color.unwrap_or(JpxColor::Unspecified);
        if self.interpret_channels {
            apply_channels(decoded, &self.channels, &origins, budget).at(positions.channels)?;
        }
        decoded.channel_definitions = self.channels;
        Ok(())
    }
}

/// Replaces the codestream components by the channels the component mapping
/// makes of them (T.800 I.5.3.4, I.5.3.5) and returns where each came from.
fn map_components(
    decoded: &mut Decoded,
    palette: &Palette,
    mapping: &[Mapping],
    budget: &mut Budget,
) -> Result<Vec<Origin>> {
    let image = &mut decoded.image;
    let mut components = budget.output_with_capacity(mapping.len())?;
    let mut geometry = budget.output_with_capacity(mapping.len())?;
    let mut origins = budget.with_capacity(mapping.len())?;
    for &Mapping { component, source } in mapping {
        let component = usize::from(component);
        let (Some(input), Some(&placement)) = (
            image.components.get(component),
            decoded.component_geometry.get(component),
        ) else {
            return Err(Error::syntax("JP2 mapping references missing component"));
        };
        geometry.push(placement);
        let mut result = JpxComponent {
            bits: input.bits,
            signed: input.signed,
            width: input.width,
            height: input.height,
            samples: budget.output(input.samples.len(), 0)?,
        };
        let column = match source {
            Source::Direct => {
                result.samples.copy_from_slice(&input.samples);
                None
            }
            Source::Palette(column) => {
                let &depth = palette
                    .depths
                    .get(usize::from(column))
                    .ok_or(Error::syntax("JP2 palette column out of range"))?;
                result.bits = (depth & 127) + 1;
                result.signed = depth & 128 != 0;
                budget.work(input.samples.len())?;
                look_up(palette, usize::from(column), input, &mut result.samples);
                Some(column)
            }
        };
        origins.push(Origin {
            component,
            palette_column: column,
        });
        components.push(result);
    }
    image.components = components;
    decoded.component_geometry = geometry;
    Ok(origins)
}

/// Maps the samples of `input` through palette column `column`.
fn look_up(palette: &Palette, column: usize, input: &JpxComponent, output: &mut [u16]) {
    let offset = if input.signed {
        1_i32 << (input.bits - 1)
    } else {
        0
    };
    for (out, &index) in output.iter_mut().zip(&input.samples) {
        // T.800 I.5.3.4 leaves out-of-table lookup unspecified; choose endpoint clamping.
        let index = usize::try_from(i32::from(index) - offset)
            .unwrap_or(0)
            .min(palette.entries - 1);
        // parse_palette stores `entries` rows of one value per column.
        *out = palette.values[index * palette.depths.len() + column];
    }
}

fn check_depth(depth: u8, component: &JpxComponent, position: Position) -> Result<()> {
    if (depth & 127) + 1 != component.bits || (depth & 128 != 0) != component.signed {
        return Err(syntax("JP2 bit depth disagrees with codestream", position));
    }
    Ok(())
}

/// Reorders the channels by their color associations, with the opacity
/// channel last, and reports the opacity (T.800 I.5.3.6). Only complete
/// definitions with whole-image opacity are interpreted.
fn apply_channels(
    decoded: &mut Decoded,
    definitions: &[ChannelDefinition],
    origins: &[Origin],
    budget: &mut Budget,
) -> Result<()> {
    let count = decoded.image.components.len();
    debug_assert_eq!(origins.len(), count, "one origin per channel");
    let Classes { mut order, alpha } = classify(definitions, &decoded.image.components, budget)?;
    let colors = count - usize::from(alpha.is_some());
    let (color_order, rest) = order.split_at_mut(colors);
    if color_order.contains(&None) || rest.iter().any(Option::is_some) {
        return Err(Error::syntax("incomplete JP2 colour associations"));
    }
    if let Some((index, kind)) = alpha {
        // The opacity channel goes last; select_channels moves its index.
        if let Some(slot) = rest.first_mut() {
            *slot = Some(index);
        }
        let origin = origins.get(index);
        decoded.image.alpha = Some(index);
        decoded.opacity = Some(Opacity {
            kind,
            channel: index,
            source_channel: index,
            source_component: origin.map_or(index, |origin| origin.component),
            palette_column: origin.and_then(|origin| origin.palette_column),
        });
    }
    // Every slot holds a channel now. Swaps keep the sample allocations.
    let order: Vec<usize> = order.into_iter().flatten().collect();
    budget.work(count)?;
    budget.memory::<usize>(count.saturating_mul(3))?;
    decoded.select_channels(&order)
}

/// The channels a complete channel definition associates.
struct Classes {
    /// For each color, the channel associated with it.
    order: Vec<Option<usize>>,
    /// The whole-image opacity channel and its kind.
    alpha: Option<(usize, OpacityKind)>,
}

/// Checks that every channel has one definition and classifies them.
fn classify(
    definitions: &[ChannelDefinition],
    components: &[JpxComponent],
    budget: &mut Budget,
) -> Result<Classes> {
    let mut seen = budget.filled(components.len(), false)?;
    // order[i] is the channel that becomes channel i.
    let mut order = budget.filled(components.len(), None)?;
    let mut alpha = None;
    for definition in definitions {
        let index = usize::from(definition.channel);
        let entry = seen
            .get_mut(index)
            .ok_or(Error::syntax("JP2 channel index out of range"))?;
        if std::mem::replace(entry, true) {
            return Err(Error::unsupported(
                super::UNSUPPORTED,
                "multiple descriptions of one JP2 channel are not implemented",
            ));
        }
        let kind = match (definition.kind, definition.association) {
            (ChannelType::Color, Association::Color(color)) => {
                let slot = order
                    .get_mut(usize::from(color.get()) - 1)
                    .ok_or(Error::syntax("JP2 channel association out of range"))?;
                if slot.replace(index).is_some() {
                    return Err(Error::syntax("repeated JP2 colour association"));
                }
                continue;
            }
            (ChannelType::Color | ChannelType::Unspecified, _) => {
                return Err(Error::unsupported(
                    super::UNSUPPORTED,
                    "unassociated JP2 channels are not implemented",
                ));
            }
            (ChannelType::Opacity, Association::Image) => OpacityKind::Straight,
            (ChannelType::PremultipliedOpacity, Association::Image) => OpacityKind::Premultiplied,
            (ChannelType::Opacity | ChannelType::PremultipliedOpacity, _) => {
                return Err(Error::unsupported(
                    super::UNSUPPORTED,
                    "per-colour JP2 opacity is not implemented",
                ));
            }
            (ChannelType::Reserved(_), _) => {
                return Err(Error::syntax("invalid JP2 channel type"));
            }
        };
        if components.get(index).is_some_and(|c| c.signed) {
            return Err(Error::syntax("signed JP2 opacity component"));
        }
        if alpha.replace((index, kind)).is_some() {
            return Err(Error::unsupported(
                super::UNSUPPORTED,
                "multiple JP2 opacity channels are not implemented",
            ));
        }
    }
    // A complete definition is needed to reorder without guessing unspecified associations.
    if seen.contains(&false) {
        return Err(Error::unsupported(
            super::UNSUPPORTED,
            "partial JP2 channel definitions are not implemented",
        ));
    }
    Ok(Classes { order, alpha })
}
