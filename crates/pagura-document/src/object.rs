//! Indirect objects (ISO 32000-2 clause 7.3.10) and stream extents.

pub(crate) mod recovery;
pub(crate) mod stream;

use recovery::{StreamContext, corrected_offset, end, stream_extent, zero_catalog_header};

use crate::Document;
use crate::memory::path_bytes;
use crate::source::{NeedData, ReadFault, ReadResult};
use pagura_core::syntax::{InputEnd, ParseFault, Parser, WindowInput, WindowRead};
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Position, Result, Value};
use std::ops::Range;

/// Where an indirect object's definition is stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ObjectSource {
    /// `N G obj … endobj` at this byte range of the file, or the complete
    /// dictionary/payload range of a repaired direct stream.
    File(Range<usize>),
    /// Inside the object stream `stream`, at position `index` (7.5.7).
    #[non_exhaustive]
    Compressed {
        /// Object number of the containing object stream.
        stream: u32,
        /// Zero-based index within the object stream.
        index: u32,
        /// Byte offset of the object's value in the decoded stream data.
        offset: usize,
    },
}

impl ObjectSource {
    /// Where the definition starts: its `N G obj` header in the file, or its
    /// value in the decoded data of the containing object stream.
    pub const fn position(&self) -> Position {
        match self {
            Self::File(range) => Position::File(range.start),
            Self::Compressed { stream, offset, .. } => Position::Stream {
                object: ObjectId::new(*stream, 0),
                offset: *offset,
            },
        }
    }
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
                sum.saturating_add(path_bytes(&direct.path, direct.path.capacity()))
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
) -> ReadResult<(IndirectObject, Vec<Diagnostic>)> {
    let mut repairs = pagura_core::Diagnostics::new(document.recovery());
    let offset = corrected_offset(document, id, offset, &mut repairs)?;
    let parser = document
        .file_parser(offset)
        .map_err(|error| error.in_object(id))?;
    match parser.read_window_with_control(InputEnd::Complete, |parser| {
        parse(document, id, offset, path, parser, &mut repairs)
    })? {
        WindowRead::Complete(parsed) => Ok(parsed.value),
        WindowRead::Deferred(missing) => Err(ReadFault::Unavailable(missing)),
        _ => Err(Error::syntax("incomplete indirect object at file end")
            .in_object(id)
            .into()),
    }
}

fn parse(
    document: &Document,
    id: ObjectId,
    offset: usize,
    path: &mut Vec<ObjectId>,
    parser: &mut Parser<'_, WindowInput<NeedData>>,
    repairs: &mut pagura_core::Diagnostics,
) -> std::result::Result<(IndirectObject, Vec<Diagnostic>), ParseFault<NeedData>> {
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
                .at(Position::File(offset))
                .in_object(id)
                .into(),
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
                .in_object(id)
                .into());
            }
            if !parser.keyword(b"stream") {
                return Err(Error::syntax("missing direct stream keyword")
                    .in_object(id)
                    .into());
            }
            stream_extent(
                document,
                id,
                dictionary,
                parser,
                path,
                repairs,
                StreamContext::Direct,
            )
            .map(Some)
        })
        .map_err(|fault| {
            <WindowInput<NeedData> as pagura_core::syntax::ParseMode>::map_error(fault, |error| {
                error.in_object(id)
            })
        })?;
    let stream = if parser.keyword(b"stream") {
        Some(stream_extent(
            document,
            id,
            value
                .as_dictionary()
                .ok_or_else(|| Error::syntax("stream value must be a dictionary").in_object(id))?,
            parser,
            path,
            repairs,
            StreamContext::Indirect,
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
                    .at(Position::File(offset))
                    .in_object(id)
                    .into(),
            );
        }
        repairs.repair(
            Diagnostic::new(
                crate::repairs::CATALOG_OBJECT_NUMBER,
                "zero-numbered Catalog uses its positive trailer Root and exact xref identity",
            )
            .at(Position::File(offset))
            .in_object(id),
        )?;
    } else {
        end(parser, &document.limits(), id, repairs)?;
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
        retained = retained
            .saturating_add(path_bytes(&direct.path, direct.path.capacity()))
            .saturating_add(
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
            assert_eq!(
                error.as_error().unwrap().repair,
                Some(DIRECT_STREAM),
                "{name}"
            );
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
        crate::tests::document(
            "direct-stream-array",
            Recovery::default(),
            Limits {
                document_bytes: bytes,
                ..Limits::default()
            },
        )
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
        assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit);
        assert_eq!(error.as_error().unwrap().object, Some(ObjectId::new(3, 0)));
        assert_eq!(limited.retained.used(), initial);
        assert_eq!(limited.object(ObjectId::new(3, 0)).unwrap_err(), error);
    }
}
