//! Indirect objects (ISO 32000-2 clause 7.3.10) and stream extents.

use crate::Document;
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Result, Value};
use std::ops::Range;

/// Where an indirect object's definition is stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ObjectSource {
    /// `N G obj … endobj` at this byte range of the file.
    File(Range<usize>),
    /// Inside the object stream `stream`, at position `index`.
    Compressed {
        /// Object number of the containing object stream.
        stream: u32,
        /// Zero-based index within the object stream.
        index: u32,
    },
}

/// A parsed indirect object.
#[derive(Debug, Clone, PartialEq)]
pub struct IndirectObject {
    id: ObjectId,
    pub(crate) value: Value,
    stream: Option<Range<usize>>,
    source: ObjectSource,
}

impl IndirectObject {
    /// Creates an object record. Used by this crate's parsers.
    pub(crate) fn new(
        id: ObjectId,
        value: Value,
        stream: Option<Range<usize>>,
        source: ObjectSource,
    ) -> Self {
        Self {
            id,
            value,
            stream,
            source,
        }
    }

    /// The object's identity.
    pub fn id(&self) -> ObjectId {
        self.id
    }

    /// The object's value; for a stream, its dictionary.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// The value as a dictionary, if it is one (always so for streams).
    pub fn dictionary(&self) -> Option<&Dictionary> {
        self.value.as_dictionary()
    }

    /// Whether the object is a stream.
    pub fn is_stream(&self) -> bool {
        self.stream.is_some()
    }

    /// The file byte range of a stream's encoded data.
    pub fn stream_range(&self) -> Option<Range<usize>> {
        self.stream.clone()
    }

    /// Where the object is defined.
    pub fn source(&self) -> &ObjectSource {
        &self.source
    }
}

/// Parses the object `id` whose header starts at file `offset`.
pub(crate) fn parse_at(
    document: &Document,
    id: ObjectId,
    offset: usize,
    path: &mut Vec<ObjectId>,
) -> Result<(IndirectObject, Vec<Diagnostic>)> {
    let in_object = |error: Error| error.in_object(id);
    let mut repairs = pagura_core::Diagnostics::new(document.recovery());
    let offset = corrected_offset(document, id, offset, &mut repairs)?;
    let mut parser = document.file_parser(offset).map_err(in_object)?;
    let header_matches = parser.unsigned().ok() == Some(u64::from(id.number))
        && parser.unsigned().ok() == Some(u64::from(id.generation))
        && parser.keyword(b"obj");
    if !header_matches {
        return Err(
            Error::syntax("cross-reference entry disagrees with object header")
                .at(pagura_core::Position::File(offset))
                .in_object(id),
        );
    }
    let value = parser.parse().map_err(in_object)?;
    let stream = if parser.keyword(b"stream") {
        Some(stream_extent(
            document,
            id,
            &value,
            &mut parser,
            path,
            &mut repairs,
        )?)
    } else {
        None
    };
    if !parser.keyword(b"endobj") {
        return Err(Error::syntax("missing endobj")
            .at(parser.position())
            .in_object(id));
    }
    let source = ObjectSource::File(offset..parser.offset());
    repairs.extend(
        parser
            .take_diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.in_object(id))
            .collect::<Vec<_>>(),
    );
    Ok((
        IndirectObject::new(id, value, stream, source),
        repairs.take(),
    ))
}

/// Locates stream data after the `stream` keyword (clause 7.3.8.1).
fn stream_extent(
    document: &Document,
    id: ObjectId,
    value: &Value,
    parser: &mut pagura_core::syntax::Parser<'_>,
    path: &mut Vec<ObjectId>,
    repairs: &mut pagura_core::Diagnostics,
) -> Result<Range<usize>> {
    let dictionary = value
        .as_dictionary()
        .ok_or_else(|| Error::syntax("stream value must be a dictionary").in_object(id))?;
    let length = match dictionary.get(b"Length") {
        Some(Value::Integer(length)) => *length,
        Some(Value::Reference(length_id)) => {
            if dictionary.is_name(b"Type", b"ObjStm")
                && document
                    .slots
                    .get(length_id)
                    .is_some_and(|slot| matches!(slot.location, crate::Location::Compressed { .. }))
            {
                return Err(
                    Error::syntax("object stream Length cannot be compressed").in_object(id)
                );
            }
            document
                .object_inner(*length_id, path)?
                .value()
                .as_integer()
                .ok_or_else(|| {
                    Error::syntax("indirect stream Length must be an integer").in_object(id)
                })?
        }
        _ => return Err(Error::syntax("stream Length must be an integer").in_object(id)),
    };
    let length = usize::try_from(length)
        .map_err(|_| Error::syntax("stream Length is negative").in_object(id))?;
    if length > document.limits().stream_bytes {
        return Err(Error::limit("stream size budget exceeded").in_object(id));
    }
    let bytes = parser.data();
    let start = crate::stream::start(parser, repairs, Some(id))?;
    let end = start
        .checked_add(length)
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| Error::syntax("stream extends beyond the file").in_object(id))?;
    parser.seek(end).map_err(|error| error.in_object(id))?;
    if parser.keyword(b"endstream") {
        return Ok(start..end);
    }
    // Reject lengths spanning an object boundary: recovery must not consume
    // the next object's bytes, even when a later terminator looks plausible.
    if bytes
        .get(start..end)
        .is_some_and(|data| data.windows(6).any(|part| part == b"endobj"))
    {
        return Err(Error::syntax("stream Length crosses an object boundary")
            .at(parser.position())
            .in_object(id));
    }
    let (marker, actual) = terminator(document, id, start)?;
    repairs.repair(
        Diagnostic::new(
            crate::tags::LENGTH_CORRECTED,
            "stream Length corrected to the unambiguous endstream terminator",
        )
        .at(pagura_core::Position::File(end))
        .in_object(id),
    )?;
    parser.seek(marker)?;
    if !parser.keyword(b"endstream") {
        return Err(Error::syntax("missing endstream").in_object(id));
    }
    Ok(start..actual)
}

fn terminator(document: &Document, id: ObjectId, start: usize) -> Result<(usize, usize)> {
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
    let scan_end = start
        .saturating_add(document.limits().stream_bytes)
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
        if marker != start && !matches!(bytes.get(marker - 1), Some(b'\n' | b'\r')) {
            continue;
        }
        let mut probe = document.file_parser(marker)?;
        if !probe.keyword(b"endstream") || !probe.keyword(b"endobj") {
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
        if actual - start > document.limits().stream_bytes {
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

fn corrected_offset(
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
