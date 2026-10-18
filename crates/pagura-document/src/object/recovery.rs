//! Recovery of object boundaries, stream extents and offsets (7.3.8.1, 7.3.10, 7.5.4).

use crate::Document;
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Result, Value};
use std::ops::Range;

/// A complete value can end before a following object/xref token (7.3.10).
/// Do not infer closure from EOF or skip arbitrary intervening tokens.
pub(crate) fn end(
    parser: &mut pagura_core::syntax::Parser<'_>,
    limits: pagura_core::Limits,
    id: ObjectId,
    diagnostics: &mut pagura_core::Diagnostics,
) -> Result<()> {
    if parser.keyword(b"endobj") {
        return Ok(());
    }
    // A missing separator is provable when endobj is immediately followed by
    // the next complete object header; EOF or arbitrary regular text is not.
    let at = parser.offset();
    if parser
        .data()
        .get(at..)
        .is_some_and(|tail| tail.starts_with(b"endobj"))
        && header(parser.data(), at.saturating_add(6), limits).is_some()
    {
        diagnostics.repair(
            Diagnostic::new(
                crate::tags::OBJECT_SEPARATOR,
                "missing separator after endobj before the next object header",
            )
            .at(parser.position())
            .in_object(id),
        )?;
        return parser.seek(at + 6);
    }
    let mut probe = pagura_core::syntax::Parser::with_offset(
        parser.data(),
        parser.offset(),
        pagura_core::syntax::Origin::File,
        limits,
    )?;
    if header(parser.data(), parser.offset(), limits).is_some()
        || probe.keyword(b"xref")
        || probe.keyword(b"trailer")
    {
        return diagnostics.repair(
            Diagnostic::new(crate::tags::OBJECT_END, "complete indirect value has no endobj before the next object or cross-reference section")
                .at(parser.position()).in_object(id),
        );
    }
    Err(Error::syntax("missing endobj")
        .at(parser.position())
        .in_object(id))
}

/// Recognize a positive indirect-object identity at a token boundary (7.3.10).
pub(crate) fn header(bytes: &[u8], offset: usize, limits: pagura_core::Limits) -> Option<ObjectId> {
    if !bytes.get(offset).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut parser = pagura_core::syntax::Parser::with_offset(
        bytes,
        offset,
        pagura_core::syntax::Origin::File,
        limits,
    )
    .ok()?;
    let number = u32::try_from(parser.unsigned().ok()?).ok()?;
    let generation = u16::try_from(parser.unsigned().ok()?).ok()?;
    (number != 0 && parser.keyword(b"obj")).then_some(ObjectId::new(number, generation))
}

/// Locates stream data after the `stream` keyword (clause 7.3.8.1).
pub(super) fn stream_extent(
    document: &Document,
    id: ObjectId,
    dictionary: &Dictionary,
    parser: &mut pagura_core::syntax::Parser<'_>,
    path: &mut Vec<ObjectId>,
    repairs: &mut pagura_core::Diagnostics,
    direct: bool,
) -> Result<Range<usize>> {
    // ObjStm Length is explicitly excluded from compressed objects (7.5.7).
    // It must never become a recoverable scalar-resolution failure.
    if (dictionary.is_name(b"Type", b"ObjStm") || document.object_streams.contains_key(&id.number))
        && dictionary
            .get(b"Length")
            .and_then(Value::as_reference)
            .is_some_and(|length_id| {
                document
                    .slots
                    .get(&length_id)
                    .is_some_and(|slot| matches!(slot.location, crate::Location::Compressed { .. }))
            })
    {
        return Err(Error::syntax("object stream Length cannot be compressed").in_object(id));
    }
    let start = crate::stream::start(parser, repairs, Some(id))?;
    let declared = stream_length(document, id, dictionary, path);
    let length = match declared {
        Ok(length) => Some(length),
        // A strict-mode repair needed by the Length object itself keeps its tag.
        Err(error)
            if error.repair.is_none()
                && matches!(
                    error.kind,
                    pagura_core::ErrorKind::Syntax | pagura_core::ErrorKind::MissingObject
                ) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    let bytes = parser.data();
    let end = length.and_then(|length| start.checked_add(length).filter(|&end| end <= bytes.len()));
    if let Some(end) = end {
        parser.seek(end).map_err(|error| error.in_object(id))?;
        if parser.keyword(b"endstream") {
            return Ok(start..end);
        }
        // A wrong length cannot swallow another complete physical object.
        if bytes
            .get(start..end)
            .is_some_and(|data| data.windows(6).any(|part| part == b"endobj"))
        {
            return Err(Error::syntax("stream Length crosses an object boundary")
                .at(parser.position())
                .in_object(id));
        }
    }
    let (marker, actual) = terminator(document, id, start, direct)?;
    repairs.repair(
        Diagnostic::new(
            crate::tags::LENGTH_CORRECTED,
            "unusable stream Length corrected to the unambiguous endstream terminator",
        )
        .at(pagura_core::Position::File(end.unwrap_or(start)))
        .in_object(id),
    )?;
    parser.seek(marker)?;
    if !parser.keyword(b"endstream") {
        return Err(Error::syntax("missing endstream").in_object(id));
    }
    Ok(start..actual)
}

fn stream_length(
    document: &Document,
    id: ObjectId,
    dictionary: &Dictionary,
    path: &mut Vec<ObjectId>,
) -> Result<usize> {
    let mut value = dictionary
        .get(b"Length")
        .ok_or_else(|| Error::syntax("stream Length must be an integer").in_object(id))?;
    let mut visited = Vec::new();
    let length = loop {
        match value {
            Value::Integer(length) => break *length,
            Value::Reference(reference) => {
                if visited.contains(reference) {
                    return Err(Error::cycle("indirect stream Length cycle").in_object(*reference));
                }
                if visited.len() >= document.limits().depth {
                    return Err(
                        Error::limit("indirect stream Length depth exceeded").in_object(*reference)
                    );
                }
                if dictionary.is_name(b"Type", b"ObjStm")
                    && document.slots.get(reference).is_some_and(|slot| {
                        matches!(slot.location, crate::Location::Compressed { .. })
                    })
                {
                    return Err(
                        Error::syntax("object stream Length cannot be compressed").in_object(id)
                    );
                }
                visited.push(*reference);
                value = document.object_inner(*reference, path)?.value();
            }
            _ => {
                return Err(
                    Error::syntax("indirect stream Length must be an integer").in_object(id)
                );
            }
        }
    };
    let length = usize::try_from(length)
        .map_err(|_| Error::syntax("stream Length is negative").in_object(id))?;
    if length > document.limits().stream_bytes {
        return Err(Error::limit("stream size budget exceeded").in_object(id));
    }
    Ok(length)
}

fn terminator(
    document: &Document,
    id: ObjectId,
    start: usize,
    direct: bool,
) -> Result<(usize, usize)> {
    let bytes = document.original_bytes();
    let boundary = document
        .slots
        .values()
        .filter_map(|slot| match slot.location {
            crate::Location::Offset(at) if at > start => Some(at),
            _ => None,
        })
        .chain(
            document
                .revisions()
                .iter()
                .map(crate::Revision::offset)
                .filter(|&at| at > start),
        )
        .min()
        .unwrap_or(bytes.len());
    terminator_in(bytes, document.limits(), id, start, boundary, direct)
}

pub(crate) fn terminator_in(
    bytes: &[u8],
    limits: pagura_core::Limits,
    id: ObjectId,
    start: usize,
    boundary: usize,
    direct: bool,
) -> Result<(usize, usize)> {
    let scan_end = start
        .saturating_add(limits.stream_bytes)
        .saturating_add(11)
        .min(boundary)
        .min(bytes.len());
    let candidates = bytes
        .get(start..scan_end)
        .ok_or(Error::syntax("invalid stream bounds"))?;
    let mut found = None;
    for (relative, _) in candidates
        .windows(9)
        .enumerate()
        .filter(|(_, part)| *part == b"endstream")
    {
        let marker = start + relative;
        let separated = marker == start
            || bytes.get(marker - 1).is_some_and(|&byte| {
                if direct {
                    pagura_core::syntax::is_whitespace(byte)
                } else {
                    matches!(byte, b'\n' | b'\r')
                }
            });
        if !separated {
            continue;
        }
        let mut probe = pagura_core::syntax::Parser::with_offset(
            bytes,
            marker,
            pagura_core::syntax::Origin::File,
            limits,
        )?;
        if !probe.keyword(b"endstream") {
            continue;
        }
        if direct {
            probe.peek();
            if !direct_continuation(&mut probe) {
                continue;
            }
        } else if !probe.keyword(b"endobj") {
            continue;
        }
        if found.is_some() {
            return Err(Error::syntax("ambiguous stream terminators")
                .at(probe.position())
                .in_object(id));
        }
        let mut actual = marker;
        if actual > start && bytes.get(actual - 1) == Some(&b'\n') {
            actual -= 1;
        }
        if actual > start && bytes.get(actual - 1) == Some(&b'\r') {
            actual -= 1;
        }
        if actual - start > limits.stream_bytes {
            return Err(Error::limit("stream size budget exceeded").in_object(id));
        }
        found = Some((marker, actual));
    }
    found.ok_or_else(|| {
        Error::syntax("stream Length does not end at endstream")
            .at(pagura_core::Position::File(start))
            .in_object(id)
    })
}

fn direct_continuation(parser: &mut pagura_core::syntax::Parser<'_>) -> bool {
    matches!(
        parser.peek(),
        Some(b'/' | b']' | b'[' | b'(' | b'<' | b'+' | b'-' | b'.' | b'0'..=b'9')
    ) || parser.data()[parser.offset()..].starts_with(b">>")
        || parser.keyword(b"true")
        || parser.keyword(b"false")
        || parser.keyword(b"null")
}

/// Repair only the reserved zero spelling at the exact offset assigned to a
/// positive trailer Root (7.3.10, 7.5.4, Table 15). Encrypted files cannot
/// establish whether a zero or positive identity was used for object keys.
pub(super) fn zero_catalog_header(document: &Document, id: ObjectId, offset: usize) -> bool {
    if id.number == 0
        || document
            .trailer()
            .get(b"Root")
            .and_then(Value::as_reference)
            != Some(id)
        || document
            .trailer()
            .get(b"Encrypt")
            .is_some_and(|value| !value.is_null())
    {
        return false;
    }
    let Ok(mut parser) = document.file_parser(offset) else {
        return false;
    };
    parser.unsigned().ok() == Some(0)
        && parser.unsigned().ok() == Some(u64::from(id.generation))
        && parser.keyword(b"obj")
}

pub(super) fn corrected_offset(
    document: &Document,
    id: ObjectId,
    offset: usize,
    diagnostics: &mut pagura_core::Diagnostics,
) -> Result<usize> {
    let matches = |at| -> bool {
        if !document
            .original_bytes()
            .get(at)
            .is_some_and(u8::is_ascii_digit)
        {
            return false;
        }
        let Ok(mut parser) = document.file_parser(at) else {
            return false;
        };
        parser.unsigned().ok() == Some(u64::from(id.number))
            && parser.unsigned().ok() == Some(u64::from(id.generation))
            && parser.keyword(b"obj")
    };
    if matches(offset) {
        return Ok(offset);
    }
    let bytes = document.original_bytes();
    let mut found = None;
    for at in offset.saturating_sub(32)..offset.saturating_add(33).min(bytes.len()) {
        if (at == 0
            || bytes
                .get(at - 1)
                .is_some_and(|&byte| pagura_core::syntax::is_whitespace(byte)))
            && bytes.get(at).is_some_and(u8::is_ascii_digit)
            && matches(at)
        {
            if found.is_some() {
                return Err(Error::syntax("ambiguous nearby object headers").in_object(id));
            }
            found = Some(at);
        }
    }
    if found.is_none() && zero_catalog_header(document, id, offset) {
        return Ok(offset);
    }
    let corrected = found.ok_or_else(|| {
        Error::syntax("cross-reference entry disagrees with object header")
            .at(pagura_core::Position::File(offset))
            .in_object(id)
    })?;
    diagnostics.repair(
        Diagnostic::new(
            crate::tags::OFFSET_CORRECTED,
            "cross-reference offset corrected to a nearby matching object header",
        )
        .at(pagura_core::Position::File(offset))
        .in_object(id),
    )?;
    Ok(corrected)
}

#[cfg(test)]
mod tests;
