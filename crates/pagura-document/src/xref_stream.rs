//! Cross-reference streams, ISO 32000-2 clause 7.5.8.

use crate::xref::{Location, XrefEntry, integer_offset};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Diagnostics, Dictionary, Error, Limits, ObjectId, Position, Result, Value};
use std::collections::BTreeMap;

pub(crate) fn read(
    bytes: &[u8],
    offset: usize,
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<(BTreeMap<u32, XrefEntry>, Dictionary)> {
    let mut parser =
        Parser::with_offset(bytes, offset, Origin::File, limits)?.with_recovery(diagnostics.mode());
    let number = u32::try_from(parser.unsigned()?)
        .map_err(|_| Error::syntax("xref stream number out of range"))?;
    let generation = u16::try_from(parser.unsigned()?)
        .map_err(|_| Error::syntax("xref stream generation out of range"))?;
    let id = ObjectId::new(number, generation);
    let result = read_body(&mut parser, limits, diagnostics);
    result.map_err(|error| error.at(Position::File(offset)).in_object(id))
}

fn read_body(
    parser: &mut Parser<'_>,
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<(BTreeMap<u32, XrefEntry>, Dictionary)> {
    if !parser.keyword(b"obj") {
        return Err(Error::syntax("expected xref stream object header"));
    }
    let Value::Dictionary(dictionary) = parser.parse()? else {
        return Err(Error::syntax("xref stream must be a dictionary"));
    };
    if !dictionary.is_name(b"Type", b"XRef") {
        return Err(Error::syntax("cross-reference stream Type must be XRef"));
    }
    if dictionary.contains_key(b"F") {
        return Err(Error::syntax("xref stream cannot be external"));
    }
    let raw = direct_stream(parser, &dictionary, limits, diagnostics)?;
    if !parser.keyword(b"endobj") {
        return Err(Error::syntax("missing xref stream endobj"));
    }
    diagnostics.absorb(parser.take_diagnostics())?;
    let chain = pagura_codec::FilterChain::from_entries(
        dictionary.get(b"Filter"),
        dictionary.get(b"DecodeParms"),
    )?;
    if chain
        .filters()
        .iter()
        .any(|f| matches!(f, pagura_codec::Filter::Crypt(_)))
    {
        return Err(Error::syntax("xref stream cannot use a Crypt filter"));
    }
    let data = chain.decode(raw, &limits, diagnostics)?;
    let entries = entries(&data, &dictionary, limits)?;
    if entries
        .values()
        .any(|entry| matches!(entry.location, Location::Offset(at) if at >= parser.data().len()))
    {
        return Err(Error::syntax("object offset outside the file"));
    }
    Ok((entries, dictionary))
}

/// Bootstrap streams require direct `/Length` (7.5.8.2).
pub(crate) fn direct_stream<'a>(
    parser: &mut Parser<'a>,
    dictionary: &Dictionary,
    limits: Limits,
    diagnostics: &mut Diagnostics,
) -> Result<&'a [u8]> {
    let length =
        integer_offset(dictionary, b"Length")?.ok_or(Error::syntax("stream Length is missing"))?;
    if length > limits.stream_bytes {
        return Err(Error::limit("stream size budget exceeded"));
    }
    if !parser.keyword(b"stream") {
        return Err(Error::syntax("expected stream keyword"));
    }
    let start = crate::stream::start(parser, diagnostics, None)?;
    let end = start
        .checked_add(length)
        .ok_or(Error::syntax("stream length overflow"))?;
    let data = parser
        .data()
        .get(start..end)
        .ok_or(Error::syntax("stream extends beyond the file"))?;
    parser.seek(end)?;
    if !parser.keyword(b"endstream") {
        return Err(Error::syntax("stream Length does not end at endstream").at(parser.position()));
    }
    Ok(data)
}

fn entries(
    data: &[u8],
    dictionary: &Dictionary,
    limits: Limits,
) -> Result<BTreeMap<u32, XrefEntry>> {
    let size = integer_offset(dictionary, b"Size")?
        .filter(|&n| n > 0)
        .ok_or(Error::syntax("xref stream Size must be positive"))?;
    let widths = dictionary
        .get(b"W")
        .and_then(Value::as_array)
        .filter(|values| values.len() == 3)
        .ok_or(Error::syntax("xref stream W must have three widths"))?;
    let widths = widths
        .iter()
        .map(|value| {
            value
                .as_integer()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(Error::syntax("xref field width must be nonnegative"))
        })
        .collect::<Result<Vec<_>>>()?;
    let row = widths.iter().try_fold(0_usize, |sum, &width| {
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
    for pair in index.as_chunks::<2>().0 {
        if pair[0].checked_add(pair[1]).is_none_or(|end| end > size) {
            return Err(Error::syntax("xref Index exceeds Size"));
        }
        total = total
            .checked_add(pair[1])
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
    for pair in index.as_chunks::<2>().0 {
        for number in pair[0]..pair[0] + pair[1] {
            let number = u32::try_from(number)
                .map_err(|_| Error::syntax("xref object number out of range"))?;
            let kind = field(&mut data, widths[0], 1)?;
            let second = field(&mut data, widths[1], 0)?;
            let third = field(&mut data, widths[2], 0)?;
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
        0 => (
            u16::try_from(third).map_err(|_| Error::syntax("free generation out of range"))?,
            Location::Free,
        ),
        1 if number != 0 => (
            u16::try_from(third).map_err(|_| Error::syntax("object generation out of range"))?,
            Location::Offset(
                usize::try_from(second).map_err(|_| Error::syntax("object offset out of range"))?,
            ),
        ),
        1 => return Err(Error::syntax("object 0 cannot be in use")),
        2 if number != 0 => (
            0,
            Location::Compressed {
                stream: u32::try_from(second)
                    .map_err(|_| Error::syntax("object stream number out of range"))?,
                index: u32::try_from(third)
                    .map_err(|_| Error::syntax("object stream index out of range"))?,
            },
        ),
        2 => return Err(Error::syntax("object 0 cannot be compressed")),
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
    let bytes = data
        .get(..width)
        .ok_or(Error::syntax("truncated xref field"))?;
    // Saturation preserves the distinction between the known types 0/1/2
    // and any arbitrarily wide unknown type. Unused fields of unknown types
    // are immaterial (7.5.8.3). Known generations and object numbers undergo
    // checked narrowing; saturated offsets cannot lie within a bounded file.
    let value = bytes.iter().fold(0_u64, |value, &byte| {
        value.saturating_mul(256).saturating_add(u64::from(byte))
    });
    *data = data
        .get(width..)
        .ok_or(Error::syntax("truncated xref field"))?;
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
    fn all_zero_widths_use_defaults_without_requiring_encoded_rows() {
        let dictionary = dictionary(b"<< /Size 2 /W [0 0 0] /Index [1 1] >>");
        let table = entries(b"", &dictionary, Limits::default()).unwrap();
        assert_eq!(table.get(&1).unwrap().location, Location::Offset(0));
        assert_eq!(table.get(&1).unwrap().generation, 0);
    }

    #[test]
    fn arbitrarily_wide_unknown_types_ignore_their_other_fields() {
        let dictionary = dictionary(b"<< /Size 8 /W [9 9 9] /Index [7 1] >>");
        let table = entries(&[255; 27], &dictionary, Limits::default()).unwrap();
        assert_eq!(table.get(&7).unwrap().location, Location::Free);
    }

    #[test]
    fn overlapping_subsections_and_negative_widths_fail() {
        for bytes in [
            b"<< /Size 2 /W [0 0 0] /Index [1 1 1 1] >>".as_slice(),
            b"<< /Size 2 /W [-1 0 0] >>",
        ] {
            assert_eq!(
                entries(b"", &dictionary(bytes), Limits::default())
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
        }
    }
}
