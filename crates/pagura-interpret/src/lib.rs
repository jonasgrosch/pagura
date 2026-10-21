#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! PDF content interpretation (ADR 0034): one interpreter runs a page's
//! content streams, annotations and forms through the graphics state machine
//! (clauses 8–11, 12.5, 14.6) and emits the result into a [`Device`].
//!
//! The interpreter asks the device for its [`Needs`] first and does no work
//! it does not need. Two devices live here: [`SceneRecorder`], which records
//! a device-independent [`Scene`] for rendering, and [`TextDevice`], which
//! collects positioned glyphs for `pagura-extract` without decoding images,
//! parsing shadings or converting colours.

mod device;
mod model;
mod recorder;
mod scene;
mod text_device;

pub use device::{
    Device, FillMark, FontInfo, FontKey, Glyph, GlyphPaint, GlyphRun, GroupInfo, ImageMark,
    MarkState, Marked, Needs, OutlineKey, PageInfo, ShadingMark, SoftMaskInfo, StrokeMark,
    TilingInfo, glyph_origin,
};
pub use model::{
    BlendMode, Clip, Color, Composite, GroupAttributes, Image, ImageMask, MaskId, MaskKind,
    MeshVertex, Overprint, Paint, Patch, PatternId, Pen, Shading, ShadingFunction, ShadingKind,
    ShadingPaint, TilingColors, TilingPaint, Transfer, ViewFit,
};
pub use recorder::SceneRecorder;
pub use scene::{
    ClipId, ClipNode, FillNode, GlyphSource, GlyphsNode, Group, GroupId, GroupRole, ImageNode,
    MarkInfo, Node, NodeId, PageSpace, PaintId, PenId, Placement, Scene, ShadingNode, SoftMask,
    StateId, StrokeNode, Tiling,
};
pub use text_device::TextDevice;

use pagura_core::{Error, Result, Store};
use pagura_document::{Document, Page};

/// The shared inputs of interpreting pages of one document.
#[derive(Clone, Copy)]
pub struct Interpreter<'a> {
    document: &'a Document,
    provider: Option<&'a dyn pagura_text::Provider>,
    store: Option<&'a Store>,
    filler: Option<&'a pagura_document::forms::FormFiller<'a>>,
}

impl std::fmt::Debug for Interpreter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Interpreter")
            .field("provider", &self.provider.is_some())
            .field("store", &self.store.is_some())
            .field("filler", &self.filler.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> Interpreter<'a> {
    /// Interprets pages of `document` with no external fonts and no cache.
    pub fn new(document: &'a Document) -> Self {
        Self {
            document,
            provider: None,
            store: None,
            filler: None,
        }
    }

    /// Uses `provider` for fonts the document does not embed (ADR 0009).
    #[must_use]
    pub fn with_provider(mut self, provider: Option<&'a dyn pagura_text::Provider>) -> Self {
        self.provider = provider;
        self
    }

    /// Caches font programs and other reusable values in `store`.
    #[must_use]
    pub fn with_store(mut self, store: &'a Store) -> Self {
        self.store = Some(store);
        self
    }

    /// Draws form fields with the in-memory values of `filler`, generating
    /// appearances where needed (12.7.4.3).
    #[must_use]
    pub fn with_filler(mut self, filler: &'a pagura_document::forms::FormFiller<'a>) -> Self {
        self.filler = Some(filler);
        self
    }

    /// The document.
    pub fn document(&self) -> &'a Document {
        self.document
    }

    /// Interprets `page` into `device`.
    ///
    /// # Errors
    ///
    /// Returns positioned `Syntax` errors, `Limit` errors for budgets,
    /// stable `Unsupported` tags and the device's errors. A failed
    /// interpretation leaves the device with a partial page, which devices
    /// discard.
    pub fn run<D: Device + ?Sized>(&self, page: &Page, device: &mut D) -> Result<()> {
        let _ = (page, device);
        Err(Error::invalid_argument("interpretation is not implemented"))
    }

    /// Records `page` for rendering.
    ///
    /// # Errors
    ///
    /// As [`Self::run`].
    pub fn scene(&self, page: &Page) -> Result<Scene> {
        let mut recorder = SceneRecorder::new(self.document.limits());
        self.run(page, &mut recorder)?;
        recorder.finish()
    }

    /// Collects the positioned text of `page`.
    ///
    /// # Errors
    ///
    /// As [`Self::run`], plus `text_items` limits.
    pub fn text(&self, page: &Page) -> Result<pagura_extract::PositionedText> {
        let mut device = TextDevice::new(self.document.limits());
        self.run(page, &mut device)?;
        device.finish()
    }
}
