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
    pub(crate) fn retained_bytes(&self) -> usize {
        self.value
            .retained_bytes()
            .saturating_sub(size_of::<Value>())
    }

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
    end(&mut parser, document.limits(), id, &mut repairs)?;
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
    let mut probe = pagura_core::syntax::Parser::at(
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
    let mut parser =
        pagura_core::syntax::Parser::at(bytes, offset, pagura_core::syntax::Origin::File, limits)
            .ok()?;
    let number = u32::try_from(parser.unsigned().ok()?).ok()?;
    let generation = u16::try_from(parser.unsigned().ok()?).ok()?;
    (number != 0 && parser.keyword(b"obj")).then_some(ObjectId::new(number, generation))
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
    // ObjStm Length is explicitly excluded from compressed objects (7.5.7).
    // It must never become a recoverable scalar-resolution failure.
    if dictionary.is_name(b"Type", b"ObjStm")
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
    let (marker, actual) = terminator(document, id, start)?;
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

#[cfg(test)]
mod tests {
    use super::end;
    use crate::{Document, OpenOptions, tags};
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Diagnostics, ErrorKind, Limits, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn missing_object_terminators_require_a_following_structural_token() {
        let limits = Limits::default();
        for next in [b"2 0 obj".as_slice(), b"xref", b"trailer << >>"] {
            let bytes = [b"null\n".as_slice(), next].concat();
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let mut parser = Parser::new(&bytes, Origin::File, limits);
                parser.parse().unwrap();
                let mut diagnostics = Diagnostics::new(recovery);
                let result = end(&mut parser, limits, ObjectId::new(1, 0), &mut diagnostics);
                if recovery == Recovery::Strict {
                    let error = result.unwrap_err();
                    assert_eq!(error.repair, Some(tags::OBJECT_END));
                    assert_eq!(error.object, Some(ObjectId::new(1, 0)));
                } else {
                    result.unwrap();
                    assert_eq!(&bytes[parser.offset()..], next);
                    assert_eq!(diagnostics.as_slice()[0].repair, tags::OBJECT_END);
                }
            }
        }
        for bytes in [
            b"null".as_slice(),
            b"null\nunknown",
            b"null\nendobject",
            b"null\n0 0 obj",
        ] {
            let mut parser = Parser::new(bytes, Origin::File, limits);
            parser.parse().unwrap();
            let mut diagnostics = Diagnostics::new(Recovery::Repair);
            assert_eq!(
                end(&mut parser, limits, ObjectId::new(1, 0), &mut diagnostics)
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
            assert!(diagnostics.is_empty());
        }
    }

    #[test]
    fn ambiguous_stream_boundaries_are_not_repaired() {
        let mut bytes = fixture("document", "stream-length-missing.pdf");
        let start = bytes
            .windows(8)
            .position(|part| part == b"\nstream\n")
            .unwrap()
            + 8;
        bytes.splice(start..start, b"\nendstream\nendobj\n".iter().copied());
        let document = Document::open(bytes, &OpenOptions::default()).unwrap();
        let error = document.object(ObjectId::new(4, 0)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.message, "ambiguous stream terminators");
    }

    #[test]
    fn unusable_lengths_are_repaired_lazily_and_keep_exact_bytes() {
        for name in [
            "stream-length-missing",
            "stream-length-negative",
            "stream-length-undefined",
            "stream-length-outside",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            let strict = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let error = strict.object(ObjectId::new(4, 0)).unwrap_err();
            assert_eq!(error.repair, Some(tags::LENGTH_CORRECTED), "{name}");
            let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
            let object = document.object(ObjectId::new(4, 0)).unwrap();
            assert_eq!(
                document.stream_data(object).unwrap().as_ref(),
                b"1 0 0 rg 8 8 48 32 re f\n",
                "{name}"
            );
            assert_eq!(document.original_bytes(), bytes);
            assert_eq!(
                document
                    .diagnostics()
                    .iter()
                    .filter(|item| item.repair == tags::LENGTH_CORRECTED)
                    .count(),
                1
            );
        }
        let bytes = fixture("document", "stream-length-outside.pdf");
        let document = Document::open(
            bytes,
            &OpenOptions {
                limits: Limits {
                    stream_bytes: 32,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            document.object(ObjectId::new(4, 0)).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}

#[cfg(test)]
mod length_dependencies {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn cyclic_and_deep_lengths_remain_structured_errors_in_both_modes() {
        for (name, expected) in [
            ("length-self-cycle", ErrorKind::Cycle),
            ("length-mutual-cycle", ErrorKind::Cycle),
            ("length-chain-depth", ErrorKind::Limit),
        ] {
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let document = Document::open(
                    fixture("document", &format!("{name}.pdf")),
                    &OpenOptions {
                        recovery,
                        ..OpenOptions::default()
                    },
                )
                .unwrap();
                assert_eq!(document.page_count().unwrap(), 1);
                let error = document
                    .content_streams(document.page(0).unwrap())
                    .unwrap_err();
                assert_eq!(error.kind, expected);
                let number = if expected == ErrorKind::Limit { 69 } else { 4 };
                assert_eq!(error.object, Some(ObjectId::new(number, 0)));
            }
        }
    }
}
