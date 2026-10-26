//! The rendering context of one document (ADR 0012, ADR 0034).

mod loading;
pub use loading::{RenderJob, RenderJobLimits, RenderLoading};

use crate::options::{RenderOptions, Tile};
use crate::renderer::Renderer;
use pagura_core::{Built, ObjectId, Rect, Result, Store, StoreKey};
use pagura_document::forms::FormFiller;
use pagura_document::source::ReadResult;
use pagura_document::{Document, Page};
use pagura_extract::{PositionedText, TextPage};
use pagura_interpret::Scene;
use pagura_paint::{Completion, OutputLimits, RenderSession, Scratch};
use pagura_raster::Pixmap;
use pagura_text::Provider;
use std::ops::Range;
use std::sync::{Arc, Mutex};

/// A document, its font provider and the [`Store`] that holds everything
/// rendering its pages reuses: recorded scenes, decoded images, colour
/// transforms, font programs and glyph edge setups, under one byte capacity.
///
/// Every page is recorded once into a [`Scene`] and rendered from it at any
/// resolution or region. Dropping the context
/// releases the store; the document's own object caches stay with the
/// document (ADR 0012). The context is `Send + Sync`.
///
/// It also keeps the scratch storage of its last render for the next, while
/// the store's capacity holds it; a render that finds it taken by another
/// thread makes its own.
pub struct RenderContext {
    document: Arc<Document>,
    provider: Option<Arc<dyn Provider>>,
    store: Store,
    scratch: Mutex<Option<Scratch>>,
}

impl std::fmt::Debug for RenderContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderContext")
            .field("document", &self.document)
            .field("provider", &self.provider.is_some())
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

/// A recorded scene in the store. A page object can occur more than once in
/// the page tree with different inherited attributes, so the page is named
/// by its object and its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SceneKey {
    page: ObjectId,
    index: usize,
    configuration: Configuration,
}

/// The optional-content configuration that decided visibility (8.11.4.3).
/// Scenes are recorded with the document's default configuration, `D`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Configuration {
    Default,
}

impl StoreKey for SceneKey {
    type Value = Scene;
}

impl RenderContext {
    /// The store capacity of [`Self::new`] and [`Self::with_provider`]:
    /// 64 MiB, what the viewer host retained between tiles before the store.
    pub const DEFAULT_STORE_BYTES: usize = 64 * 1024 * 1024;

    /// A context that uses only the resources embedded in the PDF, with the
    /// default store capacity.
    pub fn new(document: Arc<Document>) -> Self {
        Self::with_store_capacity(document, None, Self::DEFAULT_STORE_BYTES)
    }

    /// A context with external font resources (ADR 0009) and the default
    /// store capacity. Sharing one provider across contexts also shares its
    /// parsing.
    pub fn with_provider(document: Arc<Document>, provider: Arc<dyn Provider>) -> Self {
        Self::with_store_capacity(document, Some(provider), Self::DEFAULT_STORE_BYTES)
    }

    /// A context whose store retains at most `capacity` bytes between
    /// renders (ADR 0012). Zero retains nothing, so every render records and
    /// decodes again.
    pub fn with_store_capacity(
        document: Arc<Document>,
        provider: Option<Arc<dyn Provider>>,
        capacity: usize,
    ) -> Self {
        Self {
            document,
            provider,
            store: Store::new(capacity),
            scratch: Mutex::new(None),
        }
    }

    /// The document.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Bytes the store retains: an accounting estimate of the values it
    /// holds, at most its capacity (and values someone else still holds), not
    /// allocator telemetry.
    pub fn retained_bytes(&self) -> usize {
        self.store.retained_bytes()
    }

    fn renderer(&self) -> Renderer<'_> {
        Renderer {
            document: &self.document,
            provider: self.provider.as_deref(),
            store: &self.store,
        }
    }

    /// The recorded scene of `page`, from the store or recorded now and
    /// stored. It uses stored appearances and ignores `NeedAppearances`;
    /// [`Self::filled_scene`] honours it.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a page of another document, and the
    /// interpreter's errors: positioned `Syntax` errors, `Limit` errors and
    /// stable `Unsupported` tags. A failed recording stores nothing.
    pub fn scene(&self, page: &Page) -> ReadResult<Arc<Scene>> {
        self.document.check_page(page)?;
        let key = SceneKey {
            page: page.id(),
            index: page.index(),
            configuration: Configuration::Default,
        };
        self.store.get_or_build(key, || {
            let scene = self.renderer().record(page)?;
            let bytes = scene.bytes;
            Ok(Built {
                value: scene,
                bytes,
            })
        })
    }

    /// Records `page` without keeping the scene, for a single pass over
    /// pages: fonts, glyphs and images still come from the store, but the
    /// scene is the caller's alone.
    ///
    /// # Errors
    ///
    /// As [`Self::scene`].
    pub fn record(&self, page: &Page) -> ReadResult<Scene> {
        self.document.check_page(page)?;
        self.renderer().record(page)
    }

    /// Records `page` with the in-memory form values of `filler`, generating
    /// appearances where they are needed (12.7.4.3). Filled scenes change with
    /// every edit, so they are not stored.
    ///
    /// # Errors
    ///
    /// As [`Self::scene`], and `InvalidArgument` when `filler` belongs to
    /// another document.
    pub fn filled_scene(&self, filler: &FormFiller<'_>, page: &Page) -> ReadResult<Scene> {
        self.document.check_page(page)?;
        self.renderer().record_filled(filler, page)
    }

    /// Renders the whole page.
    ///
    /// # Errors
    ///
    /// As [`Self::scene`] and [`Self::render_scene`]. No partial page is ever
    /// returned.
    pub fn render_page(&self, page: &Page, options: &RenderOptions) -> ReadResult<Pixmap> {
        Ok(self.render_scene(&*self.scene(page)?, options, None)?)
    }

    /// Renders one tile of the page. Tiles of one page with the same options
    /// assemble byte for byte into [`Self::render_page`]'s result.
    ///
    /// # Errors
    ///
    /// As [`Self::render_page`], and `InvalidArgument` for a tile that is
    /// empty or outside the page.
    pub fn render_tile(
        &self,
        page: &Page,
        options: &RenderOptions,
        tile: Tile,
    ) -> ReadResult<Pixmap> {
        Ok(self.render_scene(&*self.scene(page)?, options, Some(tile))?)
    }

    /// Renders `tile` of a scene this context recorded, or its whole page
    /// for `None`. Images and glyphs come from this context's store, which
    /// names them by this document's objects, so the scene must come from
    /// [`Self::scene`] or [`Self::filled_scene`] of this context.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a non-positive or non-finite scale and a
    /// tile that is empty or outside the page; otherwise the first error of
    /// preparation in source order among the marks that meet the region:
    /// `Syntax` and `Unsupported` for content that cannot be painted, `Limit`
    /// for `pixels`, `render_bytes`, `render_work`, `function_work` and the
    /// per-fill budgets, and a refused repair under strict recovery. Budgets
    /// are per render, so a whole page can reach a limit its tiles do not.
    pub fn render_scene(
        &self,
        scene: &Scene,
        options: &RenderOptions,
        tile: Option<Tile>,
    ) -> Result<Pixmap> {
        self.render_until(scene, options, tile, |_| false)
            .map(|(pixmap, _)| pixmap)
    }

    /// Renders as [`Self::render_scene`] band by band, top to bottom, asking
    /// `stop` with the first device row of each band before rendering it, so
    /// that a viewer can abandon or interleave work. Everything that can fail
    /// fails before the first band; stopping is not an error. After
    /// [`Completion::Stopped`], the pixmap's rows above the reported device
    /// row are rendered and the rest hold the background.
    ///
    /// # Errors
    ///
    /// As [`Self::render_scene`].
    pub fn render_until(
        &self,
        scene: &Scene,
        options: &RenderOptions,
        tile: Option<Tile>,
        stop: impl FnMut(u32) -> bool,
    ) -> Result<(Pixmap, Completion)> {
        self.with_scratch(|scratch| {
            self.renderer()
                .render(scene, (options, tile), scratch, stop)
        })
    }

    /// Renders the whole region as [`Self::render_scene`] does, lending
    /// each completed band's device-row range and premultiplied RGBA8 bytes
    /// to `consume`, top to bottom. The bytes are valid only for that call.
    /// `stop` is asked before each band, as in [`Self::render_until`].
    ///
    /// All preparation, repair reporting and renderer allocation finish
    /// before the first consumer call. Output storage is one reusable band,
    /// capped by `Limits::decoded_bytes`; an owned full surface is not
    /// allocated or subject to that byte cap. The whole region still obeys
    /// `Limits::pixels`, and all render budgets remain per region.
    ///
    /// # Errors
    ///
    /// The outer result has the preparation and allocation errors of
    /// [`Self::render_scene`], before any consumer call. The inner result
    /// returns the first consumer error without rendering another band.
    /// Stopping is not an error. Repeating the call starts at the region's
    /// first row, whether the previous call completed, stopped or failed.
    pub fn render_bands<E>(
        &self,
        scene: &Scene,
        options: &RenderOptions,
        tile: Option<Tile>,
        stop: impl FnMut(u32) -> bool,
        consume: impl FnMut(Range<u32>, &[u8]) -> std::result::Result<(), E>,
    ) -> Result<std::result::Result<Completion, E>> {
        self.with_scratch(|scratch| {
            self.renderer()
                .render_bands(scene, (options, tile), scratch, stop, consume)
        })
    }

    /// Prepares an owned render that continues one final band per step.
    ///
    /// The session owns its scratch and reserved output slots, holds no context
    /// lock, and needs no source bytes after this call. Leases pin output slots
    /// until released. Existing callbacks retain their per-call restart behavior.
    ///
    /// # Errors
    /// As [`Self::render_scene`], plus the separate output quota in `limits`.
    /// All errors occur before the session can publish output.
    pub fn render_session(
        &self,
        scene: &Scene,
        options: &RenderOptions,
        tile: Option<Tile>,
        limits: OutputLimits,
    ) -> Result<RenderSession> {
        let prepared = Arc::new(self.renderer().prepare(scene, (options, tile))?);
        let scratch = self
            .scratch
            .lock()
            .ok()
            .and_then(|mut kept| kept.take())
            .unwrap_or_default();
        RenderSession::new(prepared, scratch, limits)
    }

    /// Takes and returns scratch identically for collectors and consumers,
    /// including preparation refusals, stops and consumer errors.
    fn with_scratch<T>(&self, render: impl FnOnce(&mut Scratch) -> T) -> T {
        let kept = self.scratch.lock().ok().and_then(|mut kept| kept.take());
        let mut scratch = kept.unwrap_or_default();
        let rendered = render(&mut scratch);
        if scratch.retained_bytes() <= self.store.capacity()
            && let Ok(mut kept) = self.scratch.lock()
        {
            *kept = Some(scratch);
        }
        rendered
    }

    /// The positioned text of `page` in reading order with its logical
    /// structure (ADR 0014), through the text device: no image is decoded,
    /// no shading parsed and no colour converted. Coordinates are default
    /// user space, before rotation, `UserUnit` and scaling. Annotation
    /// appearances, pattern cells, soft masks and Type 3 glyph procedures are
    /// paint, not page text.
    ///
    /// # Errors
    ///
    /// As [`Self::scene`], plus `text_items` limits and the layout's errors.
    pub fn text_page(&self, page: &Page) -> ReadResult<TextPage> {
        self.document.check_page(page)?;
        self.renderer().text_page(page)
    }

    /// The positioned text of the stored annotation appearance `appearance`
    /// of `page`, fitted into `rect` (12.5.5), through the text device. The
    /// page's content and other annotations are not interpreted.
    ///
    /// # Errors
    ///
    /// As [`Self::text_page`].
    pub fn appearance_text(
        &self,
        page: &Page,
        appearance: ObjectId,
        rect: Rect,
    ) -> ReadResult<PositionedText> {
        self.document.check_page(page)?;
        self.renderer().appearance_text(page, appearance, rect)
    }

    /// Conservative bounds of what `page` paints, in default user space and
    /// clamped to the crop box (12.3.2.2), or `None` when it paints nothing.
    /// Control hulls, stroke envelopes, clips and group bounding boxes can
    /// include empty space; the bounds never exclude a painted pixel.
    ///
    /// # Errors
    ///
    /// As [`Self::scene`], and `Limit` when groups nest deeper than
    /// `Limits::depth`.
    pub fn content_bounds(&self, page: &Page) -> ReadResult<Option<Rect>> {
        let scene = self.scene(page)?;
        Ok(crate::bounds::content_bounds(
            &scene,
            self.document.limits().depth,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::RenderContext;
    use crate::{Completion, RenderOptions};
    use pagura_document::{Document, OpenOptions};
    use pagura_testkit::open_fixture;
    use std::sync::Arc;

    #[test]
    fn collectors_and_consumers_return_or_drop_scratch_on_every_exit() {
        for capacity in [0, 1 << 20] {
            let document = Arc::new(open_fixture("classic", "rectangle.pdf", |bytes| {
                Document::open(bytes, &OpenOptions::default())
            }));
            let context = RenderContext::with_store_capacity(document, None, capacity);
            let scene = context.record(context.document().page(0).unwrap()).unwrap();
            let options = RenderOptions::default();
            let expected = context.render_scene(&scene, &options, None).unwrap();
            assert_eq!(context.scratch.lock().unwrap().is_some(), capacity != 0);
            let failed = context
                .render_bands(&scene, &options, None, |_| false, |_, _| Err("sink"))
                .unwrap();
            assert_eq!(failed, Err("sink"));
            assert_eq!(context.scratch.lock().unwrap().is_some(), capacity != 0);
            let stopped = context
                .render_bands(
                    &scene,
                    &options,
                    None,
                    |_| true,
                    |_, _| panic!("stopped render called its sink"),
                )
                .unwrap();
            assert_eq!(stopped, Ok::<_, ()>(Completion::Stopped { row: 0 }));
            assert_eq!(context.scratch.lock().unwrap().is_some(), capacity != 0);
            let previous = context
                .scratch
                .lock()
                .unwrap()
                .as_ref()
                .map_or(0, pagura_paint::Scratch::retained_bytes);
            let error = context
                .render_bands(
                    &scene,
                    &RenderOptions::from_dpi(0.0),
                    None,
                    |_| false,
                    |_, _| Ok::<(), ()>(()),
                )
                .unwrap_err();
            assert_eq!(error.kind, pagura_core::ErrorKind::InvalidArgument);
            // An early refusal leaves the prior scratch untouched; when
            // none was kept, its empty replacement fits even capacity 0.
            let retained = context
                .scratch
                .lock()
                .unwrap()
                .as_ref()
                .map(pagura_paint::Scratch::retained_bytes);
            assert_eq!(retained, Some(previous));
            let actual = context.render_scene(&scene, &options, None).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(context.scratch.lock().unwrap().is_some(), capacity != 0);
        }
    }
}
