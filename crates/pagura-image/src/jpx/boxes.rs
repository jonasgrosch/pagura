//! JP2/JPX fields, T.800 I.4–I.5, T.801 M.8/M.11 and PDF 7.4.9.

use std::borrow::Cow;
use std::ops::Range;

use pagura_core::{Error, Position, Result};

use super::io::{Budget, Reader, Span, area, source_position, syntax};
use super::{
    ColorSelection, ColorSpecification, Decoded, JpxColor, JpxComponent, JpxImage, Options,
    unsupported,
};

// T.801 Table M.33 (Opacity box) and T.800 Table I.16 (Channel Definition).
const OPACITY_STRAIGHT: u8 = 0;
const OPACITY_PREMULTIPLIED: u8 = 1;
const OPACITY_CHROMA_KEY: u8 = 2;
const CHANNEL_STRAIGHT: u16 = 1;
const CHANNEL_PREMULTIPLIED: u16 = 2;

struct BoxData<'a> {
    kind: &'a [u8],
    body: &'a [u8],
    position: usize,
    to_end: bool,
}

fn next_box<'a>(reader: &mut Reader<'a>, budget: &mut Budget<'_>) -> Result<BoxData<'a>> {
    budget.work(1)?;
    let start = reader.position();
    let length = reader.u32()?;
    let to_end = length == 0;
    let kind = reader.take(4)?;
    let (length, header) = match length {
        0 => (reader.remaining(), 0),
        1 => {
            let high = u64::from(reader.u32()?);
            let length = (high << 32) | u64::from(reader.u32()?);
            (
                usize::try_from(length)
                    .map_err(|_| Error::limit("JP2 box length exceeds address space"))?,
                16,
            )
        }
        n => (super::io::usize_from(n)?, 8),
    };
    let body_len = length
        .checked_sub(header)
        .ok_or_else(|| syntax("JP2 box is shorter than its header", start))?;
    let position = reader.position();
    Ok(BoxData {
        kind,
        body: reader.take(body_len)?,
        position,
        to_end,
    })
}

#[derive(Default)]
pub(super) struct Header {
    base: usize,
    dimensions: Option<(u32, u32, u16, u8)>,
    depths: Option<Vec<u8>>,
    colors: Vec<ColorSpecification>,
    palette: Option<Palette>,
    mapping: Option<Vec<(u16, u8, u8)>>,
    channels: Option<Vec<(u16, u16, u16)>>,
    channels_use: ChannelsUse,
    opacity: Option<super::OpacityKind>,
    color_present: bool,
    baseline_color: bool,
    resolution_seen: bool,
    reference_end: usize,
}

#[derive(Default, PartialEq, Eq)]
enum ChannelsUse {
    #[default]
    Apply,
    Ignore,
    DiscardOpacity,
}

struct Palette {
    entries: usize,
    depths: Vec<u8>,
    values: Vec<u16>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileType {
    Jp2,
    JpxCompatible,
    JpxBaseline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderLocation {
    Defaults,
    Codestream,
    Layer,
    Colors,
}

impl HeaderLocation {
    /// Owned fields are read; unrelated metadata is skipped after validation.
    fn accepts(self, b: &BoxData<'_>) -> Result<bool> {
        let kind = b.kind;
        let accepted = match self {
            Self::Defaults => matches!(
                kind,
                b"ihdr" | b"bpcc" | b"colr" | b"pclr" | b"cmap" | b"cdef" | b"res "
            ),
            Self::Codestream => matches!(kind, b"ihdr" | b"bpcc" | b"pclr" | b"cmap"),
            Self::Layer => matches!(kind, b"cgrp" | b"opct" | b"cdef" | b"creg" | b"res "),
            Self::Colors => kind == b"colr",
        };
        if accepted {
            return Ok(true);
        }
        if matches!(
            kind,
            b"ihdr"
                | b"bpcc"
                | b"colr"
                | b"pclr"
                | b"cmap"
                | b"cdef"
                | b"res "
                | b"cgrp"
                | b"opct"
                | b"creg"
                | b"cref"
        ) {
            return Err(syntax("misplaced JPX header field", b.position));
        }
        if self != Self::Colors
            && matches!(
                kind,
                b"lbl " | b"jp2i" | b"xml " | b"uuid" | b"uinf" | b"free"
            )
        {
            return Ok(false);
        }
        Err(unsupported(
            "JPX image-header extension is not implemented",
            b.position,
        ))
    }
}

struct HeaderPolicy<'a> {
    file: &'a [u8],
    options: &'a Options,
    selection: ColorSelection,
    format: FileType,
}

/// A referenced field's contents, not a second box parser (T.801 M.11.4).
/// Only fragmented metadata is copied; the codestream remains borrowed.
struct Reference<'a> {
    kind: &'a [u8],
    body: Cow<'a, [u8]>,
    base: usize,
    spans: Vec<Span>,
    end: usize,
}

impl<'a> Reference<'a> {
    fn read(b: &BoxData<'a>, file: &'a [u8], budget: &mut Budget<'_>) -> Result<Self> {
        budget.work(b.body.len())?;
        let mut fields = Reader::new(b.body, b.position);
        let kind = fields.take(4)?;
        if kind == b"cref" {
            return Err(syntax(
                "JPX cross-reference targets another reference",
                b.position,
            ));
        }
        let list = next_box(&mut fields, budget)?;
        fields.finish()?;
        if list.kind != b"flst" || list.to_end {
            return Err(syntax("invalid JPX reference fragment list", list.position));
        }
        let mut entries = Reader::new(list.body, list.position);
        let count = usize::from(entries.u16()?);
        if entries.remaining() != count * 14 {
            return Err(syntax("incorrect JPX fragment count", list.position));
        }
        let records = entries.take(entries.remaining())?;
        let base = entries.base + 2;
        let mut first = b.position;
        let mut end = 0;
        let mut total = 0_usize;
        for (index, record) in records.chunks_exact(14).enumerate() {
            let range = fragment(record, base + index * 14, file.len())?;
            if range.start < end {
                return Err(unsupported(
                    "unordered JPX metadata fragments",
                    base + index * 14,
                ));
            }
            if index == 0 {
                first = range.start;
            }
            total = total
                .checked_add(range.len())
                .ok_or_else(|| Error::limit("JPX reference length overflow"))?;
            end = range.end;
        }
        if count <= 1 {
            let body = if count == 0 {
                &[][..]
            } else {
                &file[first..end]
            };
            return Ok(Self {
                kind,
                body: Cow::Borrowed(body),
                base: first,
                spans: Vec::new(),
                end,
            });
        }
        // Validate every extent before reserving. The second bounded traversal
        // copies bytes and records their physical positions for error mapping.
        budget.work(records.len())?;
        budget.charge::<Span>(count)?;
        budget.charge::<u8>(total)?;
        let mut spans = Vec::new();
        spans
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("JPX reference allocation failed"))?;
        let mut body = Vec::new();
        body.try_reserve_exact(total)
            .map_err(|_| Error::limit("JPX reference allocation failed"))?;
        for (index, record) in records.chunks_exact(14).enumerate() {
            let range = fragment(record, base + index * 14, file.len())?;
            spans.push(Span {
                logical: body.len(),
                source: range.start,
            });
            body.extend_from_slice(&file[range]);
        }
        Ok(Self {
            kind,
            body: Cow::Owned(body),
            base: 0,
            spans,
            end,
        })
    }

    fn field(&self) -> BoxData<'_> {
        BoxData {
            kind: self.kind,
            body: &self.body,
            position: self.base,
            to_end: false,
        }
    }

    fn place(&self, mut error: Error) -> Error {
        if !self.spans.is_empty()
            && let Position::Buffer(offset) = error.position
        {
            error.position = Position::Buffer(source_position(&self.spans, offset, self.base));
        }
        error
    }
}

/// Table M.17 offsets are unsigned 64-bit; checked narrowing/overflow is Limit.
fn fragment(record: &[u8], position: usize, file_len: usize) -> Result<Range<usize>> {
    let mut fields = Reader::new(record, position);
    let offset = (u64::from(fields.u32()?) << 32) | u64::from(fields.u32()?);
    let length = u64::from(fields.u32()?);
    let end = offset.checked_add(length).ok_or_else(|| {
        Error::limit("JPX fragment offset overflow").at(Position::Buffer(position))
    })?;
    let start = usize::try_from(offset).map_err(|_| {
        Error::limit("JPX fragment offset exceeds address space").at(Position::Buffer(position))
    })?;
    let end = usize::try_from(end).map_err(|_| {
        Error::limit("JPX fragment extent exceeds address space").at(Position::Buffer(position))
    })?;
    if fields.u16()? != 0 {
        return Err(unsupported(
            "external JPX metadata reference",
            position + 12,
        ));
    }
    if offset < 12 || end > file_len {
        return Err(syntax("JPX fragment lies outside its file", position));
    }
    Ok(start..end)
}

fn file_type(box_data: &BoxData<'_>, budget: &mut Budget<'_>) -> Result<FileType> {
    let body = box_data.body;
    if box_data.kind != b"ftyp" || body.len() < 12 || !body.len().is_multiple_of(4) {
        return Err(syntax("invalid JP2 file type box", box_data.position));
    }
    budget.work(body.len() / 4)?;
    let brands = body[8..].as_chunks::<4>().0;
    if brands.contains(b"jp2 ") {
        return Ok(if body.get(..4) == Some(b"jpx ") {
            FileType::JpxCompatible
        } else {
            FileType::Jp2
        });
    }
    // T.801 M.8/M.9.2 and Cor.4 item 1: the baseline reader label is
    // additional to jpx compatibility, and the corrected minor version is 1.
    if body.get(..4) == Some(b"jpx ")
        && brands.contains(b"jpx ")
        && brands.contains(b"jpxb")
        && body.get(4..8) == Some(&[0, 0, 0, 1])
    {
        return Ok(FileType::JpxBaseline);
    }
    Err(unsupported(
        "JPX file extensions are not implemented",
        box_data.position,
    ))
}

/// Corrected T.801 M.6.3/M.11.1 and Cor.4 Table M.14. Display Contents is
/// a conjunction of alternatives: OR the masks of supported features, then
/// require every DCM bit. FUAM can additionally require non-display metadata.
fn reader_requirements(box_data: &BoxData<'_>, budget: &mut Budget<'_>) -> Result<()> {
    if box_data.kind != b"rreq" {
        return Err(syntax(
            "JPX reader requirements must follow file type",
            box_data.position,
        ));
    }
    budget.work(box_data.body.len())?;
    let mut fields = Reader::new(box_data.body, box_data.position);
    let length = usize::from(fields.u8()?);
    if !matches!(length, 1 | 2 | 4 | 8) {
        return Err(syntax("invalid JPX requirements mask length", fields.base));
    }
    fields.take(length)?; // FUAM; displaying a PDF does not require editing all metadata.
    let display = fields.take(length)?;
    let mut supported = [0_u8; 8];
    for _ in 0..fields.u16()? {
        let feature = fields.u16()?;
        let mask = fields.take(length)?;
        // Part 1 profiles, the two cdef opacity types, PDF's CMYK space and
        // default/explicit CIELab. Colour selection still validates the
        // illuminant and sample interpretation before using CIELab.
        // The codestream and header decoders still enforce their actual
        // precision/layout limits; declaring a feature cannot bypass them.
        // Deprecated flags contribute nothing (Cor.4 item 2).
        if matches!(feature, 4 | 5 | 9 | 10 | 55 | 56 | 57) {
            for (byte, &value) in supported.iter_mut().zip(mask) {
                *byte |= value;
            }
        }
    }
    for _ in 0..fields.u16()? {
        fields.take(16 + length)?; // Unknown vendor UUID and mask grant no capability.
    }
    fields.finish()?;
    if display
        .iter()
        .zip(supported)
        .any(|(&required, have)| required & !have != 0)
    {
        return Err(unsupported(
            "JPX required display feature is not implemented",
            fields.base,
        ));
    }
    Ok(())
}

pub(super) fn parse<'a>(
    data: &'a [u8],
    budget: &mut Budget<'_>,
    options: &Options,
    selection: ColorSelection,
) -> Result<(&'a [u8], usize, Header)> {
    let mut reader = Reader::new(data, 0);
    let signature = next_box(&mut reader, budget)?;
    if signature.kind != b"jP  " || signature.body != [13, 10, 135, 10] || signature.position != 8 {
        return Err(syntax("invalid JP2 signature", 0));
    }
    let format = file_type(&next_box(&mut reader, budget)?, budget)?;
    if format == FileType::JpxBaseline {
        reader_requirements(&next_box(&mut reader, budget)?, budget)?;
    }
    let mut header = None;
    let mut codestream = None;
    let mut codestream_start = 0;
    let mut stream_header_seen = false;
    let mut layer_header_seen = false;
    let policy = HeaderPolicy {
        file: data,
        options,
        selection,
        format,
    };
    while reader.remaining() != 0 {
        let start = reader.position();
        let b = next_box(&mut reader, budget)?;
        match b.kind {
            b"jp2h" => {
                if header.is_some() || codestream.is_some() {
                    return Err(syntax("misplaced or repeated JP2 header", b.position));
                }
                let mut defaults = Header {
                    channels_use: match (options.ignore_opacity, selection) {
                        (false, _) => ChannelsUse::Apply,
                        (true, ColorSelection::First) => ChannelsUse::Ignore,
                        (true, ColorSelection::Pdf) => ChannelsUse::DiscardOpacity,
                    },
                    ..Header::default()
                };
                defaults.read_boxes(&b, HeaderLocation::Defaults, budget, &policy)?;
                header = Some(defaults);
            }
            b"jpch" | b"jplh" if format == FileType::JpxBaseline => {
                let (seen, location) = if b.kind == b"jpch" {
                    (&mut stream_header_seen, HeaderLocation::Codestream)
                } else {
                    (&mut layer_header_seen, HeaderLocation::Layer)
                };
                // M.9.2.2 selects codestream/layer zero. Later headers are
                // length-delimited metadata for images PDF does not display.
                if !*seen {
                    let first = header.as_mut().ok_or_else(|| {
                        syntax("JPX first-image header precedes defaults", b.position)
                    })?;
                    first.read_boxes(&b, location, budget, &policy)?;
                    *seen = true;
                }
            }
            b"jp2c" => {
                if header.is_none() {
                    return Err(syntax("misplaced or repeated JP2 codestream", b.position));
                }
                if codestream.is_none() {
                    codestream = Some((b.body, b.position));
                    codestream_start = start;
                }
            }
            b"mdat" if format == FileType::JpxBaseline => {
                if header.is_none() {
                    return Err(syntax("JPX media data precedes defaults", b.position));
                }
            }
            b"ftbl" if format == FileType::JpxBaseline && codestream.is_some() => {}
            b"cmap" => misplaced_mapping(&b, header.as_mut(), codestream.is_some(), budget)?,
            b"jP  " | b"ftyp" => {
                return Err(syntax("repeated JP2 signature or file type", b.position));
            }
            b"rreq" if format == FileType::JpxBaseline => {
                return Err(syntax("repeated JPX reader requirements", b.position));
            }
            b"jp2i" | b"xml " | b"uuid" | b"uinf" | b"free" => {}
            _ if format == FileType::JpxBaseline => {
                // Selected codestream fragments and other layouts
                // still need their own semantics, even with DCM=0.
                return Err(unsupported(
                    "JPX first-image layout is not implemented",
                    b.position,
                ));
            }
            _ => {} // Unknown length-delimited metadata is skipped; T.800 I.8 permits unknown metadata.
        }
    }
    let (stream, position) =
        codestream.ok_or_else(|| syntax("missing JP2 codestream", data.len()))?;
    let mut header = header.ok_or_else(|| syntax("missing JP2 header", 0))?;
    if format == FileType::JpxBaseline {
        header.finish_first_image(codestream_start, budget)?;
    }
    Ok((stream, position, header))
}

/// A sole misplaced mapping has one matching header palette. Conflicting
/// definitions, missing palettes and mappings after the codestream stay errors.
fn misplaced_mapping(
    box_data: &BoxData<'_>,
    header: Option<&mut Header>,
    after_stream: bool,
    budget: &mut Budget<'_>,
) -> Result<()> {
    let header = header
        .filter(|h| h.palette.is_some() && h.mapping.is_none() && !after_stream)
        .ok_or_else(|| {
            syntax(
                "misplaced or repeated JP2 component mapping",
                box_data.position,
            )
        })?;
    if box_data.body.is_empty() || !box_data.body.len().is_multiple_of(4) {
        return Err(syntax("invalid JP2 component mapping", box_data.position));
    }
    budget.diagnostics.repair(
        pagura_core::Diagnostic::new(
            crate::tags::JPX_MAPPING_LOCATION,
            "top-level JP2 mapping used for its header palette (T.800 I.5.3.4–I.5.3.5)",
        )
        .at(pagura_core::Position::Buffer(box_data.position)),
    )?;
    let mut fields = Reader::new(box_data.body, box_data.position);
    header.mapping = Some(read_mapping(&mut fields, budget)?);
    fields.finish()
}

#[derive(Default, PartialEq, Eq)]
enum ColorRun {
    #[default]
    Absent,
    Contiguous,
    Ended,
}

#[derive(Default)]
struct HeaderFields {
    count: usize,
    color: ColorRun,
    depth: Option<u8>,
    layer_channels: bool,
    registration_seen: bool,
    depths_in_header: bool,
}

impl Header {
    fn read_boxes(
        &mut self,
        container: &BoxData<'_>,
        location: HeaderLocation,
        budget: &mut Budget<'_>,
        policy: &HeaderPolicy<'_>,
    ) -> Result<()> {
        if location == HeaderLocation::Defaults {
            self.base = container.position;
        }
        let mut reader = Reader::new(container.body, container.position);
        let mut state = HeaderFields::default();
        while reader.remaining() != 0 {
            let b = next_box(&mut reader, budget)?;
            if b.to_end {
                return Err(syntax(
                    "zero-length JP2 child in a bounded superbox",
                    b.position,
                ));
            }
            if policy.format == FileType::JpxBaseline
                && b.kind == b"cref"
                && matches!(location, HeaderLocation::Codestream | HeaderLocation::Layer)
            {
                let reference = Reference::read(&b, policy.file, budget)?;
                self.reference_end = self.reference_end.max(reference.end);
                self.read_header(&reference.field(), location, budget, policy, &mut state)
                    .map_err(|error| reference.place(error))?;
            } else {
                self.read_header(&b, location, budget, policy, &mut state)?;
            }
            state.count += 1;
        }
        if policy.format == FileType::JpxBaseline
            && state.depth == Some(255)
            && !state.depths_in_header
        {
            return Err(syntax(
                "JPX variable depths must share their image header",
                container.position,
            ));
        }
        if (location == HeaderLocation::Defaults
            && policy.format != FileType::JpxBaseline
            && (self.dimensions.is_none() || state.color == ColorRun::Absent))
            || (location == HeaderLocation::Colors && state.color == ColorRun::Absent)
        {
            return Err(syntax("incomplete JP2 header", container.position));
        }
        Ok(())
    }

    fn read_header(
        &mut self,
        b: &BoxData<'_>,
        location: HeaderLocation,
        budget: &mut Budget<'_>,
        policy: &HeaderPolicy<'_>,
        state: &mut HeaderFields,
    ) -> Result<()> {
        if location == HeaderLocation::Defaults
            && ((policy.format != FileType::JpxBaseline
                && self.dimensions.is_none()
                && b.kind != b"ihdr")
                || (self.dimensions.is_none() && b.kind == b"ihdr" && state.count != 0))
        {
            return Err(syntax("JP2 image header must be first", b.position));
        }
        if b.kind == b"colr" {
            if state.color == ColorRun::Ended {
                return Err(syntax(
                    "noncontiguous JP2 colour specifications",
                    b.position,
                ));
            }
            state.color = ColorRun::Contiguous;
            self.color_present = true;
        } else if state.color == ColorRun::Contiguous {
            state.color = ColorRun::Ended;
        }
        if policy.format == FileType::JpxBaseline && !location.accepts(b)? {
            return Ok(());
        }
        if location == HeaderLocation::Layer {
            if b.kind == b"cdef" {
                if self.opacity.is_some() {
                    return Err(syntax(
                        "JPX layer combines opacity and channel definitions",
                        b.position,
                    ));
                }
                state.layer_channels = true;
            } else if b.kind == b"opct" && state.layer_channels {
                return Err(syntax(
                    "JPX layer combines opacity and channel definitions",
                    b.position,
                ));
            } else if b.kind == b"creg" {
                if state.registration_seen {
                    return Err(syntax("repeated JPX codestream registration", b.position));
                }
                state.registration_seen = true;
            }
        }
        self.read_field(b, location, budget, policy)?;
        if b.kind == b"ihdr" {
            state.depth = self.dimensions.map(|(_, _, _, depth)| depth);
        } else if b.kind == b"bpcc" {
            state.depths_in_header = true;
        }
        Ok(())
    }

    fn read_field(
        &mut self,
        b: &BoxData<'_>,
        location: HeaderLocation,
        budget: &mut Budget<'_>,
        policy: &HeaderPolicy<'_>,
    ) -> Result<()> {
        let mut fields = Reader::new(b.body, b.position);
        match b.kind {
            b"ihdr" => {
                if self.dimensions.is_some() {
                    return Err(syntax("repeated JP2 image header", b.position));
                }
                self.dimensions = Some(read_dimensions(&mut fields, budget)?);
            }
            b"bpcc" => {
                if self.depths.is_some() {
                    return Err(syntax("repeated JP2 bit depths", b.position));
                }
                budget.charge::<u8>(b.body.len())?;
                self.depths = Some(fields.take(fields.remaining())?.to_vec());
            }
            b"colr" => self.read_color(
                &mut fields,
                budget,
                policy.options,
                policy.selection,
                policy.format,
            )?,
            b"pclr" => {
                if self.palette.is_some() {
                    return Err(syntax("repeated JP2 palette", b.position));
                }
                self.palette = Some(parse_palette(&mut fields, budget)?);
            }
            b"cmap" => {
                if self.mapping.is_some()
                    || fields.remaining() == 0
                    || !fields.remaining().is_multiple_of(4)
                {
                    return Err(syntax("invalid JP2 component mapping", b.position));
                }
                self.mapping = Some(read_mapping(&mut fields, budget)?);
            }
            b"cdef" => {
                if self.channels.is_some() {
                    return Err(syntax("repeated JP2 channel definition", b.position));
                }
                self.channels = Some(read_channels(&mut fields, budget)?);
            }
            b"res " => {
                if self.resolution_seen {
                    return Err(syntax("repeated JP2 resolution superbox", b.position));
                }
                self.resolution_seen = true;
                return parse_resolution(b.body, b.position, budget);
            }
            b"cgrp" if location == HeaderLocation::Layer => {
                if self.color_present {
                    return Err(syntax(
                        "repeated JPX first-image colour specification",
                        b.position,
                    ));
                }
                return self.read_boxes(b, HeaderLocation::Colors, budget, policy);
            }
            b"opct" if location == HeaderLocation::Layer => {
                if self.opacity.is_some() {
                    return Err(syntax("repeated JPX opacity", b.position));
                }
                self.opacity = Some(match fields.u8()? {
                    OPACITY_STRAIGHT => super::OpacityKind::Straight,
                    OPACITY_PREMULTIPLIED => super::OpacityKind::Premultiplied,
                    OPACITY_CHROMA_KEY => {
                        return Err(unsupported(
                            "JPX chroma-key opacity is not implemented",
                            b.position,
                        ));
                    }
                    _ => return Err(syntax("invalid JPX opacity type", b.position)),
                });
            }
            b"creg" if location == HeaderLocation::Layer => read_registration(&mut fields, budget)?,
            _ => return Ok(()), // JP2 I.8 unknown length-delimited metadata.
        }
        fields.finish()
    }

    fn finish_first_image(
        &mut self,
        codestream_start: usize,
        budget: &mut Budget<'_>,
    ) -> Result<()> {
        if self.reference_end > codestream_start {
            return Err(unsupported(
                "JPX metadata reference follows selected codestream",
                codestream_start,
            ));
        }
        let (_, _, components, _) = self
            .dimensions
            .ok_or_else(|| syntax("incomplete JP2 header", self.base))?;
        if !self.color_present || !self.baseline_color {
            return Err(syntax("incomplete JP2 header", self.base));
        }
        if let Some(opacity) = self.opacity {
            let count = self
                .mapping
                .as_ref()
                .map_or(usize::from(components), Vec::len);
            let count = u16::try_from(count)
                .map_err(|_| Error::limit("JPX opacity channel count exceeds range"))?;
            if count < 2 {
                return Err(syntax("JPX opacity requires colour channels", self.base));
            }
            // M.11.7.6 describes the final channels, after palette expansion.
            // Build the same definitions used by cdef and the existing PDF
            // Indexed/SMaskInData owners; stored sample planes are unchanged.
            let mut channels = budget.filled(usize::from(count), (0, 0, 0))?;
            for (channel, definition) in (0..count).zip(&mut channels) {
                *definition = if channel + 1 == count {
                    (
                        channel,
                        if opacity == super::OpacityKind::Straight {
                            CHANNEL_STRAIGHT
                        } else {
                            CHANNEL_PREMULTIPLIED
                        },
                        0,
                    )
                } else {
                    (channel, 0, channel + 1)
                };
            }
            self.channels = Some(channels); // opct overrides default cdef (M.11.7).
        }
        Ok(())
    }
}

fn read_registration(fields: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<()> {
    budget.work(fields.remaining())?;
    let x = fields.u16()?;
    let y = fields.u16()?;
    if x == 0 || y == 0 || fields.remaining() == 0 || !fields.remaining().is_multiple_of(6) {
        return Err(syntax(
            "invalid JPX codestream registration",
            fields.position(),
        ));
    }
    if fields.remaining() != 6 {
        return Err(unsupported(
            "JPX first-layer codestream combination is not implemented",
            fields.position(),
        ));
    }
    let stream = fields.u16()?;
    let xr = fields.u8()?;
    let yr = fields.u8()?;
    let xo = fields.u8()?;
    let yo = fields.u8()?;
    if xr == 0 || yr == 0 {
        return Err(syntax(
            "invalid JPX registration sampling",
            fields.position(),
        ));
    }
    if stream != 0 || u16::from(xr) != x || u16::from(yr) != y || xo != 0 || yo != 0 {
        return Err(unsupported(
            "JPX nonidentity first-layer registration is not implemented",
            fields.position(),
        ));
    }
    Ok(())
}

fn read_channels(fields: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<Vec<(u16, u16, u16)>> {
    let count = usize::from(fields.u16()?);
    if count == 0 {
        return Err(syntax("empty JP2 channel definition", fields.position()));
    }
    let mut channels = budget.filled(count, (0, 0, 0))?;
    for item in &mut channels {
        *item = (fields.u16()?, fields.u16()?, fields.u16()?);
    }
    Ok(channels)
}

fn read_mapping(fields: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<Vec<(u16, u8, u8)>> {
    let mut mapping = budget.filled(fields.remaining() / 4, (0, 0, 0))?;
    for item in &mut mapping {
        *item = (fields.u16()?, fields.u8()?, fields.u8()?);
    }
    Ok(mapping)
}

fn read_dimensions(fields: &mut Reader<'_>, budget: &Budget<'_>) -> Result<(u32, u32, u16, u8)> {
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
        || compression != 7
        || unknown_color > 1
        || intellectual_property > 1
    {
        return Err(syntax("invalid JP2 image header", fields.base));
    }
    if area(width, height)? > budget.limits.pixels {
        return Err(Error::limit("JP2 pixel budget exceeded"));
    }
    Ok((width, height, components, depth))
}

fn parse_color(
    fields: &mut Reader<'_>,
    budget: &mut Budget<'_>,
    selection: ColorSelection,
    format: FileType,
) -> Result<ColorSpecification> {
    let method = fields.u8()?;
    let precedence = i8::from_be_bytes([fields.u8()?]);
    let approximation = fields.u8()?;
    let ranked = format != FileType::Jp2;
    if ranked && selection == ColorSelection::Pdf && !(1..=4).contains(&approximation) {
        return Err(syntax(
            "invalid JPX colour approximation",
            fields.position() - 1,
        ));
    }
    let color = match method {
        1 => {
            let color = fields.u32()?;
            // PDF 7.4.9 excludes this enumerated space even from the baseline set.
            if color == 19 {
                return Err(syntax(
                    "CIEJab colour is forbidden in PDF JPEG 2000 images",
                    fields.position() - 4,
                ));
            }
            if color == 14 && fields.remaining() != 0 {
                if fields.remaining() != 28 {
                    return Err(syntax(
                        "invalid JPX CIELab parameter length",
                        fields.position(),
                    ));
                }
                let mut ranges = [0; 3];
                let mut offsets = [0; 3];
                for (range, offset) in ranges.iter_mut().zip(&mut offsets) {
                    *range = fields.u32()?;
                    *offset = fields.u32()?;
                }
                JpxColor::Lab(super::LabParameters::new(ranges, offsets, fields.u32()?))
            } else {
                JpxColor::Enumerated(color)
            }
        }
        2 | 3 => {
            let bytes = fields.take(fields.remaining())?;
            if bytes.len() < 128 {
                return Err(syntax("truncated JP2 ICC profile", fields.position()));
            }
            let mut profile = Reader::new(bytes, fields.position() - bytes.len());
            if super::io::usize_from(profile.u32()?)? != bytes.len()
                || bytes.get(36..40) != Some(b"acsp")
            {
                return Err(syntax("invalid JP2 ICC profile header", profile.base));
            }
            budget.charge::<u8>(bytes.len())?;
            JpxColor::Icc(bytes.to_vec())
        }
        _ if selection == ColorSelection::Pdf => {
            let bytes = fields.take(fields.remaining())?;
            budget.charge::<u8>(bytes.len())?;
            JpxColor::Other {
                method,
                data: bytes.to_vec(),
            }
        }
        _ => {
            return Err(unsupported(
                "vendor JPX colour spaces are not implemented",
                fields.position(),
            ));
        }
    };
    // T.800 I.5.3.3 reserves these fields in JP2 and requires readers to
    // ignore them. T.801 M.11.7.2 gives them meaning for JPX-branded files.
    Ok(ColorSpecification {
        precedence: if ranked { precedence } else { 0 },
        approximation: if ranked { approximation } else { 0 },
        color,
    })
}

fn parse_palette(fields: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<Palette> {
    let entries = usize::from(fields.u16()?);
    let columns = usize::from(fields.u8()?);
    if entries == 0 || entries > 1024 || columns == 0 {
        return Err(syntax("empty JP2 palette", fields.position()));
    }
    budget.charge::<u8>(columns)?;
    let depths = fields.take(columns)?.to_vec();
    let mut values = budget.filled(entries * columns, 0)?;
    for row in values.chunks_exact_mut(columns) {
        for (value, &depth) in row.iter_mut().zip(&depths) {
            let bits = (depth & 127) + 1;
            if bits > 38 {
                return Err(syntax("invalid JP2 palette precision", fields.position()));
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
                return Err(syntax(
                    "nonzero JP2 palette sample padding",
                    fields.position(),
                ));
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

fn parse_resolution(data: &[u8], base: usize, budget: &mut Budget<'_>) -> Result<()> {
    let mut reader = Reader::new(data, base);
    let mut seen = [false; 2];
    while reader.remaining() != 0 {
        let b = next_box(&mut reader, budget)?;
        if b.to_end {
            return Err(syntax(
                "zero-length JP2 child in a bounded superbox",
                b.position,
            ));
        }
        let index = match b.kind {
            b"resc" => 0,
            b"resd" => 1,
            _ => continue,
        };
        if seen[index] {
            return Err(syntax("repeated JP2 resolution", b.position));
        }
        seen[index] = true;
        let mut fields = Reader::new(b.body, b.position);
        let vertical_numerator = fields.u16()?;
        let vertical_denominator = fields.u16()?;
        let horizontal_numerator = fields.u16()?;
        let horizontal_denominator = fields.u16()?;
        fields.take(2)?; // Decimal exponents; resolution does not alter decoded pixels.
        fields.finish()?;
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
    if !seen.iter().any(|&v| v) {
        return Err(syntax("empty JP2 resolution superbox", base));
    }
    Ok(())
}

impl Header {
    fn read_color(
        &mut self,
        fields: &mut Reader<'_>,
        budget: &mut Budget<'_>,
        options: &Options,
        selection: ColorSelection,
        format: FileType,
    ) -> Result<()> {
        if fields.remaining() < 3 {
            return Err(syntax("short JP2 colour specification", fields.position()));
        }
        if format == FileType::JpxBaseline {
            let approximation = fields.data[fields.offset + 2];
            if !(1..=4).contains(&approximation) {
                return Err(syntax(
                    "invalid JPX colour approximation",
                    fields.position() + 2,
                ));
            }
            self.baseline_color |= approximation <= 3;
        }
        // A standalone JP2 reader follows T.800 I.3.3; PDF 7.4.9
        // additionally requires choices after an unsupported profile.
        if options.ignore_color || (selection == ColorSelection::First && !self.colors.is_empty()) {
            fields.take(fields.remaining())?;
        } else {
            let choice = parse_color(fields, budget, selection, format)?;
            budget.push(&mut self.colors, choice)?;
        }
        Ok(())
    }

    fn check_image(&self, image: &JpxImage) -> Result<()> {
        if let Some((width, height, count, depth)) = self.dimensions {
            if (width, height, usize::from(count))
                != (image.width, image.height, image.components.len())
            {
                return Err(syntax("JP2 header disagrees with codestream dimensions", 0));
            }
            if depth == 255 {
                let depths = self
                    .depths
                    .as_ref()
                    .ok_or_else(|| syntax("JP2 variable bit depths are missing", 0))?;
                if depths.len() != image.components.len() {
                    return Err(syntax("incorrect JP2 bit depth count", 0));
                }
                for (&d, c) in depths.iter().zip(&image.components) {
                    check_depth(d, c)?;
                }
            } else {
                if self.depths.is_some() {
                    return Err(syntax("unexpected JP2 variable bit depths", 0));
                }
                for c in &image.components {
                    check_depth(depth, c)?;
                }
            }
        }
        Ok(())
    }

    /// PDF 7.4.9: an explicit Indexed space interprets the stored indices.
    /// Applying the container's palette too would expand those indices twice.
    pub fn use_pdf_palette(&mut self, budget: &mut Budget<'_>) -> Result<()> {
        if self.palette.is_some() != self.mapping.is_some() {
            return Err(syntax("JP2 palette and mapping disagree", 0));
        }
        if self.palette.is_none() {
            // The raw index and opacity are already distinct channels.
            // Keep cdef so channel selection can validate and reorder them.
            return Ok(());
        }
        let mapping = self
            .mapping
            .as_ref()
            .ok_or_else(|| syntax("missing JP2 mapping", 0))?;
        let palette = self
            .palette
            .as_ref()
            .ok_or_else(|| syntax("missing JP2 palette", 0))?;
        let mut types = budget.filled(mapping.len(), None)?;
        for &(channel, kind, association) in self.channels.iter().flatten() {
            let slot = types
                .get_mut(usize::from(channel))
                .ok_or_else(|| syntax("JP2 channel index out of range", 0))?;
            if slot.replace((kind, association)).is_some() {
                return Err(syntax("repeated Indexed JPX channel definition", 0));
            }
        }
        let mut index = None;
        let mut opacity = None;
        for (channel, &(component, kind, column)) in mapping.iter().enumerate() {
            budget.work(1)?;
            if self
                .dimensions
                .is_none_or(|(_, _, count, _)| component >= count)
                || !matches!((kind, column), (0, 0) | (1, _))
                || (kind == 1 && usize::from(column) >= palette.depths.len())
            {
                return Err(syntax("invalid Indexed JPX component mapping", 0));
            }
            match types[channel] {
                Some((opacity_kind @ (1 | 2), association)) => {
                    if self.channels_use != ChannelsUse::DiscardOpacity
                        && (association != 0
                            || opacity.replace((mapping[channel], opacity_kind)).is_some())
                    {
                        return Err(Error::unsupported(
                            crate::features::JPX_SAMPLE_LAYOUT,
                            "Indexed JPX requires one whole-image opacity channel",
                        ));
                    }
                }
                Some((0, 1..=65534)) | None
                    if self.channels.is_none() || types[channel].is_some() =>
                {
                    if index
                        .replace(component)
                        .is_some_and(|previous| previous != component)
                    {
                        return Err(Error::unsupported(
                            crate::features::JPX_SAMPLE_LAYOUT,
                            "Indexed JPX colour channels require one source index component",
                        ));
                    }
                }
                Some((0 | 65535, _)) | None => {
                    return Err(Error::unsupported(
                        crate::features::JPX_SAMPLE_LAYOUT,
                        "Indexed JPX requires complete colour channel definitions",
                    ));
                }
                _ => return Err(syntax("invalid JP2 channel type", 0)),
            }
        }
        let index = index.ok_or_else(|| syntax("Indexed JPX has no index component", 0))?;
        let mut mapping = budget.filled(1 + usize::from(opacity.is_some()), (index, 0, 0))?;
        let mut channels = budget.filled(mapping.len(), (0, 0, 1))?;
        if let Some((alpha, kind)) = opacity {
            mapping[1] = alpha;
            channels[1] = (1, kind, 0);
        }
        self.mapping = Some(mapping);
        self.channels = Some(channels);
        Ok(())
    }

    pub fn apply(self, decoded: &mut Decoded, budget: &mut Budget<'_>) -> Result<()> {
        self.check_image(&decoded.image)?;
        let origins = match (self.mapping, self.palette) {
            (Some(mapping), Some(palette)) => palette.apply(decoded, mapping, budget)?,
            (None, None) => {
                let mut origins = budget.filled(decoded.image.components.len(), (0_usize, None))?;
                for (index, source) in origins.iter_mut().enumerate() {
                    source.0 = index;
                }
                origins
            }
            _ => return Err(syntax("JP2 palette and mapping disagree", 0)),
        };
        let image = &mut decoded.image;
        if let Some(channels) = self.channels {
            match self.channels_use {
                ChannelsUse::Apply => {
                    decoded.opacity = super::channels::apply(
                        image,
                        &channels,
                        &origins,
                        &mut decoded.component_geometry,
                        budget,
                    )?;
                }
                ChannelsUse::DiscardOpacity => {
                    super::channels::discard_opacity(
                        image,
                        &channels,
                        &mut decoded.component_geometry,
                        budget,
                    )?;
                }
                ChannelsUse::Ignore => {}
            }
        }
        let mut colors = self.colors;
        image.color = match colors.len() {
            0 => JpxColor::Unspecified,
            1 => colors.remove(0).color,
            _ => {
                budget.work(
                    colors
                        .len()
                        .saturating_mul(super::io::usize_from(colors.len().ilog2() + 1)?),
                )?;
                budget.charge::<ColorSpecification>(colors.len())?;
                colors.sort_by_key(|choice| {
                    (std::cmp::Reverse(choice.precedence), choice.approximation)
                });
                JpxColor::Alternatives(colors)
            }
        };
        Ok(())
    }
}

impl Palette {
    fn apply(
        &self,
        decoded: &mut Decoded,
        mapping: Vec<(u16, u8, u8)>,
        budget: &mut Budget<'_>,
    ) -> Result<Vec<(usize, Option<u8>)>> {
        let image = &mut decoded.image;
        budget.charge::<JpxComponent>(mapping.len())?;
        let mut components = Vec::with_capacity(mapping.len());
        let mut geometry = Vec::new();
        let mut mapped_origins = Vec::new();
        for (component, kind, column) in mapping {
            let source = image
                .components
                .get(usize::from(component))
                .ok_or_else(|| syntax("JP2 mapping references missing component", 0))?;
            budget.push(
                &mut geometry,
                decoded.component_geometry[usize::from(component)],
            )?;
            budget.push(
                &mut mapped_origins,
                (
                    usize::from(component),
                    if kind == 1 { Some(column) } else { None },
                ),
            )?;
            let mut result = JpxComponent {
                bits: source.bits,
                signed: source.signed,
                width: source.width,
                height: source.height,
                samples: budget.filled(source.samples.len(), 0)?,
            };
            match kind {
                0 if column == 0 => result.samples.copy_from_slice(&source.samples),
                1 => {
                    let palette = self;
                    let column = usize::from(column);
                    let &depth = palette
                        .depths
                        .get(column)
                        .ok_or_else(|| syntax("JP2 palette column out of range", 0))?;
                    result.bits = (depth & 127) + 1;
                    result.signed = depth & 128 != 0;
                    budget.work(source.samples.len())?;
                    for (out, &index) in result.samples.iter_mut().zip(&source.samples) {
                        let signed_index = i32::from(index)
                            - if source.signed {
                                1_i32 << (source.bits - 1)
                            } else {
                                0
                            };
                        // I.5.3.4 leaves out-of-table lookup unspecified; choose endpoint clamping.
                        let index = usize::try_from(signed_index)
                            .unwrap_or(0)
                            .min(palette.entries - 1);
                        *out = palette.values[index * palette.depths.len() + column];
                    }
                }
                _ => return Err(syntax("invalid JP2 mapping type or palette column", 0)),
            }
            components.push(result);
        }
        image.components = components;
        decoded.component_geometry = geometry;
        Ok(mapped_origins)
    }
}

fn check_depth(depth: u8, component: &JpxComponent) -> Result<()> {
    if (depth & 127) + 1 != component.bits || (depth & 128 != 0) != component.signed {
        return Err(syntax("JP2 bit depth disagrees with codestream", 0));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
