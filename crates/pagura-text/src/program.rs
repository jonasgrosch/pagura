//! PDF embedding bindings, ISO 32000-2 9.9.1 Tables 124–125.
use crate::{
    FontKind, descriptor::Descriptor, features, objects::optional, repairs, report::Report,
};
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, ObjectId, Position, Result, Value};
use pagura_document::Document;
use pagura_document::Prefix;
use pagura_font::{FontFormat, FontProgram};

/// Loads the embedded program of `descriptor`. `metrics_font` is the simple
/// font itself, or the CIDFont of a Type 0 font: its subtype selects the
/// program format.
pub(crate) fn load(
    document: &Document,
    descriptor: Descriptor<'_>,
    kind: FontKind,
    metrics_font: &Dictionary,
    rendering: bool,
) -> Result<FontProgram> {
    let Some(key) = descriptor.program_key(document)? else {
        return Err(Error::unsupported(
            features::NON_EMBEDDED,
            "font program is not embedded",
        ));
    };
    let id = descriptor
        .dictionary
        .get(key)
        .and_then(Value::as_reference)
        .ok_or_else(|| Error::syntax("embedded font must be a stream reference"))?;
    load_stream(document, id, key, kind, metrics_font, rendering).map_err(|e| e.in_object(id))
}

/// Alias proof uses the original complete stream and strict font policy, never
/// the rendering prefix or reported segment-length/font-program repairs.
/// Stream reading keeps this document's shared decode/work limits and caches.
pub(crate) fn load_descriptor_recovery(
    document: &Document,
    descriptor: Descriptor<'_>,
    font: &Dictionary,
) -> Result<FontProgram> {
    let id = descriptor
        .dictionary
        .get(b"FontFile2")
        .and_then(Value::as_reference)
        .ok_or_else(|| Error::syntax("FontFile2 stream reference required"))?;
    let object = document.object(id)?;
    let dictionary = object
        .dictionary()
        .ok_or_else(|| Error::syntax("font stream dictionary required"))?;
    let range = object
        .stream_range()
        .ok_or_else(|| Error::syntax("font stream required"))?;
    if length(document, dictionary, b"Length")? != Some(range.len()) {
        return Err(
            Error::syntax("recovered font requires its original encoded Length").in_object(id),
        );
    }
    let format = format(document, dictionary, b"FontFile2", FontKind::TrueType, font)?;
    let declared = length(document, dictionary, b"Length1")?
        .ok_or_else(|| Error::syntax("TrueType font stream has no valid Length1").in_object(id))?;
    let data = document.stream_data_sized(object, declared.min(document.limits().decoded_bytes))?;
    if data.len() != declared {
        return Err(Error::syntax("TrueType Length1 disagrees with decoded size").in_object(id));
    }
    let mut diagnostics = Diagnostics::new(pagura_core::Recovery::Strict);
    FontProgram::parse_with_diagnostics(
        std::sync::Arc::<[u8]>::from(data),
        format,
        &document.limits(),
        &mut diagnostics,
    )
    .map_err(|error| {
        error
            .within(Position::Stream {
                object: id,
                offset: 0,
            })
            .in_object(id)
    })
}

fn load_stream(
    document: &Document,
    id: ObjectId,
    key: &[u8],
    kind: FontKind,
    metrics_font: &Dictionary,
    rendering: bool,
) -> Result<FontProgram> {
    let object = document.object(id)?;
    let dictionary = object
        .dictionary()
        .ok_or_else(|| Error::syntax("embedded font is not a stream"))?;
    let format = format(document, dictionary, key, kind, metrics_font)?;
    let prefix = if rendering && key == b"FontFile2" && format == FontFormat::TrueType {
        drawn_tables(document, object, dictionary)?
    } else {
        None
    };
    let whole = prefix.is_none();
    let data = match prefix {
        Some(data) => std::borrow::Cow::Owned(data),
        None => document.stream_data_sized(object, declared_length(document, dictionary))?,
    };
    let report = Report::new(document);
    // A prefix was checked against Length1 when it was decoded.
    if whole && let Some(problem) = length_problem(document, dictionary, key, format, data.len())? {
        // The decoded program describes its own extent; the Length entries
        // are bookkeeping that writers often get wrong (Table 125).
        report.diagnostic(Diagnostic::new(repairs::STREAM_LENGTHS, problem).in_object(id))?;
    }
    // Program-level repairs (damaged tables, glyphs and cmaps) are reported
    // with the document's, located in the font stream rather than its buffer.
    let start = Position::Stream {
        object: id,
        offset: 0,
    };
    let mut diagnostics = Diagnostics::new(document.recovery());
    // Shared storage made straight from the stream data: one copy at most.
    let result = FontProgram::parse_with_diagnostics(
        std::sync::Arc::<[u8]>::from(data),
        format,
        &document.limits(),
        &mut diagnostics,
    );
    report.diagnostics(
        diagnostics
            .take()
            .into_iter()
            .map(|diagnostic| diagnostic.within(start).in_object(id)),
    )?;
    result.map_err(|error| error.within(start))
}

/// The program length a font stream's `Length1` to `Length3` declare, as an
/// allocation hint for decoding it (0 when absent or unreadable).
fn declared_length(document: &Document, dictionary: &Dictionary) -> usize {
    let mut total = 0_usize;
    for key in [b"Length1".as_slice(), b"Length2", b"Length3"] {
        let length = optional(document, dictionary, key)
            .ok()
            .flatten()
            .and_then(Value::as_integer)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        total = total.saturating_add(length);
    }
    total.min(document.limits().decoded_bytes)
}

/// The tables that drawing glyphs from a TrueType program reads.
const DRAWN: [&[u8; 4]; 8] = [
    b"head", b"hhea", b"hmtx", b"maxp", b"loca", b"glyf", b"cmap", b"post",
];

/// A TrueType program decoded up to the end of the last table in [`DRAWN`],
/// its directory listing only the tables decoded whole
/// ([`crate::PdfFont::load_for_rendering`]).
/// `None`, for a whole decode, unless `Length1` is a positive integer within
/// the decoding budget, the stream is zlib `FlateDecode` without
/// parameters, and its table directory is one the program parser accepts
/// without repair: a TrueType version, aligned, unique, non-overlapping
/// tables after the directory and within `Length1`.
fn drawn_tables(
    document: &Document,
    object: &pagura_document::IndirectObject,
    dictionary: &Dictionary,
) -> Result<Option<Vec<u8>>> {
    let Some(length) = optional(document, dictionary, b"Length1")?
        .and_then(Value::as_integer)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0 && n <= document.limits().decoded_bytes)
    else {
        return Ok(None);
    };
    let mut enough = |data: &[u8]| directory_end(data, length);
    let Some(mut data) = document.stream_data_prefix(object, length, &mut enough)? else {
        return Ok(None);
    };
    if data.len() > length {
        return Ok(None);
    }
    // Tables that end past the prefix are left out of its directory: drawing
    // never reads them, and the program parser sees every table it reads
    // whole, as in the whole program.
    let count = usize::from(u16::from_be_bytes([data[4], data[5]]));
    let directory = 12 + 16 * count;
    let mut kept = Vec::with_capacity(count);
    for record in data[12..directory].as_chunks::<16>().0 {
        let word = |at: usize| {
            u32::from_be_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]])
        };
        let end = u64::from(word(8)) + u64::from(word(12));
        if usize::try_from(end).is_ok_and(|end| end <= data.len()) {
            kept.push(*record);
        }
    }
    data[12..directory].fill(0);
    data[12..12 + 16 * kept.len()].copy_from_slice(kept.as_flattened());
    let kept = u16::try_from(kept.len()).map_err(|_| Error::limit("font table count"))?;
    data[4..6].copy_from_slice(&kept.to_be_bytes());
    Ok(Some(data))
}

/// Whether `data`, a decoded prefix of a program of `length` bytes, holds
/// its table directory and every table in [`DRAWN`].
fn directory_end(data: &[u8], length: usize) -> Prefix {
    let Some(head) = data.get(..12) else {
        return Prefix::More;
    };
    if !matches!(head[..4], [0, 1, 0, 0] | [b't', b'r', b'u', b'e']) {
        return Prefix::Abandon;
    }
    let count = usize::from(u16::from_be_bytes([head[4], head[5]]));
    let directory = 12 + 16 * count;
    if directory > length {
        return Prefix::Abandon;
    }
    let Some(records) = data.get(12..directory) else {
        return Prefix::More;
    };
    let mut needed = directory;
    let mut tables: Vec<([u8; 4], usize, usize)> = Vec::with_capacity(count);
    for record in records.as_chunks::<16>().0 {
        let word = |at: usize| {
            usize::try_from(u32::from_be_bytes([
                record[at],
                record[at + 1],
                record[at + 2],
                record[at + 3],
            ]))
            .ok()
        };
        let tag = [record[0], record[1], record[2], record[3]];
        let (Some(start), Some(size)) = (word(8), word(12)) else {
            return Prefix::Abandon;
        };
        let Some(end) = start.checked_add(size).filter(|&end| end <= length) else {
            return Prefix::Abandon;
        };
        if size != 0 && (start < directory || start % 4 != 0)
            || tables.iter().any(|&(other, ..)| other == tag)
        {
            return Prefix::Abandon;
        }
        if DRAWN.contains(&&tag) {
            needed = needed.max(end);
        }
        tables.push((tag, start, end));
    }
    tables.retain(|&(_, start, end)| start < end);
    tables.sort_unstable_by_key(|&(_, start, _)| start);
    if tables.windows(2).any(|pair| pair[0].2 > pair[1].1) {
        return Prefix::Abandon;
    }
    if data.len() >= needed {
        Prefix::Enough
    } else {
        Prefix::More
    }
}

/// Why the stream's Length entries disagree with its decoded program, if they
/// do: TrueType needs `Length1` equal to the program, Type 1 needs the three
/// segment lengths to add up to it, and CFF has none.
fn length_problem(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    format: FontFormat,
    decoded: usize,
) -> Result<Option<&'static str>> {
    if key == b"FontFile2" {
        return Ok(match length(document, dictionary, b"Length1")? {
            None => Some("TrueType font stream has no valid Length1"),
            Some(length) if length != decoded => {
                Some("TrueType Length1 disagrees with decoded size")
            }
            Some(_) => None,
        });
    }
    if key == b"FontFile" {
        let mut total = Some(0_usize);
        for key in [b"Length1".as_slice(), b"Length2", b"Length3"] {
            total = match (total, length(document, dictionary, key)?) {
                (Some(sum), Some(length)) => sum.checked_add(length),
                _ => None,
            };
        }
        return Ok(match total {
            None => Some("Type1 font stream lacks valid Length1, Length2 or Length3"),
            Some(total) if total != decoded => {
                Some("Type1 segment lengths disagree with decoded size")
            }
            Some(_) => None,
        });
    }
    let has_lengths = [b"Length1".as_slice(), b"Length2", b"Length3"]
        .iter()
        .any(|key| dictionary.contains_key(key));
    Ok((format == FontFormat::Cff && has_lengths)
        .then_some("CFF font stream has Type1 segment lengths"))
}

fn length(document: &Document, dict: &Dictionary, key: &[u8]) -> Result<Option<usize>> {
    Ok(optional(document, dict, key)?
        .and_then(Value::as_integer)
        .and_then(|v| usize::try_from(v).ok()))
}

fn format(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    kind: FontKind,
    metrics_font: &Dictionary,
) -> Result<FontFormat> {
    let type1 = matches!(kind, FontKind::Type1 | FontKind::MmType1);
    let cid0 = metrics_font.is_name(b"Subtype", b"CIDFontType0");
    let cid2 = metrics_font.is_name(b"Subtype", b"CIDFontType2");
    let result = match key {
        b"FontFile" if type1 => Some(FontFormat::Type1),
        b"FontFile2" if kind == FontKind::TrueType || cid2 => Some(FontFormat::TrueType),
        b"FontFile3" => match optional(document, dictionary, b"Subtype")?
            .and_then(Value::as_name)
            .map(pagura_core::Name::as_bytes)
        {
            Some(b"Type1C") if type1 => Some(FontFormat::Cff),
            Some(b"CIDFontType0C") if cid0 => Some(FontFormat::Cff),
            Some(b"OpenType") if type1 || kind == FontKind::TrueType || cid0 || cid2 => {
                Some(FontFormat::OpenType)
            }
            _ => None,
        },
        _ => None,
    };
    result.ok_or_else(|| Error::syntax("font program binding disagrees with PDF font subtype"))
}

#[cfg(test)]
mod prefix_tests {
    use super::{Prefix, directory_end};

    /// A TrueType directory of `(tag, offset, length)` records.
    fn directory(tables: &[(&[u8; 4], u32, u32)]) -> Vec<u8> {
        let mut data = vec![0, 1, 0, 0];
        data.extend_from_slice(&u16::try_from(tables.len()).unwrap().to_be_bytes());
        data.extend_from_slice(&[0; 6]);
        for &(tag, offset, length) in tables {
            data.extend_from_slice(tag);
            data.extend_from_slice(&[0; 4]);
            data.extend_from_slice(&offset.to_be_bytes());
            data.extend_from_slice(&length.to_be_bytes());
        }
        data
    }

    #[test]
    fn prefixes_end_after_the_drawn_tables_of_a_clean_directory() {
        // 12 + 3 * 16 = 60 directory bytes; glyf ends at 72, name at 100.
        let tables = [(b"head", 60, 4), (b"glyf", 64, 8), (b"name", 72, 28)];
        let mut data = directory(&tables);
        assert_eq!(directory_end(&data[..20], 100), Prefix::More);
        assert_eq!(directory_end(&data, 100), Prefix::More);
        data.resize(72, 0);
        assert_eq!(directory_end(&data, 100), Prefix::Enough);
        // Records the parser would repair or refuse are left to a whole decode.
        for bad in [
            [(b"head", 60, 4), (b"glyf", 64, 8), (b"name", 72, 29)],
            [(b"head", 60, 4), (b"glyf", 66, 8), (b"name", 76, 4)],
            [(b"head", 60, 8), (b"glyf", 64, 8), (b"name", 72, 4)],
            [(b"head", 56, 4), (b"glyf", 64, 8), (b"name", 72, 4)],
            [(b"head", 60, 4), (b"head", 64, 8), (b"name", 72, 4)],
        ] {
            let mut data = directory(&bad);
            data.resize(100, 0);
            assert_eq!(directory_end(&data, 100), Prefix::Abandon, "{bad:?}");
        }
        let mut otto = directory(&tables);
        otto[..4].copy_from_slice(b"OTTO");
        assert_eq!(directory_end(&otto, 100), Prefix::Abandon);
        assert_eq!(directory_end(&directory(&tables), 59), Prefix::Abandon);
    }
}
