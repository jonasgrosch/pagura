//! JP2 fields, T.800 I.4–I.5 and PDF 7.4.9; Part 2 extensions need T.801.

use pagura_core::{Error, Result};

use super::io::{Budget, Reader, area, syntax};
use super::{
    ColorSelection, ColorSpecification, Decoded, JpxColor, JpxComponent, JpxImage, Options,
    unsupported,
};

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
    dimensions: Option<(u32, u32, u16, u8)>,
    depths: Option<Vec<u8>>,
    colors: Vec<ColorSpecification>,
    palette: Option<Palette>,
    mapping: Option<Vec<(u16, u8, u8)>>,
    channels: Option<Vec<(u16, u16, u16)>>,
    channels_use: ChannelsUse,
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
    let file_type = next_box(&mut reader, budget)?;
    if file_type.kind != b"ftyp"
        || file_type.body.len() < 12
        || !file_type.body.len().is_multiple_of(4)
    {
        return Err(syntax("invalid JP2 file type box", file_type.position));
    }
    budget.work(file_type.body.len() / 4)?;
    if !file_type.body[8..]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|brand| brand == b"jp2 ")
    {
        return Err(unsupported(
            "JPX file extensions are not implemented",
            file_type.position,
        ));
    }
    let mut header = None;
    let mut codestream = None;
    while reader.remaining() != 0 {
        let b = next_box(&mut reader, budget)?;
        match b.kind {
            b"jp2h" => {
                if header.is_some() || codestream.is_some() {
                    return Err(syntax("misplaced or repeated JP2 header", b.position));
                }
                header = Some(parse_header(
                    b.body,
                    b.position,
                    budget,
                    options,
                    selection,
                    file_type.body.get(..4) == Some(b"jpx "),
                )?);
            }
            b"jp2c" => {
                if header.is_none() {
                    return Err(syntax("misplaced or repeated JP2 codestream", b.position));
                }
                if codestream.is_none() {
                    codestream = Some((b.body, b.position));
                }
            }
            b"cmap" => misplaced_mapping(&b, header.as_mut(), codestream.is_some(), budget)?,
            b"jP  " | b"ftyp" => {
                return Err(syntax("repeated JP2 signature or file type", b.position));
            }
            _ => {} // Unknown length-delimited metadata is skipped; T.800 I.8 permits unknown metadata.
        }
    }
    let (stream, position) =
        codestream.ok_or_else(|| syntax("missing JP2 codestream", data.len()))?;
    Ok((
        stream,
        position,
        header.ok_or_else(|| syntax("missing JP2 header", 0))?,
    ))
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

fn parse_header(
    data: &[u8],
    base: usize,
    budget: &mut Budget<'_>,
    options: &Options,
    selection: ColorSelection,
    ranked: bool,
) -> Result<Header> {
    let mut reader = Reader::new(data, base);
    let mut header = Header {
        channels_use: match (options.ignore_opacity, selection) {
            (false, _) => ChannelsUse::Apply,
            (true, ColorSelection::First) => ChannelsUse::Ignore,
            (true, ColorSelection::Pdf) => ChannelsUse::DiscardOpacity,
        },
        ..Header::default()
    };
    let mut color_seen = false;
    let mut color_ended = false;
    let mut resolution_seen = false;
    let mut channels_seen = false;
    while reader.remaining() != 0 {
        let b = next_box(&mut reader, budget)?;
        if b.to_end {
            return Err(syntax(
                "zero-length JP2 child in a bounded superbox",
                b.position,
            ));
        }
        if header.dimensions.is_none() && b.kind != b"ihdr" {
            return Err(syntax("JP2 image header must be first", b.position));
        }
        if b.kind == b"colr" {
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
        let mut fields = Reader::new(b.body, b.position);
        match b.kind {
            b"ihdr" => {
                if header.dimensions.is_some() {
                    return Err(syntax("repeated JP2 image header", b.position));
                }
                header.dimensions = Some(read_dimensions(&mut fields, budget)?);
            }
            b"bpcc" => {
                if header.depths.is_some() {
                    return Err(syntax("repeated JP2 bit depths", b.position));
                }
                budget.charge::<u8>(b.body.len())?;
                header.depths = Some(fields.take(fields.remaining())?.to_vec());
            }
            b"colr" => {
                if fields.remaining() < 3 {
                    return Err(syntax("short JP2 colour specification", b.position));
                }
                // A standalone JP2 reader follows T.800 I.3.3; PDF 7.4.9
                // additionally requires choices after an unsupported profile.
                if options.ignore_color
                    || (selection == ColorSelection::First && !header.colors.is_empty())
                {
                    fields.take(fields.remaining())?;
                } else {
                    let choice = parse_color(&mut fields, budget, selection, ranked)?;
                    budget.push(&mut header.colors, choice)?;
                }
            }
            b"pclr" => {
                if header.palette.is_some() {
                    return Err(syntax("repeated JP2 palette", b.position));
                }
                header.palette = Some(parse_palette(&mut fields, budget)?);
            }
            b"cmap" => {
                if header.mapping.is_some()
                    || fields.remaining() == 0
                    || !fields.remaining().is_multiple_of(4)
                {
                    return Err(syntax("invalid JP2 component mapping", b.position));
                }
                header.mapping = Some(read_mapping(&mut fields, budget)?);
            }
            b"cdef" => {
                if channels_seen {
                    return Err(syntax("repeated JP2 channel definition", b.position));
                }
                channels_seen = true;
                header.channels = Some(read_channels(&mut fields, budget)?);
            }
            b"res " => {
                if resolution_seen {
                    return Err(syntax("repeated JP2 resolution superbox", b.position));
                }
                resolution_seen = true;
                parse_resolution(b.body, b.position, budget)?;
            }
            _ => {
                continue;
            }
        }
        // The resolution superbox was consumed by its own reader.
        if b.kind != b"res " {
            fields.finish()?;
        }
    }
    if header.dimensions.is_none() || !color_seen {
        return Err(syntax("incomplete JP2 header", base));
    }
    Ok(header)
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
    ranked: bool,
) -> Result<ColorSpecification> {
    let method = fields.u8()?;
    let precedence = i8::from_be_bytes([fields.u8()?]);
    let approximation = fields.u8()?;
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
            JpxColor::Enumerated(color)
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
        let image = &mut decoded.image;
        self.check_image(image)?;
        if self.palette.is_some() != self.mapping.is_some() {
            return Err(syntax("JP2 palette and mapping disagree", 0));
        }
        let mapping = self.mapping;
        let mut origins = budget.filled(image.components.len(), (0_usize, None))?;
        for (index, source) in origins.iter_mut().enumerate() {
            source.0 = index;
        }
        if let Some(mapping) = mapping {
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
                        let palette = self
                            .palette
                            .as_ref()
                            .ok_or_else(|| syntax("missing JP2 palette", 0))?;
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
            origins = mapped_origins;
        }
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

fn check_depth(depth: u8, component: &JpxComponent) -> Result<()> {
    if (depth & 127) + 1 != component.bits || (depth & 128 != 0) != component.signed {
        return Err(syntax("JP2 bit depth disagrees with codestream", 0));
    }
    Ok(())
}
