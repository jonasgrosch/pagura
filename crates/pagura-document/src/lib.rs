#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Immutable PDF documents: cross-reference data, lazily parsed and cached
//! indirect objects, decoded streams and the typed page tree.
//!
//! A [`Document`] never modifies its source bytes. Objects are parsed on
//! first access and cached for the document's lifetime; the document is
//! `Send + Sync`, so pages can be processed in parallel. Repairs of
//! malformed input follow ADR 0003 and are reported by
//! [`Document::diagnostics`].

mod access;
mod builder;
mod catalog;
mod diagnostics;
pub mod features;
pub mod forms;
mod history;
pub mod information;
mod load;
mod memory;
pub mod navigation;
mod object;
mod options;
mod pages;
pub mod repairs;
mod revision_bounds;
mod security;
pub mod signatures;
mod stream;
mod view;
mod xref;

pub use object::{IndirectObject, ObjectSource};
pub use options::OpenOptions;
pub use pages::Page;
pub use pagura_codec::{ImageCodec, Prefix};
pub use pagura_core::Rotation;
pub use stream::ImageStream;
pub use xref::{Revision, RevisionKind};

use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, ObjectId, Recovery, Result, Value,
};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use xref::Location;

/// An opened, immutable PDF document.
pub struct Document {
    /// Identifies this document to its pages: they hold a weak reference,
    /// which keeps the allocation, and so its address, from being reused.
    token: Arc<()>,
    bytes: history::Bytes,
    limits: Limits,
    recovery: Recovery,
    trailer: Dictionary,
    slots: BTreeMap<ObjectId, Slot>,
    revisions: Vec<Revision>,
    object_streams: BTreeMap<u32, OnceLock<Result<object::stream::Contents>>>,
    security: Option<security::Security>,
    /// Sorted offsets where physical objects and revisions start.
    boundaries: OnceLock<Result<Box<[usize]>>>,
    pages: OnceLock<Result<Box<[Page]>>>,
    diagnostics: Mutex<diagnostics::Log>,
    retained: Arc<memory::Budget>,
    historical: history::Cache,
}

// Pages are processed in parallel; keep the document shareable.
const _: fn() = || {
    fn shareable<T: Send + Sync>() {}
    shareable::<Document>();
};

impl fmt::Debug for Document {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Document")
            .field("bytes", &self.bytes.len())
            .field("objects", &self.slots.len())
            .field("revisions", &self.revisions.len())
            .field("encrypted", &self.security.is_some())
            .field("recovery", &self.recovery)
            .finish_non_exhaustive()
    }
}

/// One cross-reference entry and its lazily parsed object.
#[derive(Debug)]
struct Slot {
    location: Location,
    object: OnceLock<Result<IndirectObject>>,
}

impl Slot {
    const fn new(location: Location) -> Self {
        Self {
            location,
            object: OnceLock::new(),
        }
    }
}

impl Document {
    /// Opens a document from its complete bytes.
    ///
    /// # Errors
    ///
    /// Returns a limit error for oversized input or retained-cache storage, syntax errors for
    /// malformed structure that recovery cannot (or may not) repair, and
    /// `Unsupported` for structures not implemented yet. DocOpen authorization
    /// can return `Password`; EFOpen-only filters defer authorization until an
    /// encrypted embedded stream is requested (7.6.6). Reopen with credentials
    /// to authorize those embedded streams.
    pub fn open(bytes: impl Into<Arc<[u8]>>, options: &OpenOptions) -> Result<Self> {
        let bytes = bytes.into();
        if bytes.len() > options.limits.file_bytes {
            return Err(Error::limit("file size budget exceeded"));
        }
        builder::open(bytes, options)
    }

    /// The original, unmodified file bytes.
    pub fn original_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The resource budgets in effect.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The recovery mode in effect.
    pub fn recovery(&self) -> Recovery {
        self.recovery
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

    /// Whether the effective xref contains this exact in-use identity.
    /// Does not parse the object or follow references (7.3.10).
    pub fn contains_object(&self, id: ObjectId) -> bool {
        self.slots.contains_key(&id)
    }

    /// Identities of all in-use objects, in ascending order.
    pub fn object_ids(&self) -> impl ExactSizeIterator<Item = ObjectId> + '_ {
        self.slots.keys().copied()
    }

    /// Distinct diagnostics, in discovery order. Identity is the diagnostic kind/tag,
    /// object and position; the first message is retained. At most 1,024 repairs
    /// and 1,024 informational entries are retained across opening, lazy decoding
    /// and external reports.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .items
            .clone()
    }

    /// Number of reports omitted after a 1,024-entry diagnostic cap was reached.
    /// Repeated retained identities are deduplicated without incrementing this
    /// count. Omitted identities are not stored, so their repeated reports count
    /// again. The count saturates at `usize::MAX` (ADR 0012).
    pub fn suppressed_diagnostics(&self) -> usize {
        self.diagnostics
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .suppressed
    }

    /// Reports diagnostics from any document consumer, using the same bounded store
    /// as document parsing. Deduplicates by kind/tag, object and position.
    ///
    /// Callers must obtain permission to repair through
    /// [`Diagnostics::repair`] with this document's [`Self::recovery`] mode
    /// before applying a repair. Information needs no recovery permission.
    /// Reporting records evidence; it does not grant
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

    /// Applies a repair under this document's recovery mode (ADR 0003): in
    /// repair mode `diagnostic` is reported as for [`Self::report`]; in strict
    /// mode it is the error, and the caller must not apply the repair.
    ///
    /// # Errors
    ///
    /// Under [`Recovery::Strict`], the error of `diagnostic`'s repair.
    pub fn repair(&self, diagnostic: Diagnostic) -> Result<()> {
        let mut diagnostics = Diagnostics::new(self.recovery);
        diagnostics.repair(diagnostic)?;
        self.report(diagnostics.take());
        Ok(())
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
        self.resolve_inner(value, &mut Vec::new())
            .map(|(value, _)| value)
    }

    /// Resolves `value` and requires a dictionary.
    ///
    /// # Errors
    ///
    /// As [`Document::resolve`], plus a syntax error if the result is not a
    /// dictionary.
    pub fn resolve_dictionary<'a>(&'a self, value: &'a Value) -> Result<&'a Dictionary> {
        let (value, origin) = self.resolve_inner(value, &mut Vec::new())?;
        value
            .as_dictionary()
            .ok_or_else(|| Error::syntax("expected a dictionary").in_object_opt(origin))
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
    /// [`Limits::decoded_bytes`], syntax errors for corrupt data, and `Password`
    /// for embedded encrypted data awaiting EFOpen authorization (7.6.6).
    pub fn stream_data<'a>(&'a self, object: &IndirectObject) -> Result<Cow<'a, [u8]>> {
        stream::decoded(self, object, 0)
    }

    /// As [`Document::stream_data`], allocating for an expected decoded
    /// length of `size` bytes at once; the result and errors are the same.
    ///
    /// # Errors
    ///
    /// As [`Document::stream_data`].
    pub fn stream_data_sized<'a>(
        &'a self,
        object: &IndirectObject,
        size: usize,
    ) -> Result<Cow<'a, [u8]>> {
        stream::decoded(self, object, size)
    }

    /// The leading decoded bytes of a stream whose only filter is
    /// `FlateDecode` without parameters, decoded until `enough` accepts them
    /// ([`pagura_codec::FilterChain::decode_prefix`]). `None` when the
    /// stream is anything else or the prefix is not available; callers then
    /// use [`Document::stream_data`]. No diagnostics are reported: the bytes
    /// are those a whole decode starts with, and its later checks are not
    /// made.
    ///
    /// # Errors
    ///
    /// As [`Document::stream_data`] for preparing the stream (decryption,
    /// external streams and filter entries).
    ///
    /// `size` is the whole stream's expected decoded length, an allocation
    /// hint only.
    pub fn stream_data_prefix(
        &self,
        object: &IndirectObject,
        size: usize,
        enough: &mut dyn FnMut(&[u8]) -> Prefix,
    ) -> Result<Option<Vec<u8>>> {
        let (data, chain) = self.prepare_stream(object, &mut Vec::new())?;
        Ok(chain.decode_prefix(&data, (self.limits().decoded_bytes, size), enough))
    }

    /// The data of an image stream with every filter except a trailing image
    /// codec applied; the codec is returned for `pagura-image` to decode.
    ///
    /// # Errors
    ///
    /// As [`Document::stream_data`]; an image codec that is not the last
    /// filter fails with its `Unsupported` tag.
    pub fn image_stream<'a>(&'a self, object: &IndirectObject) -> Result<ImageStream<'a>> {
        self.image_stream_with_parameters(
            object,
            object
                .dictionary()
                .and_then(|dictionary| dictionary.get(b"DecodeParms")),
        )
    }

    /// Decodes an image stream using `parameters` in place of its `/DecodeParms`
    /// (7.3.8.1, Table 5; 7.4.1). `None` means absent parameters. References in
    /// the supplied value are resolved as for the original entry.
    ///
    /// Decryption, explicit Crypt selection (7.4.10), external-stream rules and
    /// decoding limits are the same as for [`Document::image_stream`]. The
    /// supplied parameters are not retained. Callers repairing malformed entries
    /// must authorize the repair through [`pagura_core::Diagnostics::repair`]
    /// and forward its diagnostics through [`Document::report`].
    ///
    /// # Errors
    ///
    /// As [`Document::image_stream`], including errors resolving parameters or
    /// validating their Crypt selection.
    pub fn image_stream_with_parameters<'a>(
        &'a self,
        object: &IndirectObject,
        parameters: Option<&Value>,
    ) -> Result<ImageStream<'a>> {
        stream::image(self, object, parameters, 0, false)
    }

    /// As [`Document::image_stream_with_parameters`], allocating for an
    /// expected length of `size` bytes after the non-image filters at once;
    /// the result and errors are the same.
    ///
    /// # Errors
    ///
    /// As [`Document::image_stream_with_parameters`].
    pub fn image_stream_sized<'a>(
        &'a self,
        object: &IndirectObject,
        parameters: Option<&Value>,
        size: usize,
    ) -> Result<ImageStream<'a>> {
        stream::image(self, object, parameters, size, false)
    }

    /// As [`Document::image_stream_sized`], but data that damaged filter
    /// input cut short is returned with [`ImageStream::damaged`] set and the
    /// `filter.corrupt` repair reported, instead of failing. The image methods
    /// above fail on such data, carrying that repair, so callers can try
    /// another reading of the stream first.
    ///
    /// # Errors
    ///
    /// As [`Document::image_stream_with_parameters`].
    pub fn image_stream_damaged<'a>(
        &'a self,
        object: &IndirectObject,
        parameters: Option<&Value>,
        size: usize,
    ) -> Result<ImageStream<'a>> {
        stream::image(self, object, parameters, size, true)
    }

    /// All pages in document order. The page tree is walked once and cached.
    ///
    /// # Errors
    ///
    /// Returns errors from interpreting the page tree, including a limit error
    /// if retaining its complete pages/resources exceeds [`Limits::document_bytes`].
    pub fn pages(&self) -> Result<&[Page]> {
        let pages = if let Some(pages) = self.pages.get() {
            pages
        } else {
            // Walk outside the cell: every dependency is loaded before a lock
            // is held (ADR 0012). A walk that loses a race to publish drops
            // its reservation, so only the published walk stays charged.
            let walk = pages::read(self);
            let mut published = None;
            let pages = self.pages.get_or_init(|| {
                let (pages, reservation) = walk?;
                published = Some(reservation);
                Ok(pages.into_boxed_slice())
            });
            if let Some(reservation) = published {
                reservation.commit();
            }
            pages
        };
        pages.as_deref().map_err(Clone::clone)
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
    /// from loading the stream objects. Repaired direct streams use their
    /// containing object's identity and retain individual file source ranges.
    /// Fails with `InvalidArgument` for a page of another document.
    pub fn content_streams(&self, page: &Page) -> Result<Vec<&IndirectObject>> {
        self.check_page(page)?;
        pages::contents::read(self, page)
    }

    /// Fails with `InvalidArgument` unless `page` came from this document.
    pub(crate) fn check_page(&self, page: &Page) -> Result<()> {
        if std::ptr::eq(page.document().as_ptr(), Arc::as_ptr(&self.token)) {
            Ok(())
        } else {
            Err(Error::invalid_argument("page belongs to another document"))
        }
    }

    /// Sorted offsets where physical objects and revisions start, built on
    /// first use and charged to the retained budget.
    pub(crate) fn boundaries(&self) -> Result<&[usize]> {
        let boundaries = if let Some(boundaries) = self.boundaries.get() {
            boundaries
        } else {
            let bytes = self
                .slots
                .len()
                .saturating_add(self.revisions.len())
                .saturating_mul(size_of::<usize>());
            let built = self
                .retained
                .charge(bytes, None)
                .map(|()| object::recovery::boundaries(self).into_boxed_slice());
            let charged = built.is_ok();
            let mut published = false;
            let boundaries = self.boundaries.get_or_init(|| {
                published = true;
                built
            });
            if charged && !published {
                self.retained.release(bytes);
            }
            boundaries
        };
        boundaries.as_deref().map_err(Clone::clone)
    }
}

#[cfg(test)]
mod tests;
