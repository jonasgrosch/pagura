//! JP2 fields for PDF 7.4.9; T.800/T.801 encodings await primary-text review.

use pagura_core::{Error, Result};

use super::io::{Budget, Reader, area, syntax};
use super::{JpxColor, JpxComponent, JpxImage, unsupported};

struct BoxData<'a> {
    kind: &'a [u8],
    body: &'a [u8],
    position: usize,
}

fn next_box<'a>(reader: &mut Reader<'a>, budget: &mut Budget<'_>) -> Result<BoxData<'a>> {
    budget.work(1)?;
    let start = reader.position();
    let length = reader.u32()?;
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
    })
}

#[derive(Default)]
pub(super) struct Header {
    dimensions: Option<(u32, u32, u16, u8)>,
    depths: Option<Vec<u8>>,
    color: Option<JpxColor>,
    palette: Option<Palette>,
    mapping: Option<Vec<(u16, u8, u8)>>,
    channels: Option<Vec<(u16, u16, u16)>>,
}

struct Palette {
    entries: usize,
    depths: Vec<u8>,
    values: Vec<u16>,
}

pub(super) fn parse<'a>(
    data: &'a [u8],
    budget: &mut Budget<'_>,
) -> Result<(&'a [u8], usize, Header)> {
    let mut reader = Reader::new(data, 0);
    let signature = next_box(&mut reader, budget)?;
    if signature.kind != b"jP  " || signature.body != [13, 10, 135, 10] || signature.position != 8 {
        return Err(syntax("invalid JP2 signature", 0));
    }
    let file_type = next_box(&mut reader, budget)?;
    if file_type.kind != b"ftyp"
        || file_type.body.len() < 8
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
                header = Some(parse_header(b.body, b.position, budget)?);
            }
            b"jp2c" => {
                if codestream.is_some() || header.is_none() {
                    return Err(syntax("misplaced or repeated JP2 codestream", b.position));
                }
                codestream = Some((b.body, b.position));
            }
            b"jP  " | b"ftyp" => {
                return Err(syntax("repeated JP2 signature or file type", b.position));
            }
            _ => {} // Unknown length-delimited metadata is skipped; extension semantics remain pending.
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

fn parse_header(data: &[u8], base: usize, budget: &mut Budget<'_>) -> Result<Header> {
    let mut reader = Reader::new(data, base);
    let mut header = Header::default();
    while reader.remaining() != 0 {
        let b = next_box(&mut reader, budget)?;
        if header.dimensions.is_none() && b.kind != b"ihdr" {
            return Err(syntax("JP2 image header must be first", b.position));
        }
        let mut fields = Reader::new(b.body, b.position);
        match b.kind {
            b"ihdr" => {
                if header.dimensions.is_some() {
                    return Err(syntax("repeated JP2 image header", b.position));
                }
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
                    return Err(syntax("invalid JP2 image header", b.position));
                }
                if area(width, height)? > budget.limits.pixels {
                    return Err(Error::limit("JP2 pixel budget exceeded"));
                }
                header.dimensions = Some((width, height, components, depth));
            }
            b"bpcc" => {
                if header.depths.is_some() {
                    return Err(syntax("repeated JP2 bit depths", b.position));
                }
                budget.charge::<u8>(b.body.len())?;
                header.depths = Some(fields.take(fields.remaining())?.to_vec());
            }
            b"colr" => {
                if header.color.is_some() {
                    return Err(unsupported(
                        "multiple JP2 colour specifications need colour negotiation",
                        b.position,
                    ));
                }
                header.color = Some(parse_color(&mut fields, budget)?);
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
                let count = fields.remaining() / 4;
                let mut mapping = budget.filled(count, (0, 0, 0))?;
                for item in &mut mapping {
                    *item = (fields.u16()?, fields.u8()?, fields.u8()?);
                }
                header.mapping = Some(mapping);
            }
            b"cdef" => {
                if header.channels.is_some() {
                    return Err(syntax("repeated JP2 channel definition", b.position));
                }
                let count = usize::from(fields.u16()?);
                if count == 0 {
                    return Err(syntax("empty JP2 channel definition", b.position));
                }
                let mut channels = budget.filled(count, (0, 0, 0))?;
                for item in &mut channels {
                    *item = (fields.u16()?, fields.u16()?, fields.u16()?);
                }
                header.channels = Some(channels);
            }
            b"res " => parse_resolution(b.body, b.position, budget)?,
            _ => {
                continue;
            }
        }
        // The resolution superbox was consumed by its own reader.
        if b.kind != b"res " {
            fields.finish()?;
        }
    }
    if header.dimensions.is_none() || header.color.is_none() {
        return Err(syntax("incomplete JP2 header", base));
    }
    Ok(header)
}

fn parse_color(fields: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<JpxColor> {
    let method = fields.u8()?;
    let precedence = fields.u8()?;
    let approximation = fields.u8()?;
    if precedence != 0 || approximation != 0 {
        return Err(unsupported(
            "JPX colour precedence and approximation are not implemented",
            fields.position(),
        ));
    }
    match method {
        1 => {
            let color = fields.u32()?;
            // PDF 7.4.9 excludes this enumerated space even from the baseline set.
            if color == 19 {
                return Err(syntax(
                    "CIEJab colour is forbidden in PDF JPEG 2000 images",
                    fields.position() - 4,
                ));
            }
            Ok(JpxColor::Enumerated(color))
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
            Ok(JpxColor::Icc(bytes.to_vec()))
        }
        _ => Err(unsupported(
            "vendor JPX colour spaces are not implemented",
            fields.position(),
        )),
    }
}

fn parse_palette(fields: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<Palette> {
    let entries = usize::from(fields.u16()?);
    let columns = usize::from(fields.u8()?);
    if entries == 0 || columns == 0 {
        return Err(syntax("empty JP2 palette", fields.position()));
    }
    budget.charge::<u8>(columns)?;
    let depths = fields.take(columns)?.to_vec();
    let mut values = budget.filled(entries * columns, 0)?;
    for row in values.chunks_exact_mut(columns) {
        for (value, &depth) in row.iter_mut().zip(&depths) {
            let bits = (depth & 127) + 1;
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
    Ok(())
}

impl Header {
    pub fn apply(self, image: &mut JpxImage, budget: &mut Budget<'_>) -> Result<()> {
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
        if self.palette.is_some()
            != self
                .mapping
                .as_ref()
                .is_some_and(|m| m.iter().any(|&(_, t, _)| t == 1))
        {
            return Err(syntax("JP2 palette and mapping disagree", 0));
        }
        if let Some(mapping) = self.mapping {
            budget.charge::<JpxComponent>(mapping.len())?;
            let mut components = Vec::with_capacity(mapping.len());
            for (component, kind, column) in mapping {
                let source = image
                    .components
                    .get(usize::from(component))
                    .ok_or_else(|| syntax("JP2 mapping references missing component", 0))?;
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
                            // Endpoint clamping remains pending primary T.800 review.
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
        }
        if let Some(channels) = self.channels {
            apply_channels(image, &channels, budget)?;
        }
        image.color = self.color.unwrap_or(JpxColor::Unspecified);
        Ok(())
    }
}

fn check_depth(depth: u8, component: &JpxComponent) -> Result<()> {
    if (depth & 127) + 1 != component.bits || (depth & 128 != 0) != component.signed {
        return Err(syntax("JP2 bit depth disagrees with codestream", 0));
    }
    Ok(())
}

fn apply_channels(
    image: &mut JpxImage,
    definitions: &[(u16, u16, u16)],
    budget: &mut Budget<'_>,
) -> Result<()> {
    let count = image.components.len();
    let mut seen = budget.filled(count, false)?;
    let mut order = budget.filled(count, None)?;
    for &(channel, kind, association) in definitions {
        let index = usize::from(channel);
        let entry = seen
            .get_mut(index)
            .ok_or_else(|| syntax("JP2 channel index out of range", 0))?;
        if *entry {
            return Err(syntax("repeated JP2 channel definition", 0));
        }
        *entry = true;
        match kind {
            0 if association != 0 && association != 65535 => {
                let target = usize::from(association - 1);
                let slot = order
                    .get_mut(target)
                    .ok_or_else(|| syntax("JP2 channel association out of range", 0))?;
                if slot.replace(index).is_some() {
                    return Err(syntax("repeated JP2 colour association", 0));
                }
            }
            0 | 65535 => {
                return Err(unsupported(
                    "unassociated JP2 channels are not implemented",
                    0,
                ));
            }
            1 if association == 0 => {
                if image.alpha.replace(index).is_some() {
                    return Err(unsupported(
                        "multiple JP2 opacity channels are not implemented",
                        0,
                    ));
                }
            }
            1 => return Err(unsupported("per-colour JP2 opacity is not implemented", 0)),
            2 => {
                return Err(unsupported(
                    "premultiplied JP2 opacity needs an explicit image API",
                    0,
                ));
            }
            _ => return Err(syntax("invalid JP2 channel type", 0)),
        }
    }
    // A complete definition is needed to reorder without guessing unspecified associations.
    if seen.contains(&false) {
        return Err(unsupported(
            "partial JP2 channel definitions are not implemented",
            0,
        ));
    }
    let colors = count - usize::from(image.alpha.is_some());
    if order[..colors].contains(&None) || order[colors..].iter().any(Option::is_some) {
        return Err(syntax("incomplete JP2 colour associations", 0));
    }
    if let Some(alpha) = image.alpha {
        order[colors] = Some(alpha);
        image.alpha = Some(colors);
    }
    // Swaps preserve the sample allocations, even for large planes.
    budget.work(count)?;
    budget.charge::<usize>(
        count
            .checked_mul(2)
            .ok_or_else(|| Error::limit("JP2 channel permutation overflow"))?,
    )?;
    let mut current: Vec<usize> = (0..count).collect();
    let mut positions = current.clone();
    for (target, source) in order.into_iter().enumerate() {
        let source = source.ok_or_else(|| syntax("missing JP2 channel association", 0))?;
        let position = positions[source];
        image.components.swap(target, position);
        current.swap(target, position);
        positions[current[target]] = target;
        positions[current[position]] = position;
    }
    Ok(())
}
