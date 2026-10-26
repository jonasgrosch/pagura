//! Recovery of object boundaries, stream extents and offsets (7.3.8.1, 7.3.10, 7.5.4).

use crate::source::{NeedData, ReadFault, ReadResult};
use crate::{Document, Location, repairs};
use pagura_core::syntax::{
    ModeResult, Origin, ParseFault, ParseMode, Parser, WindowInput, is_whitespace,
};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, ErrorKind, Limits, ObjectId, Position, Result,
    Value,
};
use std::ops::Range;

const ENDOBJ: &[u8] = b"endobj";
const ENDSTREAM: &[u8] = b"endstream";
/// How far past `start + Length` a terminator scan reads: a CRLF before
/// `endstream`, then the keyword itself.
pub(crate) const TERMINATOR_SLACK: usize = 2 + ENDSTREAM.len();

/// Where a stream's dictionary sits, which decides what may follow its
/// `endstream` (7.3.8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamContext {
    /// An indirect object: `endstream` ends a line and `endobj` follows.
    Indirect,
    /// A repaired stream inside a dictionary or array: the containing value
    /// continues after `endstream`.
    Direct,
}

/// A complete value can end before a following object/xref token (7.3.10).
/// Do not infer closure from EOF or skip arbitrary intervening tokens.
pub(crate) fn end<M: ParseMode>(
    parser: &mut Parser<'_, M>,
    limits: &Limits,
    id: ObjectId,
    diagnostics: &mut Diagnostics,
) -> ModeResult<M, ()> {
    if parser.keyword(ENDOBJ) {
        return Ok(());
    }
    // A missing separator is provable when endobj is immediately followed by
    // the next complete object header; EOF or arbitrary regular text is not.
    let at = parser.offset();
    let after = at.saturating_add(ENDOBJ.len());
    if parser
        .data()
        .get(at..)
        .is_some_and(|tail| tail.starts_with(ENDOBJ))
        && header(parser.data(), after, limits).is_some()
    {
        diagnostics.repair(
            Diagnostic::new(
                repairs::OBJECT_SEPARATOR,
                "missing separator after endobj before the next object header",
            )
            .at(parser.position())
            .in_object(id),
        )?;
        return parser.seek(after);
    }
    let mut probe = Parser::with_offset(parser.data(), parser.offset(), Origin::File, *limits)?;
    if header(parser.data(), parser.offset(), limits).is_some()
        || probe.keyword(b"xref")
        || probe.keyword(b"trailer")
    {
        return Ok(diagnostics.repair(
            Diagnostic::new(
                repairs::OBJECT_END,
                "complete indirect value has no endobj before the next object or \
                 cross-reference section",
            )
            .at(parser.position())
            .in_object(id),
        )?);
    }
    Err((Error::syntax("missing endobj")
        .at(parser.position())
        .in_object(id))
    .into())
}

/// Recognize a positive indirect-object identity at a token boundary (7.3.10).
pub(crate) fn header(bytes: &[u8], offset: usize, limits: &Limits) -> Option<ObjectId> {
    if !bytes.get(offset).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut parser = Parser::with_offset(bytes, offset, Origin::File, *limits).ok()?;
    let number = u32::try_from(parser.unsigned().ok()?).ok()?;
    let generation = u16::try_from(parser.unsigned().ok()?).ok()?;
    (number != 0 && parser.keyword(b"obj")).then_some(ObjectId::new(number, generation))
}

/// Locates stream data after the `stream` keyword (clause 7.3.8.1).
pub(super) fn stream_extent(
    document: &Document,
    id: ObjectId,
    dictionary: &Dictionary,
    parser: &mut Parser<'_, WindowInput<NeedData>>,
    path: &mut Vec<ObjectId>,
    repairs: &mut Diagnostics,
    context: StreamContext,
) -> std::result::Result<Range<usize>, ParseFault<NeedData>> {
    let start = crate::stream::start(parser, repairs, Some(id))?;
    let length = stream_length(document, id, dictionary, path).map_err(ParseFault::from)?;
    let bytes = parser.data();
    let end = length.and_then(|length| start.checked_add(length).filter(|&end| end <= bytes.len()));
    if let Some(end) = end {
        parser.seek(end).map_err(|fault| {
            <WindowInput<NeedData> as ParseMode>::map_error(fault, |error| error.in_object(id))
        })?;
        if parser.keyword(ENDSTREAM) {
            return Ok(start..end);
        }
        // A wrong length cannot swallow another complete physical object.
        if bytes
            .get(start..end)
            .is_some_and(|data| data.windows(ENDOBJ.len()).any(|part| part == ENDOBJ))
        {
            return Err((Error::syntax("stream Length crosses an object boundary")
                .at(parser.position())
                .in_object(id))
            .into());
        }
    }
    let (marker, actual) = terminator(document, id, start, context)?;
    repairs.repair(
        Diagnostic::new(
            repairs::LENGTH_CORRECTED,
            "unusable stream Length corrected to the unambiguous endstream terminator",
        )
        .at(Position::File(end.unwrap_or(start)))
        .in_object(id),
    )?;
    parser.seek(marker)?;
    if !parser.keyword(ENDSTREAM) {
        return Err((Error::syntax("missing endstream").in_object(id)).into());
    }
    Ok(start..actual)
}

/// The declared `/Length`, or `None` when it is unusable and the terminator
/// decides the extent. Resource limits, cycles, strict-mode repairs of the
/// Length object and a compressed object-stream Length stay errors.
fn stream_length(
    document: &Document,
    id: ObjectId,
    dictionary: &Dictionary,
    path: &mut Vec<ObjectId>,
) -> ReadResult<Option<usize>> {
    let Some(mut value) = dictionary.get(b"Length") else {
        return Ok(None);
    };
    // ObjStm Length is explicitly excluded from compressed objects (7.5.7).
    let object_stream =
        dictionary.is_name(b"Type", b"ObjStm") || document.object_streams.contains_key(&id.number);
    let mut visited = Vec::new();
    let length = loop {
        match value {
            Value::Integer(length) => break *length,
            Value::Reference(reference) => {
                if visited.contains(reference) {
                    return Err((Error::cycle("indirect stream Length cycle")
                        .in_object(*reference))
                    .into());
                }
                if visited.len() >= document.limits().depth {
                    return Err((Error::limit("indirect stream Length depth exceeded")
                        .in_object(*reference))
                    .into());
                }
                if object_stream
                    && document
                        .slots
                        .get(reference)
                        .is_some_and(|slot| matches!(slot.location, Location::Compressed { .. }))
                {
                    return Err((Error::syntax("object stream Length cannot be compressed")
                        .in_object(id))
                    .into());
                }
                visited.push(*reference);
                value = match document.object_inner(*reference, path) {
                    Ok(object) => object.value(),
                    Err(ReadFault::Invalid(error))
                        if error.repair.is_none()
                            && matches!(
                                error.kind,
                                ErrorKind::Syntax | ErrorKind::MissingObject
                            ) =>
                    {
                        return Ok(None);
                    }
                    Err(error) => return Err(error),
                };
            }
            _ => return Ok(None),
        }
    };
    let Ok(length) = usize::try_from(length) else {
        return Ok(None);
    };
    if length > document.limits().stream_bytes {
        return Err((Error::limit("stream size budget exceeded").in_object(id)).into());
    }
    Ok(Some(length))
}

/// Finds the terminator before the next object or revision boundary.
fn terminator(
    document: &Document,
    id: ObjectId,
    start: usize,
    context: StreamContext,
) -> Result<(usize, usize)> {
    let bytes = document.original_bytes();
    let boundaries = document.boundaries()?;
    let boundary = boundaries
        .get(boundaries.partition_point(|&at| at <= start))
        .copied()
        .unwrap_or(bytes.len());
    terminator_in(bytes, &document.limits(), id, start..boundary, context)
}

/// Every offset where a physical object or a revision's cross-reference data
/// starts, sorted, for [`terminator`].
pub(crate) fn boundaries(document: &Document) -> Vec<usize> {
    let mut boundaries: Vec<usize> = document
        .slots
        .values()
        .filter_map(|slot| match slot.location {
            Location::Offset(at) => Some(at),
            _ => None,
        })
        .chain(document.revisions().iter().map(crate::Revision::offset))
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

/// Finds the one `endstream` that can end data starting at `range.start`,
/// before `range.end`. Returns the keyword's offset and the end of the data.
pub(crate) fn terminator_in(
    bytes: &[u8],
    limits: &Limits,
    id: ObjectId,
    range: Range<usize>,
    context: StreamContext,
) -> Result<(usize, usize)> {
    let start = range.start;
    let scan_end = start
        .saturating_add(limits.stream_bytes)
        .saturating_add(TERMINATOR_SLACK)
        .min(range.end)
        .min(bytes.len());
    let candidates = bytes
        .get(start..scan_end)
        .ok_or(Error::syntax("invalid stream bounds"))?;
    let mut found = None;
    for (relative, _) in candidates
        .windows(ENDSTREAM.len())
        .enumerate()
        .filter(|(_, part)| *part == ENDSTREAM)
    {
        let marker = start + relative;
        let separated = marker == start
            || bytes.get(marker - 1).is_some_and(|&byte| match context {
                StreamContext::Direct => is_whitespace(byte),
                StreamContext::Indirect => matches!(byte, b'\n' | b'\r'),
            });
        if !separated {
            continue;
        }
        let mut probe = Parser::with_offset(bytes, marker, Origin::File, *limits)?;
        if !probe.keyword(ENDSTREAM) {
            continue;
        }
        let closed = match context {
            StreamContext::Direct => {
                probe.peek();
                direct_continuation(&mut probe)
            }
            StreamContext::Indirect => probe.keyword(ENDOBJ),
        };
        if !closed {
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
            .at(Position::File(start))
            .in_object(id)
    })
}

fn direct_continuation(parser: &mut Parser<'_>) -> bool {
    matches!(
        parser.peek(),
        Some(b'/' | b']' | b'[' | b'(' | b'<' | b'+' | b'-' | b'.' | b'0'..=b'9')
    ) || parser
        .data()
        .get(parser.offset()..)
        .is_some_and(|rest| rest.starts_with(b">>"))
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
    diagnostics: &mut Diagnostics,
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
    let mut found = None;
    for at in
        crate::xref::nearby_tokens(document.original_bytes(), offset).filter(|&at| matches(at))
    {
        if found.is_some() {
            return Err(Error::syntax("ambiguous nearby object headers").in_object(id));
        }
        found = Some(at);
    }
    if found.is_none() && zero_catalog_header(document, id, offset) {
        return Ok(offset);
    }
    let corrected = found.ok_or_else(|| {
        Error::syntax("cross-reference entry disagrees with object header")
            .at(Position::File(offset))
            .in_object(id)
    })?;
    diagnostics.repair(
        Diagnostic::new(
            repairs::OFFSET_CORRECTED,
            "cross-reference offset corrected to a nearby matching object header",
        )
        .at(Position::File(offset))
        .in_object(id),
    )?;
    Ok(corrected)
}

#[cfg(test)]
mod tests;
