//! The registry of open documents and everything retained per page.

use crate::cache::{self, PageCache, PageKey};
use crate::config::{SessionConfig, SessionLimits};
use crate::protocol::{Response, SessionStats, layout};
use pagura::{
    Diagnostic, DisplayList, Document, Error, Limits, OpenOptions, Page, Position, Recovery,
    RenderContext, RenderOptions, Result, TextPage,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Body offset of a request's leading document handle; a page index follows
/// it four bytes later.
pub(super) const HANDLE: usize = layout::HEADER_BYTES;

/// One open document and its session state.
#[derive(Debug)]
struct Slot {
    context: RenderContext,
    /// The selected viewport generation, zero before the first (1.1).
    generation: u32,
}

/// Open documents by handle, with the page caches they share. Closing a
/// handle releases everything here in one place.
#[derive(Debug)]
pub(super) struct Documents {
    engine: Limits,
    limits: SessionLimits,
    slots: BTreeMap<u32, Slot>,
    next_handle: u32,
    source_bytes: usize,
    lists: PageCache<DisplayList>,
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
            lists: PageCache::new(config.session.list_cache_bytes),
            texts: PageCache::new(config.session.text_cache_bytes),
        }
    }

    /// Opens `bytes` under the engine limits; a failed open consumes no handle.
    pub(super) fn open(
        &mut self,
        bytes: &[u8],
        recovery: Recovery,
        password: &[u8],
    ) -> Result<Response> {
        let retained = self
            .source_bytes
            .checked_add(bytes.len())
            .filter(|&size| size <= self.limits.total_source_bytes)
            .ok_or(Error::limit("session source byte budget exceeded"))?;
        if self.slots.len() >= self.limits.max_documents || self.next_handle == u32::MAX {
            return Err(Error::limit("session document handle budget exceeded"));
        }
        let options = OpenOptions {
            limits: self.engine,
            recovery,
            password: password.to_vec(),
        };
        let document = Document::open(bytes, &options)?;
        let pages = u32::try_from(document.page_count()?)
            .map_err(|_| Error::limit("page count exceeds protocol range"))?;
        let handle = self.next_handle;
        let context = pagura::render_context(Arc::new(document));
        self.slots.insert(
            handle,
            Slot {
                context,
                generation: 0,
            },
        );
        self.next_handle += 1;
        self.source_bytes = retained;
        Ok(Response::Open {
            document: handle,
            pages,
        })
    }

    pub(super) fn close(&mut self, handle: u32) -> Result<Response> {
        let slot = self.slots.remove(&handle).ok_or_else(|| unknown(HANDLE))?;
        self.lists.close(handle);
        self.texts.close(handle);
        let released = slot.context.document().original_bytes().len();
        self.source_bytes = self.source_bytes.saturating_sub(released);
        Ok(Response::Close)
    }

    /// The document of `handle`, which a request names at body offset `at`.
    pub(super) fn document(&self, handle: u32, at: usize) -> Result<&Document> {
        self.slots
            .get(&handle)
            .map(|slot| slot.context.document())
            .ok_or_else(|| unknown(at))
    }

    /// The document's rendering context and the page; the request names the
    /// document at body offset `at` and the page four bytes later.
    pub(super) fn page(
        &self,
        document: u32,
        page: u32,
        at: usize,
    ) -> Result<(&RenderContext, &Page)> {
        let context = &self
            .slots
            .get(&document)
            .ok_or_else(|| unknown(at))?
            .context;
        let found = context
            .document()
            .page(index(page)?)
            .map_err(|error| error.at(Position::Buffer(at + 4)))?;
        Ok((context, found))
    }

    pub(super) fn page_info(&self, document: u32, page: u32) -> Result<Response> {
        let (_, page) = self.page(document, page, HANDLE)?;
        let crop = page.crop_box();
        Ok(Response::PageInfo {
            crop: [crop.x0(), crop.y0(), crop.x1(), crop.y1()],
            rotation: page.rotation().degrees(),
            user_unit: page.user_unit(),
            size: pagura::device_size(page, &RenderOptions::default())?,
        })
    }

    /// The page's compiled list from the cache, or compiled now and retained
    /// if `admit` still agrees once compilation has finished.
    pub(super) fn compiled(
        &mut self,
        document: u32,
        page: u32,
        at: usize,
        admit: impl FnOnce() -> bool,
    ) -> Result<Arc<DisplayList>> {
        let key = PageKey { document, page };
        if let Some(list) = self.lists.get(key) {
            return Ok(list);
        }
        let (context, found) = self.page(document, page, at)?;
        let list = Arc::new(context.compile_page(found)?);
        if admit()
            && let Some(charge) = cache::list_charge(&list)
        {
            self.lists.insert(key, Arc::clone(&list), charge);
        }
        Ok(list)
    }

    /// The page's extracted text from the cache, or extracted and retained now.
    pub(super) fn text(&mut self, document: u32, page: u32, at: usize) -> Result<Arc<TextPage>> {
        let key = PageKey { document, page };
        if let Some(text) = self.texts.get(key) {
            return Ok(text);
        }
        let (context, found) = self.page(document, page, at)?;
        let text = Arc::new(context.text_page(found)?);
        self.texts
            .insert(key, Arc::clone(&text), cache::text_charge(&text));
        Ok(text)
    }

    /// Selects a newer viewport generation (1.1); repeating one is idempotent.
    pub(super) fn select_generation(&mut self, document: u32, generation: u32) -> Result<Response> {
        let slot = self
            .slots
            .get_mut(&document)
            .ok_or_else(|| unknown(HANDLE))?;
        if generation == 0 || generation < slot.generation {
            return Err(Error::invalid_argument("viewport generation must increase")
                .at(Position::Buffer(HANDLE + 4)));
        }
        slot.generation = generation;
        Ok(Response::Generation(generation))
    }

    /// The selected generation of a document named at body offset `at`, zero
    /// before the first.
    pub(super) fn generation(&self, document: u32, at: usize) -> Result<u32> {
        self.slots
            .get(&document)
            .map(|slot| slot.generation)
            .ok_or_else(|| unknown(at))
    }

    /// Repairs and information discovered so far, in discovery order.
    pub(super) fn diagnostics(&self, handle: u32) -> Result<Vec<Diagnostic>> {
        self.document(handle, HANDLE).map(Document::diagnostics)
    }

    pub(super) fn stats(&self) -> SessionStats {
        let (charged, lists) = self.lists.accounting();
        SessionStats {
            source_bytes: u64::try_from(self.source_bytes).unwrap_or(u64::MAX),
            cache_bytes: u64::try_from(charged).unwrap_or(u64::MAX),
            documents: u32::try_from(self.slots.len()).unwrap_or(u32::MAX),
            lists: u32::try_from(lists).unwrap_or(u32::MAX),
        }
    }

    /// Text-cache charges and retained pages.
    pub(super) fn text_accounting(&self) -> (usize, usize) {
        self.texts.accounting()
    }

    #[cfg(test)]
    pub(super) fn cached_list(&mut self, document: u32, page: u32) -> Option<Arc<DisplayList>> {
        self.lists.get(PageKey { document, page })
    }
}

/// A page index, which every supported target can address.
pub(super) fn index(value: u32) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::invalid_argument("page index exceeds address space"))
}

fn unknown(at: usize) -> Error {
    Error::invalid_argument("unknown document handle").at(Position::Buffer(at))
}
