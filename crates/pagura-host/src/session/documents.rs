//! The registry of open documents and everything retained per page.

use crate::cache::{self, PageCache, PageKey};
use crate::config::{SessionConfig, SessionLimits};
use crate::protocol::{Field, PageRef, Response, SessionStats};
use pagura::ReadResult;
use pagura::{
    Diagnostic, Document, Error, Limits, OpenOptions, Page, Recovery, Rect, RenderContext,
    RenderOptions, Result, Scene, TextPage,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

/// One open document and its session state.
#[derive(Debug)]
struct Slot {
    /// The document and its render store, which keeps recorded scenes.
    context: RenderContext,
    /// The scenes recorded for tiles and bounds by page index; a scene is
    /// retained while its store keeps it.
    scenes: BTreeMap<u32, Weak<Scene>>,
    /// The selected viewport generation, zero before the first (1.1).
    generation: u32,
}

/// Open documents by handle, with their render stores and the text cache
/// they share. Closing a handle releases everything here in one place.
#[derive(Debug)]
pub(super) struct Documents {
    engine: Limits,
    limits: SessionLimits,
    slots: BTreeMap<u32, Slot>,
    next_handle: u32,
    source_bytes: usize,
    texts: PageCache<TextPage>,
}

impl Documents {
    pub(super) const fn new(config: &SessionConfig) -> Self {
        Self {
            engine: config.engine,
            limits: config.session,
            slots: BTreeMap::new(),
            next_handle: 1,
            source_bytes: 0,
            texts: PageCache::new(config.session.text_cache_bytes),
        }
    }

    /// Opens `bytes` under the engine limits; a failed open consumes no handle.
    pub(super) fn open(
        &mut self,
        bytes: &[u8],
        recovery: Recovery,
        password: &[u8],
    ) -> ReadResult<Response> {
        self.admit(bytes.len())?;
        let options = OpenOptions {
            limits: self.engine,
            recovery,
            password: password.to_vec(),
        };
        let document = Document::open(bytes, &options)?;
        self.insert(document)
            .map(|(document, pages)| Response::Open { document, pages })
    }

    pub(super) fn reauthenticate(
        &mut self,
        handle: Field<u32>,
        password: &[u8],
    ) -> ReadResult<Response> {
        // Preflight retained source/handle budgets before opening the snapshot.
        self.admit(self.document(handle)?.source_len())?;
        let document = self.document(handle)?.reauthenticate(password)?;
        let (document, pages) = self.insert(document)?;
        let mut out = crate::wire::Writer::new(8);
        out.u32(document)?;
        out.u32(pages)?;
        Ok(Response::Reading(crate::reading::Response::new(
            crate::protocol::ResponseKind::Reauthenticate,
            out,
        )))
    }

    fn admit(&self, length: usize) -> Result<usize> {
        let retained = self
            .source_bytes
            .checked_add(length)
            .filter(|&size| size <= self.limits.total_source_bytes)
            .ok_or(Error::limit("session source byte budget exceeded"))?;
        if self.slots.len() >= self.limits.max_documents || self.next_handle == u32::MAX {
            return Err(Error::limit("session document handle budget exceeded"));
        }
        Ok(retained)
    }

    fn insert(&mut self, document: Document) -> ReadResult<(u32, u32)> {
        let retained = self.admit(document.source_len())?;
        let pages = u32::try_from(document.page_count()?)
            .map_err(|_| Error::limit("page count exceeds protocol range"))?;
        let handle = self.next_handle;
        let provider = pagura::provider::default_provider(self.engine);
        let context = RenderContext::with_store_capacity(
            Arc::new(document),
            provider,
            self.limits.store_bytes,
        );
        self.slots.insert(
            handle,
            Slot {
                context,
                scenes: BTreeMap::new(),
                generation: 0,
            },
        );
        self.next_handle += 1;
        self.source_bytes = retained;
        Ok((handle, pages))
    }

    pub(super) fn close(&mut self, handle: Field<u32>) -> Result<Response> {
        let slot = self
            .slots
            .remove(&handle.get())
            .ok_or_else(|| unknown(handle))?;
        self.texts.close(handle.get());
        let released = slot.context.document().source_len();
        self.source_bytes = self.source_bytes.saturating_sub(released);
        Ok(Response::Close)
    }

    fn slot(&self, handle: Field<u32>) -> Result<&Slot> {
        self.slots.get(&handle.get()).ok_or_else(|| unknown(handle))
    }

    pub(super) fn document(&self, handle: Field<u32>) -> Result<&Document> {
        let document = self.slot(handle)?.context.document();
        document.check_source()?;
        Ok(document)
    }

    /// The document's rendering context and the page.
    pub(super) fn page(&self, page: PageRef) -> ReadResult<(&RenderContext, &Page)> {
        let context = &self.slot(page.document)?.context;
        let found = context
            .document()
            .page(page.page.index()?)
            .map_err(|error| error.at(page.page.position()))?;
        Ok((context, found))
    }

    pub(super) fn page_info(&self, page: PageRef) -> ReadResult<Response> {
        let (_, page) = self.page(page)?;
        let crop = page.crop_box();
        Ok(Response::PageInfo {
            crop: [crop.x0(), crop.y0(), crop.x1(), crop.y1()],
            rotation: page.rotation().degrees(),
            user_unit: page.user_unit(),
            size: pagura::device_size(page, &RenderOptions::default())?,
        })
    }

    /// The page's recorded scene, from its document's store or recorded now
    /// and stored.
    pub(super) fn scene(&mut self, page: PageRef) -> ReadResult<Arc<Scene>> {
        let (context, found) = self.page(page)?;
        let scene = context.scene(found)?;
        if let Some(slot) = self.slots.get_mut(&page.document.get()) {
            slot.scenes.retain(|_, scene| scene.strong_count() > 0);
            slot.scenes.insert(page.page.get(), Arc::downgrade(&scene));
        }
        Ok(scene)
    }

    /// Conservative painted bounds of the page, from its stored scene.
    pub(super) fn content_bounds(&mut self, page: PageRef) -> ReadResult<Option<Rect>> {
        // Recording through `scene` first keeps the scene in the accounting.
        self.scene(page)?;
        let (context, found) = self.page(page)?;
        context.content_bounds(found)
    }

    /// The page's extracted text from the cache, or extracted and retained now.
    pub(super) fn text(&mut self, page: PageRef) -> ReadResult<Arc<TextPage>> {
        self.document(page.document)?.check_source()?;
        let key = key(page);
        if let Some(text) = self.texts.get(key) {
            return Ok(text);
        }
        let (context, found) = self.page(page)?;
        let text = Arc::new(context.text_page(found)?);
        self.texts
            .insert(key, Arc::clone(&text), cache::text_charge(&text));
        Ok(text)
    }

    /// Selects a newer viewport generation (1.1); repeating one is idempotent.
    pub(super) fn select_generation(
        &mut self,
        document: Field<u32>,
        generation: Field<u32>,
    ) -> Result<Response> {
        let slot = self
            .slots
            .get_mut(&document.get())
            .ok_or_else(|| unknown(document))?;
        let requested = generation.get();
        if requested == 0 || requested < slot.generation {
            return Err(Error::invalid_argument("viewport generation must increase")
                .at(generation.position()));
        }
        slot.generation = requested;
        Ok(Response::Generation(requested))
    }

    /// The selected generation of a document, zero before the first.
    pub(super) fn generation(&self, document: Field<u32>) -> Result<u32> {
        self.slot(document).map(|slot| slot.generation)
    }

    /// Repairs and information discovered so far, in discovery order.
    pub(super) fn diagnostics(&self, handle: Field<u32>) -> Result<Vec<Diagnostic>> {
        self.document(handle).map(Document::diagnostics)
    }

    /// Source bytes, the bytes the render stores retain and the scenes they
    /// keep.
    pub(super) fn stats(&self) -> SessionStats {
        let retained = self
            .slots
            .values()
            .map(|slot| slot.context.retained_bytes())
            .fold(0_usize, usize::saturating_add);
        let scenes = self
            .slots
            .values()
            .flat_map(|slot| slot.scenes.values())
            .filter(|scene| scene.strong_count() > 0)
            .count();
        SessionStats {
            source_bytes: u64::try_from(self.source_bytes).unwrap_or(u64::MAX),
            cache_bytes: u64::try_from(retained).unwrap_or(u64::MAX),
            documents: u32::try_from(self.slots.len()).unwrap_or(u32::MAX),
            lists: u32::try_from(scenes).unwrap_or(u32::MAX),
        }
    }

    /// Text-cache charges and retained pages.
    pub(super) fn text_accounting(&self) -> (usize, usize) {
        self.texts.accounting()
    }

    /// The scene a document's store still keeps for a page.
    #[cfg(test)]
    pub(super) fn stored_scene(&self, document: u32, page: u32) -> Option<Arc<Scene>> {
        self.slots.get(&document)?.scenes.get(&page)?.upgrade()
    }
}

const fn key(page: PageRef) -> PageKey {
    PageKey {
        document: page.document.get(),
        page: page.page.get(),
    }
}

fn unknown(handle: Field<u32>) -> Error {
    Error::invalid_argument("unknown document handle").at(handle.position())
}
