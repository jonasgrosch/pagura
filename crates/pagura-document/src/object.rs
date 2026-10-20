//! Indirect objects (ISO 32000-2 clause 7.3.10) and stream extents.

use crate::Document;
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Result, Value};
use std::ops::Range;

/// Where an indirect object's definition is stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ObjectSource {
    /// `N G obj … endobj` at this byte range of the file, or the complete
    /// dictionary/payload range of a repaired direct stream.
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
    direct: Vec<DirectObject>,
    direct_source: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct DirectObject {
    path: Vec<pagura_core::syntax::PathComponent>,
    object: IndirectObject,
}

impl IndirectObject {
    pub(crate) fn retained_bytes(&self) -> usize {
        let values = self
            .value
            .retained_bytes()
            .saturating_sub(size_of::<Value>());
        self.direct.iter().fold(
            values.saturating_add(
                self.direct
                    .capacity()
                    .saturating_mul(size_of::<DirectObject>()),
            ),
            |sum, direct| {
                let path = direct.path.iter().fold(
                    direct
                        .path
                        .capacity()
                        .saturating_mul(size_of::<pagura_core::syntax::PathComponent>()),
                    |sum, part| {
                        sum.saturating_add(
                            part.retained_bytes()
                                .saturating_sub(size_of::<pagura_core::syntax::PathComponent>()),
                        )
                    },
                );
                sum.saturating_add(path)
                    .saturating_add(direct.object.retained_bytes())
            },
        )
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
            direct: Vec::new(),
            direct_source: false,
        }
    }

    pub(crate) fn is_direct(&self) -> bool {
        self.direct_source
    }

    pub(crate) fn direct_stream(
        &self,
        path: &[pagura_core::syntax::PathComponent],
    ) -> Option<&Self> {
        self.direct
            .binary_search_by(|direct| direct.path.as_slice().cmp(path))
            .ok()
            .map(|index| &self.direct[index].object)
    }

    pub(crate) fn direct_objects_mut(&mut self) -> impl Iterator<Item = &mut Self> {
        self.direct.iter_mut().map(|direct| &mut direct.object)
    }

    /// The object's identity. A repaired direct stream uses its container's identity.
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
    let zero_catalog = zero_catalog_header(document, id, offset);
    let number = if zero_catalog {
        0
    } else {
        u64::from(id.number)
    };
    let header_matches = parser.unsigned().ok() == Some(number)
        && parser.unsigned().ok() == Some(u64::from(id.generation))
        && parser.keyword(b"obj");
    if !header_matches {
        return Err(
            Error::syntax("cross-reference entry disagrees with object header")
                .at(pagura_core::Position::File(offset))
                .in_object(id),
        );
    }
    let (value, direct) = parser
        .parse_with_streams(&mut |parser, dictionary| {
            if document.security.is_none()
                && document
                    .trailer()
                    .get(b"Encrypt")
                    .is_some_and(|value| !value.is_null())
            {
                return Err(Error::syntax(
                    "direct streams cannot be loaded before security initialization",
                )
                .in_object(id));
            }
            if !parser.keyword(b"stream") {
                return Err(Error::syntax("missing direct stream keyword").in_object(id));
            }
            stream_extent(document, id, dictionary, parser, path, &mut repairs, true).map(Some)
        })
        .map_err(in_object)?;
    let stream = if parser.keyword(b"stream") {
        Some(stream_extent(
            document,
            id,
            value
                .as_dictionary()
                .ok_or_else(|| Error::syntax("stream value must be a dictionary").in_object(id))?,
            &mut parser,
            path,
            &mut repairs,
            false,
        )?)
    } else {
        None
    };
    if zero_catalog {
        // Only an exact positive Root xref entry establishes the replacement
        // identity. A complete, explicitly typed plaintext Catalog is required;
        // no arbitrary object is renumbered and no encryption key is guessed.
        let catalog = value.as_dictionary().is_some_and(|dictionary| {
            dictionary.is_name(b"Type", b"Catalog")
                && dictionary
                    .get(b"Pages")
                    .and_then(Value::as_reference)
                    .is_some_and(|pages| pages.number != 0 && pages != id)
        });
        if !catalog || stream.is_some() || !direct.is_empty() || !parser.keyword(b"endobj") {
            return Err(
                Error::syntax("zero-numbered root is not a complete plain Catalog")
                    .at(pagura_core::Position::File(offset))
                    .in_object(id),
            );
        }
        repairs.repair(
            Diagnostic::new(
                crate::tags::CATALOG_OBJECT_NUMBER,
                "zero-numbered Catalog uses its positive trailer Root and exact xref identity",
            )
            .at(pagura_core::Position::File(offset))
            .in_object(id),
        )?;
    } else {
        end(&mut parser, document.limits(), id, &mut repairs)?;
    }
    let source = ObjectSource::File(offset..parser.offset());
    repairs.absorb(
        parser
            .take_diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.in_object(id)),
    )?;
    let direct = direct_objects(id, &value, direct, document.limits().document_bytes)?;
    let mut object = IndirectObject::new(id, value, stream, source);
    object.direct = direct;
    Ok((object, repairs.take()))
}

fn direct_objects(
    id: ObjectId,
    value: &Value,
    direct: Vec<pagura_core::syntax::DirectStream>,
    budget: usize,
) -> Result<Vec<DirectObject>> {
    if direct.is_empty() {
        return Ok(Vec::new());
    }
    let mut retained = value
        .retained_bytes()
        .saturating_sub(size_of::<Value>())
        .saturating_add(direct.len().saturating_mul(size_of::<DirectObject>()));
    if retained > budget {
        return Err(Error::limit("direct stream retained-memory budget exceeded").in_object(id));
    }
    let mut records = Vec::new();
    records
        .try_reserve_exact(direct.len())
        .map_err(|_| Error::limit("direct stream metadata allocation failed").in_object(id))?;
    for direct in direct {
        let mut dictionary = value;
        for part in &direct.path {
            dictionary = match part {
                pagura_core::syntax::PathComponent::Key(key) => dictionary
                    .as_dictionary()
                    .and_then(|dict| dict.get(key.as_bytes())),
                pagura_core::syntax::PathComponent::Index(index) => {
                    dictionary.as_array().and_then(|array| array.get(*index))
                }
            }
            .ok_or_else(|| Error::syntax("invalid direct stream path").in_object(id))?;
        }
        let path_bytes = direct.path.iter().fold(
            direct
                .path
                .capacity()
                .saturating_mul(size_of::<pagura_core::syntax::PathComponent>()),
            |sum, part| {
                sum.saturating_add(
                    part.retained_bytes()
                        .saturating_sub(size_of::<pagura_core::syntax::PathComponent>()),
                )
            },
        );
        retained = retained.saturating_add(path_bytes).saturating_add(
            dictionary
                .retained_bytes()
                .saturating_sub(size_of::<Value>()),
        );
        if retained > budget {
            return Err(
                Error::limit("direct stream retained-memory budget exceeded").in_object(id),
            );
        }
        let mut object = IndirectObject::new(
            id,
            dictionary.clone(),
            Some(direct.data),
            ObjectSource::File(direct.source),
        );
        object.direct_source = true;
        records.push(DirectObject {
            path: direct.path,
            object,
        });
    }
    records.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok(records)
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
fn stream_extent(
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
fn zero_catalog_header(document: &Document, id: ObjectId, offset: usize) -> bool {
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
mod tests {
    use super::end;
    use crate::{Document, OpenOptions, tags};
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Diagnostics, ErrorKind, Limits, ObjectId, Recovery};
    use pagura_testkit::fixture;

    #[test]
    fn reserved_zero_root_needs_its_exact_positive_plaintext_identity() {
        let bytes = fixture("document", "catalog-number-zero.pdf");
        let strict = Document::open(
            bytes.clone(),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(strict.repair, Some(tags::CATALOG_OBJECT_NUMBER));
        assert_eq!(strict.object, Some(ObjectId::new(1, 0)));
        let document = Document::open(bytes, &OpenOptions::default()).unwrap();
        assert_eq!(document.page_count().unwrap(), 1);
        assert_eq!(
            document.object(ObjectId::new(1, 0)).unwrap().id(),
            ObjectId::new(1, 0)
        );
        assert_eq!(
            document.object(ObjectId::new(0, 0)).unwrap_err().kind,
            ErrorKind::MissingObject
        );
        for name in [
            "catalog-number-zero-conflict",
            "catalog-number-zero-encrypted",
        ] {
            let bytes = fixture("document", &format!("{name}.pdf"));
            assert!(Document::open(bytes, &OpenOptions::default()).is_err());
        }
    }

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
                    assert_eq!(diagnostics.as_slice()[0].repair(), Some(tags::OBJECT_END));
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
                    .filter(|item| item.repair() == Some(tags::LENGTH_CORRECTED))
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

#[cfg(test)]
mod direct_tests {
    use super::DirectObject;
    use crate::{Document, OpenOptions};
    use pagura_core::repairs::DIRECT_STREAM;
    use pagura_core::syntax::PathComponent;
    use pagura_core::{ErrorKind, Limits, ObjectId, Recovery, Value};
    use pagura_testkit::fixture;

    const CONTENT: &[u8] = b"1 0 0 rg 8 8 48 32 re f\n";

    #[test]
    fn direct_streams_preserve_bytes_paths_content_order_and_encryption_identity() {
        for name in [
            "direct-stream-contents",
            "direct-stream-array",
            "direct-stream-indirect-array",
            "direct-stream-direct-page",
            "direct-stream-indirect-kids",
            "direct-stream-resources",
            "direct-stream-encrypted",
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
            let error = strict
                .pages()
                .and_then(|pages| strict.content_streams(&pages[0]))
                .unwrap_err();
            assert_eq!(error.repair, Some(DIRECT_STREAM), "{name}");
            let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
            let page = document.page(0).unwrap();
            let streams = document.content_streams(page).unwrap();
            let mut content = Vec::new();
            for stream in &streams {
                content.extend_from_slice(&document.stream_data(stream).unwrap());
            }
            assert_eq!(content, CONTENT, "{name}");
            assert_eq!(document.original_bytes(), bytes);
            if name == "direct-stream-encrypted" {
                assert_eq!(streams[0].id(), ObjectId::new(3, 0));
                assert_eq!(
                    streams[0]
                        .dictionary()
                        .unwrap()
                        .get(b"Label")
                        .unwrap()
                        .as_string(),
                    Some(b"inline".as_slice())
                );
            }
            if name == "direct-stream-indirect-kids" {
                assert_eq!(page.id(), ObjectId::new(5, 0));
            }
            let diagnostics = document.diagnostics();
            assert!(
                diagnostics
                    .iter()
                    .any(|item| item.repair() == Some(DIRECT_STREAM))
            );
            document.content_streams(page).unwrap();
            assert_eq!(document.diagnostics(), diagnostics);
        }
    }

    fn open(bytes: usize) -> Document {
        Document::open(
            fixture("document", "direct-stream-array.pdf"),
            &OpenOptions {
                limits: Limits {
                    document_bytes: bytes,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn direct_stream_metadata_and_header_copies_are_admitted_once() {
        let document = open(128 * 1024 * 1024);
        let initial = document.retained.used();
        let object = document.object(ObjectId::new(3, 0)).unwrap();
        assert_eq!(object.direct.len(), 2);
        let mut expected = object.value().retained_bytes() - size_of::<Value>()
            + object.direct.capacity() * size_of::<DirectObject>();
        for direct in &object.direct {
            assert_eq!(direct.object.direct, []);
            expected += direct.path.capacity() * size_of::<PathComponent>();
            expected += direct
                .path
                .iter()
                .map(|part| part.retained_bytes() - size_of::<PathComponent>())
                .sum::<usize>();
            expected += direct.object.value().retained_bytes() - size_of::<Value>();
        }
        assert_eq!(document.retained.used(), initial + expected);
        let exact = open(initial + expected);
        std::thread::scope(|scope| {
            let threads: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| exact.object(ObjectId::new(3, 0)).unwrap()))
                .collect();
            for thread in threads {
                thread.join().unwrap();
            }
        });
        assert_eq!(exact.retained.used(), initial + expected);
        let limited = open(initial + expected - 1);
        let error = limited.object(ObjectId::new(3, 0)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.object, Some(ObjectId::new(3, 0)));
        assert_eq!(limited.retained.used(), initial);
        assert_eq!(limited.object(ObjectId::new(3, 0)).unwrap_err(), error);
    }
}
