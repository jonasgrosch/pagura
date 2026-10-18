//! Document-bound rendering resources (ADR 0009).
use crate::{DisplayList, Pixmap, RenderOptions, Tile, interpreter};
use pagura_core::Result;
use pagura_document::{Document, Page};
use pagura_text::Provider;
use std::sync::{Arc, Mutex};

/// Immutable document and provider, with bounded fonts and glyph outlines shared
/// across page compilations. Dropping the context releases its retained caches.
/// Source/object caches remain the document's responsibility (ADR 0012).
pub struct RenderContext {
    document: Arc<Document>,
    provider: Option<Arc<dyn Provider>>,
    pub(crate) fonts: Arc<Mutex<interpreter::text::cache::FontCache>>,
}

impl std::fmt::Debug for RenderContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderContext")
            .field("document", &self.document)
            .field("has_provider", &self.provider.is_some())
            .finish_non_exhaustive()
    }
}

impl RenderContext {
    /// Creates a context using only resources embedded in the PDF.
    pub fn new(document: Arc<Document>) -> Self {
        Self::create(document, None)
    }

    /// Creates a context with explicit external font resources. Sharing the
    /// provider also shares its data parsing across contexts and documents.
    pub fn with_provider(document: Arc<Document>, provider: Arc<dyn Provider>) -> Self {
        Self::create(document, Some(provider))
    }

    fn create(document: Arc<Document>, provider: Option<Arc<dyn Provider>>) -> Self {
        let fonts = Arc::new(Mutex::new(interpreter::text::cache::FontCache::new(
            document.limits(),
        )));
        Self {
            document,
            provider,
            fonts,
        }
    }

    /// The immutable source document to which resource identities belong.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Estimated font-input, dictionary, outline and bookkeeping bytes
    /// retained, bounded by [`pagura_core::Limits::document_bytes`]. Parsed font internals
    /// retain their own limits; this is not allocator or peak-memory telemetry.
    pub fn retained_bytes(&self) -> usize {
        self.fonts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bytes
    }

    /// Compiles a page, reusing this document's fonts and glyph outlines.
    ///
    /// # Errors
    /// As [`crate::compile_page`]. A page from another document is rejected.
    pub fn compile_page(&self, page: &Page) -> Result<DisplayList> {
        let owned = self.document.page(page.index())?;
        if owned.id() != page.id()
            || !std::ptr::eq(owned.resources(), page.resources())
            || owned != page
        {
            return Err(pagura_core::Error::invalid_argument(
                "page does not belong to rendering context",
            ));
        }
        interpreter::compile_cached(&self.document, page, self.provider.as_deref(), &self.fonts)
    }

    /// Compiles and renders a whole page with shared resources.
    ///
    /// # Errors
    /// As [`Self::compile_page`] and [`DisplayList::render`].
    pub fn render_page(&self, page: &Page, options: &RenderOptions) -> Result<Pixmap> {
        self.compile_page(page)?.render(options)
    }

    /// Compiles and renders a tile with shared resources.
    ///
    /// # Errors
    /// As [`Self::compile_page`] and [`DisplayList::render_tile`].
    pub fn render_tile(&self, page: &Page, options: &RenderOptions, tile: Tile) -> Result<Pixmap> {
        self.compile_page(page)?.render_tile(options, tile)
    }
}
