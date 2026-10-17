#![forbid(unsafe_code)]
//! Immutable PDF documents: cross-reference data, lazily parsed and cached
//! indirect objects, decoded streams and the typed page tree.
//!
//! A [`Document`] never modifies its source bytes. Objects are parsed on
//! first access and cached for the document's lifetime; the document is
//! `Send + Sync`, so pages can be processed in parallel. Repairs of
//! malformed input follow ADR 0003 and are reported by
//! [`Document::diagnostics`].

mod catalog;
mod diagnostics;
mod load;
mod memory;
mod object;
mod object_stream;
mod pages;
mod rebuild;
mod security;
mod security_repair;
mod stream;
pub mod tags;
mod xref;
mod xref_stream;

pub use object::{IndirectObject, ObjectSource};
pub use pages::{Page, Rotation};
pub use pagura_codec::ImageCodec;
pub use stream::ImageStream;
pub use xref::{Location, Revision, RevisionKind, XrefEntry};

use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, ObjectId, Recovery, Result, Value,
};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// How to open a document.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Resource budgets for every operation on the document.
    pub limits: Limits,
    /// Whether malformed input may be repaired (reported) or is an error.
    pub recovery: Recovery,
    /// Password for encrypted documents; empty tries the empty password.
    pub password: Vec<u8>,
}

impl std::fmt::Debug for OpenOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenOptions")
            .field("limits", &self.limits)
            .field("recovery", &self.recovery)
            .field("password", &"[redacted]")
            .finish()
    }
}

/// An opened, immutable PDF document.
#[derive(Debug)]
pub struct Document {
    bytes: Arc<[u8]>,
    options: OpenOptions,
    trailer: Dictionary,
    slots: BTreeMap<ObjectId, Slot>,
    pages: OnceLock<Result<Box<[Page]>>>,
    revisions: Vec<Revision>,
    object_streams: BTreeMap<u32, OnceLock<Result<object_stream::Contents>>>,
    security: Option<security::Security>,
    diagnostics: Mutex<diagnostics::Log>,
    retained: memory::Budget,
}

/// One cross-reference entry and its lazily parsed object.
#[derive(Debug)]
struct Slot {
    location: Location,
    object: OnceLock<Result<IndirectObject>>,
}

impl Document {
    /// Opens a document from its complete bytes.
    ///
    /// # Errors
    ///
    /// Returns a limit error for oversized input or retained-cache storage, syntax errors for
    /// malformed structure that recovery cannot (or may not) repair, and
    /// `Unsupported` for structures not implemented yet.
    pub fn open(bytes: impl Into<Arc<[u8]>>, options: &OpenOptions) -> Result<Self> {
        let bytes = bytes.into();
        let limits = options.limits;
        if bytes.len() > limits.file_bytes {
            return Err(Error::limit("file size budget exceeded"));
        }
        let mut diagnostics = Diagnostics::new(options.recovery);
        let table = xref::read(&bytes, limits, &mut diagnostics)?;
        let primary = table.rebuild_remaining.is_none();
        match Self::from_table(
            bytes.clone(),
            options,
            table,
            diagnostics.as_slice().to_vec(),
        ) {
            Ok(document) => Ok(document),
            Err(error)
                if primary && options.recovery == Recovery::Repair && xref::structural(&error) =>
            {
                let table = rebuild::read(&bytes, limits, &mut diagnostics, error.position)?;
                Self::from_table(bytes, options, table, diagnostics.take())
            }
            Err(error) => Err(error),
        }
    }

    /// The original, unmodified file bytes.
    pub fn original_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The resource budgets in effect.
    pub fn limits(&self) -> Limits {
        self.options.limits
    }

    /// The recovery mode in effect.
    pub fn recovery(&self) -> Recovery {
        self.options.recovery
    }

    /// The effective trailer dictionary of the newest revision.
    pub fn trailer(&self) -> &Dictionary {
        &self.trailer
    }

    /// Source revisions, newest first (ISO 32000-2 clause 7.5.6).
    pub fn revisions(&self) -> &[Revision] {
        &self.revisions
    }

    /// Number of in-use objects in the cross-reference data.
    pub fn object_count(&self) -> usize {
        self.slots.len()
    }

    /// Identities of all in-use objects, in ascending order.
    pub fn object_ids(&self) -> impl ExactSizeIterator<Item = ObjectId> + '_ {
        self.slots.keys().copied()
    }

    /// Distinct reported repairs, in discovery order. Identity is the repair tag,
    /// object and position; the first message is retained. At most 1,024 entries
    /// are retained across opening, lazy decoding and external reports.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .items
            .clone()
    }

    /// Number of reports omitted after the 1,024-entry diagnostic cap was reached.
    /// Repeated retained identities are deduplicated without incrementing this
    /// count. Omitted identities are not stored, so their repeated reports count
    /// again. The count saturates at `usize::MAX` (ADR 0012).
    pub fn suppressed_diagnostics(&self) -> usize {
        self.diagnostics
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .suppressed
    }

    /// Reports repairs from any document consumer, using the same bounded store
    /// as document parsing. Deduplicates by repair tag, object and position.
    ///
    /// Callers must obtain permission to repair through
    /// [`Diagnostics::repair`] with this document's [`Self::recovery`] mode
    /// before applying a repair. Reporting records evidence; it does not grant
    /// permission to repair in Strict mode. See [`Self::suppressed_diagnostics`].
    pub fn report(&self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        // Evaluate user iterators outside the lock: they may themselves access
        // the document or report a diagnostic.
        for diagnostic in diagnostics {
            self.diagnostics
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(diagnostic);
        }
    }

    /// Loads and caches an indirect object.
    ///
    /// # Errors
    ///
    /// Returns `MissingObject` if `id` is not an in-use entry, and syntax
    /// or limit errors if the object cannot be parsed or retained within
    /// [`Limits::document_bytes`].
    pub fn object(&self, id: ObjectId) -> Result<&IndirectObject> {
        self.object_inner(id, &mut Vec::new())
    }

    /// Follows references to a direct value.
    ///
    /// A reference to an undefined or free object resolves to null, as
    /// clause 7.3.10 requires.
    ///
    /// # Errors
    ///
    /// Returns errors from loading the referenced object and a limit error
    /// for reference chains deeper than [`Limits::depth`].
    pub fn resolve<'a>(&'a self, value: &'a Value) -> Result<&'a Value> {
        static NULL: Value = Value::Null;
        let mut current = value;
        for _ in 0..=self.limits().depth {
            let Value::Reference(id) = current else {
                return Ok(current);
            };
            if !self.slots.contains_key(id) {
                return Ok(&NULL);
            }
            current = self.object(*id)?.value();
        }
        Err(Error::limit("reference chain too long"))
    }

    /// Resolves `value` and requires a dictionary.
    ///
    /// # Errors
    ///
    /// As [`Document::resolve`], plus a syntax error if the result is not a
    /// dictionary.
    pub fn resolve_dictionary<'a>(&'a self, value: &'a Value) -> Result<&'a Dictionary> {
        self.resolve(value)?
            .as_dictionary()
            .ok_or(Error::syntax("expected a dictionary"))
    }

    /// The document catalog (`/Root`).
    ///
    /// # Errors
    ///
    /// Returns a syntax error if the trailer has no usable `/Root`.
    pub fn catalog(&self) -> Result<&Dictionary> {
        let root = self
            .trailer
            .get(b"Root")
            .ok_or(Error::syntax("trailer has no Root"))?;
        self.resolve_dictionary(root)
    }

    /// The encoded bytes of a stream object, exactly as stored in the file.
    ///
    /// # Errors
    ///
    /// Returns a syntax error if `object` is not a stream.
    pub fn raw_stream_data<'a>(&'a self, object: &IndirectObject) -> Result<&'a [u8]> {
        stream::raw(self, object)
    }

    /// The decoded data of a stream object after all filters.
    ///
    /// # Errors
    ///
    /// Returns `Unsupported` for external streams and unimplemented
    /// filters, a limit error if decoding would exceed
    /// [`Limits::decoded_bytes`], and syntax errors for corrupt data.
    pub fn stream_data<'a>(&'a self, object: &IndirectObject) -> Result<Cow<'a, [u8]>> {
        stream::decoded(self, object)
    }

    /// The data of an image stream with every filter except a trailing image
    /// codec applied; the codec is returned for `pagura-image` to decode.
    ///
    /// # Errors
    ///
    /// As [`Document::stream_data`]; an image codec that is not the last
    /// filter fails with its `Unsupported` tag.
    pub fn image_stream<'a>(&'a self, object: &IndirectObject) -> Result<ImageStream<'a>> {
        stream::image(self, object)
    }

    /// All pages in document order. The page tree is walked once and cached.
    ///
    /// # Errors
    ///
    /// Returns errors from interpreting the page tree, including a limit error
    /// if retaining its complete pages/resources exceeds [`Limits::document_bytes`].
    pub fn pages(&self) -> Result<&[Page]> {
        match self
            .pages
            .get_or_init(|| pages::read(self).map(Vec::into_boxed_slice))
        {
            Ok(pages) => Ok(pages),
            Err(error) => Err(error.clone()),
        }
    }

    /// Number of pages.
    ///
    /// # Errors
    ///
    /// As [`Document::pages`].
    pub fn page_count(&self) -> Result<usize> {
        self.pages().map(<[Page]>::len)
    }

    /// The page at zero-based `index`.
    ///
    /// # Errors
    ///
    /// As [`Document::pages`], plus `InvalidArgument` for an index beyond
    /// the last page.
    pub fn page(&self, index: usize) -> Result<&Page> {
        self.pages()?
            .get(index)
            .ok_or(Error::invalid_argument("page index outside document"))
    }

    /// The content stream objects of `page`, in order (clause 7.8.2). The
    /// page's content is these streams' decoded data taken as one sequence
    /// of tokens; streams may split between any two tokens.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for a malformed `/Contents` entry and errors
    /// from loading the stream objects.
    pub fn content_streams(&self, page: &Page) -> Result<Vec<&IndirectObject>> {
        let Some(contents) = page.dictionary().get(b"Contents") else {
            return Ok(Vec::new());
        };
        let invalid = || Error::syntax("Contents must be a stream or array").in_object(page.id());
        let references = match contents {
            Value::Reference(id) => {
                let object = self.object(*id)?;
                if object.stream_range().is_some() {
                    return Ok(vec![object]);
                }
                object.value().as_array().ok_or_else(invalid)?
            }
            Value::Array(values) => values,
            _ => return Err(invalid()),
        };
        references
            .iter()
            .map(|value| {
                let id = value.as_reference().ok_or_else(|| {
                    Error::syntax("Contents array must hold stream references").in_object(page.id())
                })?;
                self.object(id)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
