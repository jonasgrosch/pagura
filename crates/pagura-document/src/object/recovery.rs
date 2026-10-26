//! Recovery of object boundaries, stream extents and offsets (7.3.8.1, 7.3.10, 7.5.4).

use crate::source::{ByteRange, NeedData, ReadFault, ReadResult};
use crate::{Document, Location, repairs};
use pagura_core::convert::usize_to_u64;
use pagura_core::syntax::{Origin, ParseFault, ParseMode, Parser, WindowInput, is_whitespace};
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
    id: ObjectId,
    diagnostics: &mut Diagnostics,
) -> std::result::Result<(), M::Fault> {
    if parser.keyword_with_boundary(ENDOBJ)? {
        return Ok(());
    }
    let at = parser.offset();
    let position = parser.position();
    let after = at.saturating_add(ENDOBJ.len());
    if parser
        .data()
        .get(at..)
        .is_some_and(|tail| tail.starts_with(ENDOBJ))
    {
        parser.seek(after)?;
        let following = following_header(parser)?;
        parser.seek(at)?;
        if following {
            diagnostics.repair(
                Diagnostic::new(
                    repairs::OBJECT_SEPARATOR,
                    "missing separator after endobj before the next object header",
                )
                .at(position)
                .in_object(id),
            )?;
            return parser.seek(after);
        }
    }
    let following = following_header(parser)?
        || parser.keyword_with_boundary(b"xref")?
        || parser.keyword_with_boundary(b"trailer")?;
    parser.seek(at)?;
    if following {
        return Ok(diagnostics.repair(
            Diagnostic::new(
                repairs::OBJECT_END,
                "complete indirect value has no endobj before the next object or \
                 cross-reference section",
            )
            .at(position)
            .in_object(id),
        )?);
    }
    Err(Error::syntax("missing endobj")
        .at(position)
        .in_object(id)
        .into())
}

/// Speculative next-object lookahead uses this parser's actual input boundary.
/// An unfinished header cannot justify either a repair or a terminal refusal.
fn following_header<M: ParseMode>(
    parser: &mut Parser<'_, M>,
) -> std::result::Result<bool, M::Fault> {
    let at = parser.offset();
    if !parser.peek().is_some_and(|byte| byte.is_ascii_digit()) {
        parser.seek(at)?;
        return Ok(false);
    }
    let result = (|| {
        let number = parser.unsigned()?;
        let generation = parser.unsigned()?;
        Ok(number != 0
            && u32::try_from(number).is_ok()
            && u16::try_from(generation).is_ok()
            && parser.keyword_with_boundary(b"obj")?)
    })();
    parser.seek(at)?;
    result
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
    origin: usize,
) -> std::result::Result<Range<usize>, ParseFault<NeedData>> {
    let start = crate::stream::start(parser, repairs, Some(id))?;
    let length = stream_length(document, id, dictionary, path).map_err(ParseFault::from)?;
    let end = length.and_then(|length| start.checked_add(length));
    if let Some(end) = end.filter(|&end| end <= document.source_len().saturating_sub(origin)) {
        parser.has_bytes(end)?;
        parser.seek(end).map_err(|fault| {
            <WindowInput<NeedData> as ParseMode>::map_error(fault, |error| error.in_object(id))
        })?;
        if parser.keyword_with_boundary(ENDSTREAM)? {
            return Ok(start..end);
        }
        // A wrong length cannot swallow another complete physical object.
        if parser
            .data()
            .get(start..end)
            .is_some_and(|data| data.windows(ENDOBJ.len()).any(|part| part == ENDOBJ))
        {
            return Err(Error::syntax("stream Length crosses an object boundary")
                .at(parser.position())
                .in_object(id)
                .into());
        }
    }
    let absolute = origin
        .checked_add(start)
        .ok_or(Error::limit("stream source offset overflow"))?;
    let (marker, actual) = terminator(document, id, absolute, StreamContext::Direct)?;
    let marker = marker - origin;
    let actual = actual - origin;
    repairs.repair(
        Diagnostic::new(
            repairs::LENGTH_CORRECTED,
            "unusable stream Length corrected to the unambiguous endstream terminator",
        )
        .at(Position::File(origin.saturating_add(end.unwrap_or(start))))
        .in_object(id),
    )?;
    parser.has_bytes(marker.saturating_add(ENDSTREAM.len()))?;
    parser.seek(marker)?;
    if !parser.keyword_with_boundary(ENDSTREAM)? {
        return Err(Error::syntax("missing endstream").in_object(id).into());
    }
    Ok(start..actual)
}

/// A normal stream's payload is not a parsing dependency. Validate its tail
/// at the declared absolute offset; only malformed length recovery scans data.
pub(super) fn indirect_stream(
    document: &Document,
    id: ObjectId,
    dictionary: &Dictionary,
    start: usize,
    path: &mut Vec<ObjectId>,
    diagnostics: &mut Diagnostics,
) -> ReadResult<(Range<usize>, usize)> {
    let length = stream_length(document, id, dictionary, path)?;
    let declared = length
        .and_then(|length| start.checked_add(length))
        .filter(|&end| end <= document.source_len());
    if let Some(end) = declared {
        if let Some(tail) = stream_tail(document, id, end, diagnostics)? {
            return Ok((start..end, tail));
        }
        let lease = document
            .bytes
            .scan(ByteRange::new(usize_to_u64(start), usize_to_u64(end))?)?;
        if lease
            .bytes()
            .windows(ENDOBJ.len())
            .any(|part| part == ENDOBJ)
        {
            return Err(Error::syntax("stream Length crosses an object boundary")
                .at(Position::File(end))
                .in_object(id)
                .into());
        }
    }
    let (marker, actual) = terminator(document, id, start, StreamContext::Indirect)?;
    diagnostics.repair(
        Diagnostic::new(
            repairs::LENGTH_CORRECTED,
            "unusable stream Length corrected to the unambiguous endstream terminator",
        )
        .at(Position::File(declared.unwrap_or(start)))
        .in_object(id),
    )?;
    let tail = stream_tail(document, id, marker, diagnostics)?
        .ok_or_else(|| Error::syntax("missing endstream").in_object(id))?;
    Ok((start..actual, tail))
}

fn stream_tail(
    document: &Document,
    id: ObjectId,
    offset: usize,
    diagnostics: &mut Diagnostics,
) -> ReadResult<Option<usize>> {
    let mut window = document
        .bytes
        .window(offset)?
        .with_recovery(document.recovery());
    let parsed = window.read(|parser| {
        if !parser.keyword_with_boundary(ENDSTREAM)? {
            return Ok(None);
        }
        let mut attempt = Diagnostics::new(document.recovery());
        end(parser, id, &mut attempt)?;
        Ok(Some(attempt.take()))
    })?;
    let Some(reports) = parsed.value else {
        return Ok(None);
    };
    diagnostics.absorb(reports)?;
    diagnostics.absorb(parsed.diagnostics)?;
    Ok(Some(usize::try_from(window.offset()).map_err(|_| {
        Error::limit("object source offset overflow")
    })?))
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
                    return Err(Error::cycle("indirect stream Length cycle")
                        .in_object(*reference)
                        .into());
                }
                if visited.len() >= document.limits().depth {
                    return Err(Error::limit("indirect stream Length depth exceeded")
                        .in_object(*reference)
                        .into());
                }
                if object_stream
                    && document
                        .slots
                        .get(reference)
                        .is_some_and(|slot| matches!(slot.location, Location::Compressed { .. }))
                {
                    return Err(Error::syntax("object stream Length cannot be compressed")
                        .in_object(id)
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
        return Err(Error::limit("stream size budget exceeded")
            .in_object(id)
            .into());
    }
    Ok(Some(length))
}

/// Finds the terminator before the next object or revision boundary.
fn terminator(
    document: &Document,
    id: ObjectId,
    start: usize,
    context: StreamContext,
) -> ReadResult<(usize, usize)> {
    let boundaries = document.boundaries()?;
    let boundary = boundaries
        .get(boundaries.partition_point(|&at| at <= start))
        .copied()
        .unwrap_or(document.source_len());
    let end = start
        .saturating_add(document.limits().stream_bytes)
        .saturating_add(TERMINATOR_SLACK)
        .min(boundary)
        .min(document.source_len());
    let lease = document
        .bytes
        .scan(ByteRange::new(usize_to_u64(start), usize_to_u64(end))?)?;
    let (marker, actual) = terminator_in(
        lease.bytes(),
        &document.limits(),
        id,
        0..lease.bytes().len(),
        context,
        start,
    )?;
    Ok((start + marker, start + actual))
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
    origin: usize,
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
        let mut probe = Parser::with_offset(bytes, marker, Origin::File, *limits)?
            .with_origin_offset(origin)?;
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
            .at(Position::File(origin.saturating_add(start)))
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
fn zero_catalog_header(document: &Document, id: ObjectId, offset: usize) -> ReadResult<bool> {
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
        return Ok(false);
    }
    matches_header(document, ObjectId::new(0, id.generation), offset)
}

fn matches_header(document: &Document, id: ObjectId, offset: usize) -> ReadResult<bool> {
    if offset >= document.source_len() {
        return Ok(false);
    }
    let mut window = document.bytes.window(offset)?;
    match window.read(|parser| {
        parser.has_bytes(1)?;
        if !parser.data().first().is_some_and(u8::is_ascii_digit) {
            return Ok(false);
        }
        Ok(parser.unsigned()? == u64::from(id.number)
            && parser.unsigned()? == u64::from(id.generation)
            && parser.keyword_with_boundary(b"obj")?)
    }) {
        Ok(parsed) => Ok(parsed.value),
        Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Syntax => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn corrected_offset(
    document: &Document,
    id: ObjectId,
    offset: usize,
    diagnostics: &mut Diagnostics,
) -> ReadResult<(usize, ObjectId)> {
    if matches_header(document, id, offset)? {
        return Ok((offset, id));
    }
    let start = offset.saturating_sub(crate::xref::OFFSET_SLACK + 1);
    let end = offset
        .saturating_add(crate::xref::OFFSET_SLACK + 1)
        .min(document.source_len());
    if start > end {
        return Err(
            Error::syntax("cross-reference entry disagrees with object header")
                .at(Position::File(offset))
                .in_object(id)
                .into(),
        );
    }
    let mut window = document.bytes.window(start)?;
    let candidates = window
        .read(|parser| {
            parser.has_bytes(end - start)?;
            Ok(crate::xref::nearby_tokens(parser.data(), offset - start)
                .map(|at| start + at)
                .collect::<Vec<_>>())
        })?
        .value;
    let mut found = None;
    for at in candidates {
        if !matches_header(document, id, at)? {
            continue;
        }
        if found.is_some() {
            return Err(Error::syntax("ambiguous nearby object headers")
                .in_object(id)
                .into());
        }
        found = Some(at);
    }
    if found.is_none() && zero_catalog_header(document, id, offset)? {
        return Ok((offset, ObjectId::new(0, id.generation)));
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
    Ok((corrected, id))
}

#[cfg(test)]
mod tests;
