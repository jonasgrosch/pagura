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
//! parsing shadings or converting colours. [`dump`] prints a scene for
//! snapshots and debugging.
//!
//! Content is read by `pagura-content`; colour spaces and functions are
//! resolved into `pagura-color` values, images into undecoded
//! `pagura-image` sources, fonts through `pagura-text`, and appearances of
//! annotations without one and of filled forms come from
//! `pagura-appearance`. Malformed input is repaired and reported exactly as
//! the [`repairs`] tags say, or rejected under strict recovery; capabilities
//! that are not implemented are `Unsupported` errors with a [`features`]
//! tag.

mod device;
mod dump;
pub mod features;
mod fonts;
mod hash;
mod machine;
mod model;
mod recorder;
pub mod repairs;
mod resources;
mod scene;
mod text_device;

pub use device::{
    Device, FillMark, FontInfo, FontKey, Glyph, GlyphPaint, GlyphRun, GroupInfo, ImageMark,
    MarkState, Marked, MarkedText, Needs, OutlineKey, PageInfo, ShadingMark, SoftMaskInfo,
    StrokeMark, TilingInfo, glyph_origin,
};
pub use dump::dump;
pub use model::{
    BlendMode, Clip, Color, Composite, GroupAttributes, GroupCause, Image, ImageMask, MaskId, MaskKind,
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

use machine::{Inputs, Machine};
use pagura_core::{Error, ObjectId, Rect, Result, Store};
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

    /// Caches font programs, glyph outlines and ICC profiles in `store`, by
    /// object identity, so that later pages and interpretations reuse
    /// them. The keys name objects of this document, so a store serves one
    /// document.
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

    /// Interprets `page` into `device`: its content streams as one sequence
    /// (7.8.2), then its annotations (12.5).
    ///
    /// # Errors
    ///
    /// Returns positioned `Syntax` errors, `Limit` errors for budgets,
    /// stable `Unsupported` tags and the device's errors; repairs of
    /// malformed content are reported to the document, or are the error in
    /// strict recovery. A failed interpretation leaves the device with a
    /// partial page, which devices discard. `InvalidArgument` when a form
    /// filler belongs to another document.
    pub fn run<D: Device + ?Sized>(&self, page: &Page, device: &mut D) -> Result<()> {
        if self
            .filler
            .is_some_and(|filler| !std::ptr::eq(filler.document(), self.document))
        {
            return Err(Error::invalid_argument(
                "page does not belong to form document",
            ));
        }
        let inputs = self.inputs(Some(page.resources()));
        let mut machine = Machine::new(&inputs, device)?;
        machine.page(page).map_err(|e| e.in_object(page.id()))
    }

    fn inputs<'p>(&self, page_resources: Option<&'p pagura_core::Dictionary>) -> Inputs<'p>
    where
        'a: 'p,
    {
        Inputs {
            document: self.document,
            provider: self.provider,
            store: self.store,
            filler: self.filler,
            page_resources,
        }
    }

    /// Records `page` for rendering.
    ///
    /// # Errors
    ///
    /// As [`Self::run`], plus `Limit` errors for the scene's marks
    /// (`display_commands`) and bytes (`render_bytes`).
    pub fn scene(&self, page: &Page) -> Result<Scene> {
        let mut recorder = SceneRecorder::new(self.document.limits());
        self.run(page, &mut recorder)?;
        recorder.finish()
    }

    /// Collects the positioned text of `page`: glyphs in content order with
    /// their marked content and object occurrences (ADR 0014). Coordinates
    /// are default user space, before rotation, `UserUnit` and scaling.
    /// Annotation appearances, pattern cells, soft masks and Type 3 glyph
    /// procedures are paint, not page text; no image is decoded, no shading
    /// parsed and no colour converted.
    ///
    /// # Errors
    ///
    /// As [`Self::run`], plus `Limit` errors for `text_items` and the text's
    /// bytes (`document_bytes`).
    pub fn text(&self, page: &Page) -> Result<pagura_extract::PositionedText> {
        let mut device = TextDevice::new(self.document.limits());
        self.run(page, &mut device)?;
        device.finish()
    }

    /// The positioned text of one stored annotation appearance `appearance`
    /// of `page`, fitted into `rect` (12.5.5). Coordinates include the
    /// fitting and are in default user space. The page's content and other
    /// annotations are not interpreted.
    ///
    /// # Errors
    ///
    /// As [`Self::text`].
    pub fn appearance_text(
        &self,
        page: &Page,
        appearance: ObjectId,
        rect: Rect,
    ) -> Result<pagura_extract::PositionedText> {
        let mut device = TextDevice::new(self.document.limits());
        let inputs = self.inputs(None);
        let mut machine = Machine::new(&inputs, &mut device)?;
        machine.appearance_text(appearance, rect, page.resources())?;
        device.finish()
    }
}
