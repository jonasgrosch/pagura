//! Facade document lifetime owns its separate rendering context.
use crate::{OpenOptions, RenderContext, Result};
use std::sync::Arc;

/// An immutable PDF document with document-bound rendering resources. Source
/// access delegates to [`pagura_document::Document`]; fonts and outlines are
/// released with this value. No process-global mutable caches are used.
#[derive(Debug)]
pub struct Document {
    rendering: RenderContext,
}
impl Document {
    /// Opens a document with the requested limits and recovery policy.
    ///
    /// # Errors
    /// As [`pagura_document::Document::open`]. External font data is loaded lazily.
    pub fn open(bytes: impl Into<Arc<[u8]>>, options: &OpenOptions) -> Result<Self> {
        pagura_document::Document::open(bytes, options).map(Self::from)
    }

    /// Uses an explicit provider instead of this facade's feature-selected data.
    /// Existing source and diagnostics remain in the supplied document.
    pub fn with_provider(
        document: pagura_document::Document,
        provider: Arc<dyn crate::Provider>,
    ) -> Self {
        Self {
            rendering: RenderContext::with_provider(Arc::new(document), provider),
        }
    }

    /// Shared rendering resources, also usable by a long-lived native/WASI host.
    pub fn render_context(&self) -> &RenderContext {
        &self.rendering
    }
}
impl From<pagura_document::Document> for Document {
    fn from(document: pagura_document::Document) -> Self {
        let document = Arc::new(document);
        #[cfg(feature = "bundled-data")]
        let rendering = RenderContext::with_provider(
            Arc::clone(&document),
            Arc::new(pagura_data::Bundled::new(document.limits())),
        );
        #[cfg(not(feature = "bundled-data"))]
        let rendering = RenderContext::new(document);
        Self { rendering }
    }
}
impl std::ops::Deref for Document {
    type Target = pagura_document::Document;
    fn deref(&self) -> &Self::Target {
        self.rendering.document()
    }
}
impl AsRef<pagura_document::Document> for Document {
    fn as_ref(&self) -> &pagura_document::Document {
        self
    }
}
