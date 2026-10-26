//! Cross-reference streams, ISO 32000-2 clause 7.5.8.

use super::{Location, RevisionKind, Section, XrefEntry, integer_offset};
use pagura_codec::{Filter, FilterChain};
use pagura_core::syntax::{Origin, ParseMode, Parser};
use pagura_core::{Diagnostics, Dictionary, Error, Limits, ObjectId, Result, Value};
use std::collections::BTreeMap;

/// Entry types of Table 18.
const FREE: u64 = 0;
const IN_USE: u64 = 1;
const COMPRESSED: u64 = 2;

pub(crate) fn read(
    bytes: &[u8],
    offset: usize,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Section> {
    let mut parser = Parser::with_offset(bytes, offset, Origin::File, *limits)?
        .with_recovery(diagnostics.mode());
    parse(&mut parser, bytes.len(), limits, diagnostics)
}

pub(super) fn parse<M: ParseMode>(
    parser: &mut Parser<'_, M>,
    file_length: usize,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> std::result::Result<Section, M::Fault> {
    let position = parser.position();
    let number = u32::try_from(parser.unsigned()?)
        .map_err(|_| Error::syntax("xref stream number out of range"))?;
    let generation = u16::try_from(parser.unsigned()?)
        .map_err(|_| Error::syntax("xref stream generation out of range"))?;
    let id = ObjectId::new(number, generation);
    read_body(parser, file_length, limits, diagnostics)
        .map_err(|fault| M::map_error(fault, |error| error.at(position).in_object(id)))
}

fn read_body<M: ParseMode>(
    parser: &mut Parser<'_, M>,
    file_length: usize,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> std::result::Result<Section, M::Fault> {
    if !parser.keyword_with_boundary(b"obj")? {
        return Err(Error::syntax("expected xref stream object header").into());
    }
    let Value::Dictionary(dictionary) = parser.parse()? else {
        return Err(Error::syntax("xref stream must be a dictionary").into());
    };
    if !dictionary.is_name(b"Type", b"XRef") {
        return Err(Error::syntax("cross-reference stream Type must be XRef").into());
    }
    if dictionary.contains_key(b"F") {
        return Err(Error::syntax("xref stream cannot be external").into());
    }
    let raw = bootstrap_stream_data(parser, &dictionary, limits, diagnostics)?;
    if !parser.keyword_with_boundary(b"endobj")? {
        return Err(Error::syntax("missing xref stream endobj").into());
    }
    let end = parser.offset();
    diagnostics.absorb(parser.take_diagnostics())?;
    let chain =
        FilterChain::from_entries(dictionary.get(b"Filter"), dictionary.get(b"DecodeParms"))?;
    if chain
        .filters()
        .iter()
        .any(|filter| matches!(filter, Filter::Crypt(_)))
    {
        return Err(Error::syntax("xref stream cannot use a Crypt filter").into());
    }
    let data = chain.decode(raw, limits.decoded_bytes, diagnostics)?;
    let entries = entries(&data, &dictionary, limits)?;
    if entries
        .values()
        .any(|entry| matches!(entry.location, Location::Offset(at) if at >= file_length))
    {
        return Err(Error::syntax("object offset outside the file").into());
    }
    Ok(Section {
        entries,
        trailer: dictionary,
        kind: RevisionKind::Stream,
        end,
    })
}

/// The data of a stream read before any object can be resolved, such as a
/// cross-reference stream, which requires a direct `/Length` (7.5.8.2).
pub(crate) fn bootstrap_stream_data<'a, M: ParseMode>(
    parser: &mut Parser<'a, M>,
    dictionary: &Dictionary,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> std::result::Result<&'a [u8], M::Fault> {
    let length =
        integer_offset(dictionary, b"Length")?.ok_or(Error::syntax("stream Length is missing"))?;
    if length > limits.stream_bytes {
        return Err(Error::limit("stream size budget exceeded").into());
    }
    if !parser.keyword_with_boundary(b"stream")? {
        return Err(Error::syntax("expected stream keyword").into());
    }
    let start = crate::stream::start(parser, diagnostics, None)?;
    let end = start
        .checked_add(length)
        .ok_or(Error::syntax("stream length overflow"))?;
    if !parser.has_bytes(end)? {
        return Err(Error::syntax("stream extends beyond the file").into());
    }
    let data = parser
        .data()
        .get(start..end)
        .ok_or(Error::syntax("stream extends beyond the file"))?;
    parser.seek(end)?;
    if !parser.keyword_with_boundary(b"endstream")? {
        return Err(Error::syntax("stream Length does not end at endstream")
            .at(parser.position())
            .into());
    }
    Ok(data)
}

fn entries(
    data: &[u8],
    dictionary: &Dictionary,
    limits: &Limits,
) -> Result<BTreeMap<u32, XrefEntry>> {
    let size = integer_offset(dictionary, b"Size")?
        .filter(|&n| n > 0)
        .ok_or(Error::syntax("xref stream Size must be positive"))?;
    let [kind_width, second_width, third_width] = dictionary
        .get(b"W")
        .and_then(Value::as_array)
        .and_then(|values| <&[Value; 3]>::try_from(values).ok())
        .ok_or(Error::syntax("xref stream W must have three widths"))?
        .each_ref()
        .map(|value| {
            value
                .as_integer()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(Error::syntax("xref field width must be nonnegative"))
        });
    let (kind_width, second_width, third_width) = (kind_width?, second_width?, third_width?);
    // Every entry type has a second field without a default (Table 18), so a
    // zero width there leaves no entry representable: each row would be
    // empty and any Index would fit an empty stream.
    if second_width == 0 {
        return Err(Error::syntax(
            "xref stream W gives the second field no width",
        ));
    }
    let row = [kind_width, second_width, third_width]
        .into_iter()
        .try_fold(0_usize, |sum, width| {
            sum.checked_add(width)
                .ok_or(Error::limit("xref row width overflow"))
        })?;
    let index = match dictionary.get(b"Index") {
        None => vec![0, size],
        Some(Value::Array(values)) if values.len() % 2 == 0 => values
            .iter()
            .map(|value| {
                value
                    .as_integer()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or(Error::syntax(
                        "xref Index must contain nonnegative integers",
                    ))
            })
            .collect::<Result<_>>()?,
        _ => return Err(Error::syntax("xref Index must contain subsection pairs")),
    };
    let mut total = 0_usize;
    for &[first, count] in index.as_chunks::<2>().0 {
        if first.checked_add(count).is_none_or(|end| end > size) {
            return Err(Error::syntax("xref Index exceeds Size"));
        }
        total = total
            .checked_add(count)
            .filter(|&n| n <= limits.objects)
            .ok_or(Error::limit("cross-reference entry budget exceeded"))?;
    }
    if row.checked_mul(total) != Some(data.len()) {
        return Err(Error::syntax(
            "xref stream length disagrees with W and Index",
        ));
    }
    let mut data = data;
    let mut result = BTreeMap::new();
    for &[first, count] in index.as_chunks::<2>().0 {
        for number in first..first + count {
            let number = u32::try_from(number)
                .map_err(|_| Error::syntax("xref object number out of range"))?;
            let kind = field(&mut data, kind_width, IN_USE)?;
            let second = field(&mut data, second_width, 0)?;
            let third = field(&mut data, third_width, 0)?;
            let entry = entry(number, kind, second, third)?;
            if result.insert(number, entry).is_some() {
                return Err(Error::syntax("xref Index subsections overlap"));
            }
        }
    }
    Ok(result)
}

fn entry(number: u32, kind: u64, second: u64, third: u64) -> Result<XrefEntry> {
    let (generation, location) = match kind {
        FREE => (
            u16::try_from(third).map_err(|_| Error::syntax("free generation out of range"))?,
            Location::Free,
        ),
        IN_USE if number != 0 => (
            u16::try_from(third).map_err(|_| Error::syntax("object generation out of range"))?,
            Location::Offset(
                usize::try_from(second).map_err(|_| Error::syntax("object offset out of range"))?,
            ),
        ),
        IN_USE => return Err(Error::syntax("object 0 cannot be in use")),
        COMPRESSED if number != 0 => (
            0,
            Location::Compressed {
                stream: u32::try_from(second)
                    .map_err(|_| Error::syntax("object stream number out of range"))?,
                index: u32::try_from(third)
                    .map_err(|_| Error::syntax("object stream index out of range"))?,
            },
        ),
        COMPRESSED => return Err(Error::syntax("object 0 cannot be compressed")),
        _ => (0, Location::Free), // Unknown types resolve to null, 7.5.8.3.
    };
    Ok(XrefEntry {
        generation,
        location,
    })
}

fn field(data: &mut &[u8], width: usize, default: u64) -> Result<u64> {
    if width == 0 {
        return Ok(default);
    }
    let (bytes, rest) = data
        .split_at_checked(width)
        .ok_or(Error::syntax("truncated xref field"))?;
    // Saturation preserves the distinction between the known types 0/1/2
    // and any arbitrarily wide unknown type. Unused fields of unknown types
    // are immaterial (7.5.8.3). Known generations and object numbers undergo
    // checked narrowing; saturated offsets cannot lie within a bounded file.
    let value = bytes.iter().fold(0_u64, |value, &byte| {
        value.saturating_mul(256).saturating_add(u64::from(byte))
    });
    *data = rest;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::entries;
    use crate::Location;
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Dictionary, ErrorKind, Limits};

    fn dictionary(bytes: &[u8]) -> Dictionary {
        Parser::new(bytes, Origin::Buffer, Limits::default())
            .parse()
            .unwrap()
            .as_dictionary()
            .unwrap()
            .clone()
    }

    #[test]
    fn zero_width_rows_are_rejected_before_any_entry_exists() {
        // W [0 0 0] would let an empty payload stand for every entry of Index.
        for widths in ["[0 0 0]", "[1 0 2]"] {
            let dictionary = dictionary(
                format!("<< /Size 4000001 /W {widths} /Index [1 4000000] >>").as_bytes(),
            );
            let error = entries(b"", &dictionary, &Limits::default()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{widths}");
            assert_eq!(
                error.message,
                "xref stream W gives the second field no width"
            );
        }
        // A zero type width still defaults the type to 1 (7.5.8.2).
        let dictionary = dictionary(b"<< /Size 2 /W [0 1 0] /Index [1 1] >>");
        let table = entries(&[9], &dictionary, &Limits::default()).unwrap();
        assert_eq!(table.get(&1).unwrap().location, Location::Offset(9));
        assert_eq!(table.get(&1).unwrap().generation, 0);
    }

    #[test]
    fn arbitrarily_wide_unknown_types_ignore_their_other_fields() {
        let dictionary = dictionary(b"<< /Size 8 /W [9 9 9] /Index [7 1] >>");
        let table = entries(&[255; 27], &dictionary, &Limits::default()).unwrap();
        assert_eq!(table.get(&7).unwrap().location, Location::Free);
    }

    #[test]
    fn overlapping_subsections_and_negative_widths_fail() {
        let overlap = dictionary(b"<< /Size 2 /W [1 1 0] /Index [1 1 1 1] >>");
        let error = entries(&[1, 9, 1, 9], &overlap, &Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.message, "xref Index subsections overlap");
        let negative = dictionary(b"<< /Size 2 /W [-1 1 0] >>");
        assert_eq!(
            entries(b"", &negative, &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}
