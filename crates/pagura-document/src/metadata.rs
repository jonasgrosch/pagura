//! Immutable metadata views (ISO 32000-2, 14.3 and 7.7.2).
//!
//! Info strings use the shared PDF text-string decoder and reported repairs.
//! Metadata packets remain bytes: these APIs do not parse XML, resolve entities,
//! validate an XMP schema or claim conformance to a PDF profile. Info and XMP
//! remain separate sources; no value is silently selected over the other.
//! Traversal and owned Info strings use the document's view budgets. Decoding a
//! metadata stream uses the ordinary stream/decryption limits and performs no IO.

use crate::view::{Context, Source};
use crate::{Document, IndirectObject};
use pagura_core::{Dictionary, Error, Name, ObjectId, Result, Value};
use std::borrow::Cow;

/// The Info dictionary's trapping declaration (14.3.3, Table 349).
/// This records what the producer declared, not an inspection of page marks.
/// The standard closes this set to these three names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Trapped {
    /// Fully trapped, according to the producer.
    True,
    /// Not yet trapped, according to the producer.
    False,
    /// Unknown, partially trapped, or no declaration (the PDF default).
    #[default]
    Unknown,
}

/// The standard Info entries, decoded without changing the source (Table 349).
/// Fields other than dates are deprecated in PDF 2.0 in favour of XMP.
/// Unknown keys remain accessible in [`Self::dictionary`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DocumentInfo<'a> {
    /// Indirect Info dictionary identity, if it has one.
    pub object: Option<ObjectId>,
    /// Unmodified source dictionary, including custom entries.
    pub dictionary: &'a Dictionary,
    /// Document title.
    pub title: Option<String>,
    /// Producer-declared author.
    pub author: Option<String>,
    /// Subject or description.
    pub subject: Option<String>,
    /// Producer-declared keywords; no separator or tokenization is inferred.
    pub keywords: Option<String>,
    /// Application that created the source document.
    pub creator: Option<String>,
    /// Application that produced the PDF.
    pub producer: Option<String>,
    /// Original date string, decoded but not normalized or calendar-validated.
    pub creation_date: Option<String>,
    /// Original modification-date string, with the same policy as creation date.
    pub modification_date: Option<String>,
    /// Trapping declaration; absence uses [`Trapped::Unknown`].
    pub trapped: Trapped,
}

/// A validated PDF metadata-stream handle (14.3.2, Table 347).
/// Its payload is not asserted to be valid XML or XMP.
#[derive(Debug, Clone, Copy)]
pub struct MetadataStream<'a> {
    document: &'a Document,
    object: &'a IndirectObject,
}

impl<'a> MetadataStream<'a> {
    /// Stream identity in this immutable document revision.
    pub fn object(&self) -> ObjectId {
        self.object.id()
    }

    /// Original source object, including its dictionary and encoded range.
    pub fn source(&self) -> &'a IndirectObject {
        self.object
    }

    /// Decoded packet bytes, preserving encoding, whitespace and packet wrappers.
    /// XML processing and external-entity resolution are never performed.
    ///
    /// # Errors
    /// As [`Document::stream_data`], including stream budgets, unsupported
    /// filters, external-data refusal and required authorization.
    pub fn data(&self) -> Result<Cow<'a, [u8]>> {
        self.document.stream_data(self.object)
    }
}

impl Document {
    /// Reads standard Info entries from the effective trailer (14.3.3).
    /// Absent/null Info returns `None`; an empty dictionary remains present.
    /// Does not merge Info with XMP, parse date semantics or enforce the
    /// conditional ModDate requirement for PieceInfo.
    ///
    /// # Errors
    /// Positioned type, cycle, text-decoding or view-budget errors. Text repairs
    /// follow the document's recovery mode and are reported in its diagnostics.
    pub fn information(&self) -> Result<Option<DocumentInfo<'_>>> {
        let mut context = Context::new(self);
        let Some(value) = context.present(self.trailer(), b"Info", Source::default())? else {
            return Ok(None);
        };
        let (dictionary, source) = context.dictionary(value, Source::default())?;
        context.allocate::<DocumentInfo<'_>>(1, source)?;
        let trapped = match context.entry(dictionary, b"Trapped", source)? {
            None => Trapped::Unknown,
            Some(value) => match value.as_name().map(Name::as_bytes) {
                Some(b"True") => Trapped::True,
                Some(b"False") => Trapped::False,
                Some(b"Unknown") => Trapped::Unknown,
                _ => return Err(source.error(Error::syntax("invalid Info Trapped name"))),
            },
        };
        Ok(Some(DocumentInfo {
            object: source.object,
            dictionary,
            title: text(&mut context, dictionary, b"Title", source)?,
            author: text(&mut context, dictionary, b"Author", source)?,
            subject: text(&mut context, dictionary, b"Subject", source)?,
            keywords: text(&mut context, dictionary, b"Keywords", source)?,
            creator: text(&mut context, dictionary, b"Creator", source)?,
            producer: text(&mut context, dictionary, b"Producer", source)?,
            creation_date: text(&mut context, dictionary, b"CreationDate", source)?,
            modification_date: text(&mut context, dictionary, b"ModDate", source)?,
            trapped,
        }))
    }

    /// The catalog's Lang text string (7.7.2, 14.9.2), without language-tag
    /// validation or inference from page text or XMP.
    ///
    /// # Errors
    /// Positioned catalog, type, cycle, text-decoding or view-budget errors.
    pub fn language(&self) -> Result<Option<String>> {
        let mut context = Context::new(self);
        let (catalog, source) = context.catalog()?;
        text(&mut context, catalog, b"Lang", source)
    }

    /// The document-level metadata stream attached to the catalog (14.3.2).
    /// Returns `None` when Metadata is absent/null. The payload is decoded
    /// lazily by [`MetadataStream::data`].
    ///
    /// # Errors
    /// Positioned catalog, type, reference/cycle or view-budget errors.
    pub fn metadata(&self) -> Result<Option<MetadataStream<'_>>> {
        let mut context = Context::new(self);
        let (catalog, source) = context.catalog()?;
        metadata(&mut context, catalog, source)
    }

    /// Metadata attached to an object dictionary (14.3.2, Table 348).
    /// References in `dictionary` are interpreted in this document. As with
    /// [`Self::metadata`], returns an inert, lazily decoded stream handle.
    ///
    /// # Errors
    /// Positioned metadata type, reference/cycle or view-budget errors.
    pub fn object_metadata<'a>(
        &'a self,
        dictionary: &'a Dictionary,
    ) -> Result<Option<MetadataStream<'a>>> {
        metadata(&mut Context::new(self), dictionary, Source::default())
    }
}

fn text<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<String>> {
    let Some(value) = dictionary.get(key) else {
        return Ok(None);
    };
    let (value, source) = context.resolve(value, source)?;
    if value.is_null() {
        return Ok(None);
    }
    let bytes = value
        .as_string()
        .ok_or_else(|| source.error(Error::syntax("metadata entry must be a text string")))?;
    context.text(bytes, source).map(Some)
}

fn metadata<'a>(
    context: &mut Context<'a>,
    owner: &'a Dictionary,
    source: Source,
) -> Result<Option<MetadataStream<'a>>> {
    let Some(value) = owner.get(b"Metadata") else {
        return Ok(None);
    };
    let (resolved, location) = context.resolve(value, source)?;
    if resolved.is_null() {
        return Ok(None);
    }
    let id = location
        .object
        .filter(|_| matches!(value, Value::Reference(_)))
        .ok_or_else(|| location.error(Error::syntax("Metadata must reference a stream")))?;
    let object = context.document.object(id)?;
    let dictionary = object
        .dictionary()
        .filter(|_| object.is_stream())
        .ok_or_else(|| location.error(Error::syntax("Metadata must reference a stream")))?;
    for (key, expected) in [
        (b"Type".as_slice(), b"Metadata".as_slice()),
        (b"Subtype", b"XML"),
    ] {
        if !context
            .entry(dictionary, key, location)?
            .is_some_and(|value| {
                value
                    .as_name()
                    .is_some_and(|name| name.as_bytes() == expected)
            })
        {
            return Err(location.error(Error::syntax("invalid metadata stream Type or Subtype")));
        }
    }
    context.allocate::<MetadataStream<'_>>(1, location)?;
    Ok(Some(MetadataStream {
        document: context.document,
        object,
    }))
}

/// Owner of a metadata stream. Page selection uses an exact zero-based ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetadataOwner {
    /// The catalog's Metadata entry.
    Document,
    /// The selected page's Metadata entry.
    Page(usize),
    /// The selected object dictionary's Metadata entry.
    Object(ObjectId),
}

impl Document {
    /// Reads a metadata stream from an explicitly selected owner (14.3.2).
    /// The packet remains lazy, inert bytes; Info/XMP reconciliation is not implied.
    ///
    /// # Errors
    /// As [`Self::metadata`], including absent page/object identities and wrong owner types.
    pub fn metadata_for(&self, owner: MetadataOwner) -> Result<Option<MetadataStream<'_>>> {
        match owner {
            MetadataOwner::Document => self.metadata(),
            MetadataOwner::Page(index) => self.object_metadata(self.page(index)?.dictionary()),
            MetadataOwner::Object(id) => {
                let object = self.object(id)?;
                let dictionary = object.dictionary().ok_or_else(|| {
                    Error::syntax("metadata owner must be a dictionary").in_object(id)
                })?;
                self.object_metadata(dictionary)
            }
        }
    }
}
