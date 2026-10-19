//! Session lifetime, document budgets and transport boundaries.

use crate::cache::PageCache;
use crate::protocol::{self, Request, Response};
use pagura::{Document, Error, Limits, OpenOptions, Position, RenderOptions, Result};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::sync::Arc;

/// Why a session stopped. Operational PDF errors do not stop a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// EOF exactly between frames.
    Eof,
    /// One terminal protocol error was emitted.
    ProtocolError,
}

/// One synchronous session with immutable documents and monotonically assigned handles.
#[derive(Debug)]
pub struct Session {
    limits: Limits,
    documents: BTreeMap<u32, pagura::RenderContext>,
    retained_bytes: usize,
    next_handle: u32,
    last_request: u32,
    version: Option<u32>,
    generations: BTreeMap<u32, u32>,
    cache: PageCache,
    cancellation: crate::Cancellation,
    text_cache: PageCache<pagura::TextPage>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Session {
    /// Creates a session. Aggregate source bytes are capped by `file_bytes`,
    /// open-document count by `objects`, and tile pixels by `pixels`.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            cancellation: crate::Cancellation::default(),
            text_cache: PageCache::new(limits.document_bytes.min(16 * 1024 * 1024)),
            documents: BTreeMap::new(),
            retained_bytes: 0,
            next_handle: 1,
            last_request: 0,
            version: None,
            generations: BTreeMap::new(),
            cache: PageCache::new(limits.decoded_bytes.min(64 * 1024 * 1024)),
        }
    }

    /// Set the compiled-page cache accounting budget; zero disables retention.
    /// At most eight lists are retained. Charges estimate command and geometry
    /// storage; they are not a total heap-memory measurement.
    #[must_use]
    pub fn with_cache_bytes(mut self, bytes: usize) -> Self {
        self.cache = PageCache::new(bytes);
        self
    }

    /// Set the independent text-cache estimate budget; zero disables retention.
    #[must_use]
    pub fn with_text_cache_bytes(mut self, bytes: usize) -> Self {
        self.text_cache = PageCache::new(bytes);
        self
    }

    /// Attach a native supervisor flag. Generated tiles observe it before and
    /// after compilation/painting; executing engine calls still run to completion.
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: crate::Cancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Snapshot source and compiled-page retention, excluding document internals,
    /// temporary allocations, allocator overhead and transport buffers.
    pub fn stats(&self) -> protocol::SessionStats {
        let (charged, lists) = self.cache.accounting();
        protocol::SessionStats {
            source_bytes: u64::try_from(self.retained_bytes).unwrap_or(u64::MAX),
            cache_bytes: u64::try_from(charged).unwrap_or(u64::MAX),
            documents: u32::try_from(self.documents.len()).unwrap_or(u32::MAX),
            lists: u32::try_from(lists).unwrap_or(u32::MAX),
        }
    }

    /// Maximum body size, derived from the configured input, password, decoded
    /// output and pixel budgets, and representable in a v1 frame.
    pub fn max_frame_bytes(&self) -> usize {
        self.limits
            .file_bytes
            .saturating_add(self.limits.string_bytes)
            .saturating_add(14)
            .max(self.limits.pixels.saturating_mul(4).saturating_add(18))
            .max(self.limits.decoded_bytes)
            .max(1024)
            .min(u32::MAX as usize)
    }

    /// Serve until EOF or a malformed/out-of-order frame. Every response is
    /// flushed, including the one terminal structured protocol error.
    ///
    /// # Errors
    /// Returns transport I/O failures; malformed input is instead a terminal
    /// error frame and `SessionEnd::ProtocolError`. EOF inside a frame is malformed.
    /// Consumes the session, releasing all documents even after an I/O failure.
    pub fn serve(
        mut self,
        input: &mut impl Read,
        output: &mut impl Write,
    ) -> io::Result<SessionEnd> {
        loop {
            let mut header = [0; 4];
            match input.read(&mut header[..1])? {
                0 => return Ok(SessionEnd::Eof),
                _ => {
                    if !read_exact(input, &mut header[1..])? {
                        return self.fail(
                            output,
                            0,
                            Error::syntax("truncated frame length").at(Position::Buffer(0)),
                        );
                    }
                }
            }
            let length = usize::try_from(u32::from_le_bytes(header)).unwrap_or(usize::MAX);
            if length < 5 {
                return self.fail(
                    output,
                    0,
                    Error::syntax("frame body is shorter than header").at(Position::Buffer(0)),
                );
            }
            if length > self.max_frame_bytes() {
                return self.fail(
                    output,
                    0,
                    Error::limit("request frame budget exceeded").at(Position::Buffer(0)),
                );
            }
            let mut body = Vec::new();
            if body.try_reserve_exact(length).is_err() {
                return self.fail(output, 0, Error::limit("request allocation failed"));
            }
            body.resize(length, 0);
            if !read_exact(input, &mut body)? {
                return self.fail(
                    output,
                    0,
                    Error::syntax("truncated frame body").at(Position::Buffer(0)),
                );
            }
            let id = body
                .get(1..5)
                .and_then(|bytes| bytes.try_into().ok())
                .map_or(0, u32::from_le_bytes);
            let frame = match protocol::decode(&body, self.limits) {
                Ok(frame) => frame,
                Err(error) => return self.fail(output, id, error),
            };
            if let Err(error) = self.order(frame.id, &frame.request) {
                return self.fail(output, frame.id, error);
            }
            let response = self.execute(frame.request).unwrap_or_else(Response::Error);
            // The document owns its source now; release a potentially large
            // request buffer before allocating the encoded response.
            drop(body);
            self.send(output, id, &response)?;
        }
    }

    fn order(&mut self, id: u32, request: &Request<'_>) -> Result<()> {
        if id == 0 || id <= self.last_request {
            return Err(Error::invalid_argument(
                "request identifiers must increase and be nonzero",
            )
            .at(Position::Buffer(1)));
        }
        match request {
            Request::Hello(version)
                if self.version.is_none()
                    && matches!(
                        *version,
                        protocol::VERSION
                            | protocol::VERSION_1_1
                            | protocol::VERSION_1_2
                            | protocol::VERSION_1_3
                            | protocol::VERSION_1_4
                            | protocol::VERSION_1_5
                            | protocol::VERSION_1_6
                            | protocol::VERSION_1_7
                    ) =>
            {
                self.version = Some(*version);
            }
            Request::Hello(_) => {
                return Err(
                    Error::invalid_argument("invalid or repeated protocol handshake")
                        .at(Position::Buffer(5)),
                );
            }
            _ if self.version.is_none() => {
                return Err(Error::invalid_argument("hello must be the first request")
                    .at(Position::Buffer(0)));
            }
            _ => (),
        }
        self.last_request = id;
        Ok(())
    }

    fn execute(&mut self, request: Request<'_>) -> Result<Response> {
        match request {
            Request::Hello(version) => Ok(Response::Hello(version)),
            Request::FormFields(_)
            | Request::SaveForm { .. }
            | Request::FormWidgets { .. }
            | Request::FormStyles { .. } => self.editing(request),
            Request::Reading(request) => self.reading(request).map(Response::Reading),
            Request::Stats => {
                self.require_extensions()?;
                Ok(Response::Stats(self.stats()))
            }
            Request::Generation {
                document,
                generation,
            } => {
                self.require_extensions()?;
                self.document(document)?;
                let current = self.generations.entry(document).or_default();
                if generation == 0 || generation < *current {
                    return Err(Error::invalid_argument("viewport generation must increase")
                        .at(Position::Buffer(9)));
                }
                *current = generation;
                Ok(Response::Generation(generation))
            }
            Request::GenerationTile {
                generation,
                document,
                page,
                scale,
                tile,
            } => {
                self.require_extensions()?;
                self.document_at(document, 9)?;
                let current = self.generations.get(&document).copied().unwrap_or(0);
                if generation < current {
                    return Ok(Response::Skipped(generation));
                }
                if generation == 0 || generation != current {
                    return Err(
                        Error::invalid_argument("viewport generation was not selected")
                            .at(Position::Buffer(5)),
                    );
                }
                if self.cancellation.is_cancelled() {
                    return Ok(Response::Skipped(generation));
                }
                let response = self.render(document, page, scale, tile, Some(generation))?;
                if self.cancellation.is_cancelled() {
                    return Ok(Response::Skipped(generation));
                }
                match response {
                    Response::Tile {
                        width,
                        height,
                        rgba,
                    } => Ok(Response::GenerationTile {
                        generation,
                        width,
                        height,
                        rgba,
                    }),
                    response => Ok(response),
                }
            }
            Request::Open {
                bytes,
                recovery,
                password,
            } => self.open(bytes, recovery, password),
            Request::PageInfo { document, page } => self.page_info(document, page),
            Request::RenderTile {
                document,
                page,
                scale,
                tile,
            } => self.render(document, page, scale, tile, None),
            Request::Diagnostics(handle) => {
                Ok(Response::Diagnostics(self.document(handle)?.diagnostics()))
            }
            Request::DiagnosticDetails(handle) => {
                if !matches!(
                    self.version,
                    Some(
                        protocol::VERSION_1_4
                            | protocol::VERSION_1_5
                            | protocol::VERSION_1_6
                            | protocol::VERSION_1_7
                    )
                ) {
                    return Err(Error::invalid_argument(
                        "typed diagnostics require protocol 1.4",
                    ));
                }
                Ok(Response::DiagnosticDetails(
                    self.document(handle)?.diagnostics(),
                ))
            }
            Request::Close(handle) => self.close(handle),
        }
    }

    fn editing(&self, request: Request<'_>) -> Result<Response> {
        self.require_editing()?;
        match request {
            Request::FormFields(handle) => {
                crate::editing::fields(self.document(handle)?).map(Response::FormFields)
            }
            Request::SaveForm { document, changes } => {
                crate::editing::save(self.document(document)?, changes).map(Response::SavedForm)
            }
            Request::FormStyles { document, page }
                if self.version == Some(protocol::VERSION_1_7) =>
            {
                crate::editing::styles(self.document(document)?, page).map(Response::FormStyles)
            }
            Request::FormWidgets { document, page }
                if matches!(
                    self.version,
                    Some(protocol::VERSION_1_6 | protocol::VERSION_1_7)
                ) =>
            {
                crate::editing::widgets(self.document(document)?, page).map(Response::FormWidgets)
            }
            _ => Err(Error::invalid_argument(
                "inline form editing requires protocol 1.6",
            )),
        }
    }

    fn page_info(&self, document: u32, page: u32) -> Result<Response> {
        let page = self
            .document(document)?
            .page(index(page)?)
            .map_err(|error| error.at(Position::Buffer(9)))?;
        let crop = page.crop_box();
        Ok(Response::PageInfo {
            crop: [crop.x0(), crop.y0(), crop.x1(), crop.y1()],
            rotation: page.rotation().degrees(),
            user_unit: page.user_unit(),
            size: pagura::device_size(page, &RenderOptions::default())?,
        })
    }

    fn close(&mut self, handle: u32) -> Result<Response> {
        let document = self
            .documents
            .remove(&handle)
            .ok_or(Error::invalid_argument("unknown document handle").at(Position::Buffer(5)))?;
        self.cache.close(handle);
        self.text_cache.close(handle);
        self.generations.remove(&handle);
        self.retained_bytes = self
            .retained_bytes
            .saturating_sub(document.document().original_bytes().len());
        Ok(Response::Close)
    }

    fn render(
        &mut self,
        document: u32,
        page: u32,
        scale: f64,
        tile: pagura::Tile,
        generation: Option<u32>,
    ) -> Result<Response> {
        let offset = if generation.is_some() { 4 } else { 0 };
        // Bound response storage before rendering or allocating straight pixels.
        let pixel_bytes = u64::from(tile.width) * u64::from(tile.height);
        if pixel_bytes > u64::try_from(self.limits.pixels).unwrap_or(u64::MAX)
            || pixel_bytes
                .checked_mul(4)
                .and_then(|n| n.checked_add(13))
                .is_none_or(|n| n > u64::try_from(self.max_frame_bytes()).unwrap_or(u64::MAX))
        {
            return Err(Error::limit("tile response budget exceeded"));
        }
        let key = (document, page);
        let cached = self.cache.get(key);
        let document = self.document_at(document, 5 + offset)?;
        let page = document
            .page(index(page)?)
            .map_err(|error| error.at(Position::Buffer(9 + offset)))?;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Error::invalid_argument("scale must be positive and finite")
                .at(Position::Buffer(13 + offset)));
        }
        let options = RenderOptions {
            scale,
            ..RenderOptions::default()
        };
        // Validate geometry before compilation or cache admission.
        let (width, height) = pagura::device_size(page, &options)?;
        if tile.width == 0
            || tile.height == 0
            || tile.x.checked_add(tile.width).is_none_or(|n| n > width)
            || tile.y.checked_add(tile.height).is_none_or(|n| n > height)
        {
            return Err(Error::invalid_argument("tile lies outside the page")
                .at(Position::Buffer(21 + offset)));
        }
        let list = if let Some(list) = cached {
            list
        } else {
            let list = Arc::new(
                self.documents
                    .get(&key.0)
                    .ok_or(Error::invalid_argument("unknown document handle"))?
                    .compile_page(page)?,
            );
            if let Some(response) = self.cancelled(generation) {
                return Ok(response);
            }
            self.cache.insert(key, Arc::clone(&list));
            list
        };
        if let Some(response) = self.cancelled(generation) {
            return Ok(response);
        }
        let pixmap = list.render_tile(&options, tile)?;
        if let Some(response) = self.cancelled(generation) {
            return Ok(response);
        }
        Ok(Response::Tile {
            width: pixmap.width(),
            height: pixmap.height(),
            rgba: pixmap.to_straight_rgba(),
        })
    }

    fn open(
        &mut self,
        bytes: &[u8],
        recovery: pagura::Recovery,
        password: &[u8],
    ) -> Result<Response> {
        let retained = self
            .retained_bytes
            .checked_add(bytes.len())
            .filter(|&size| size <= self.limits.file_bytes)
            .ok_or(Error::limit("session source byte budget exceeded"))?;
        if self.documents.len() >= self.limits.objects || self.next_handle == u32::MAX {
            return Err(Error::limit("session document handle budget exceeded"));
        }
        let options = OpenOptions {
            limits: self.limits,
            recovery,
            password: password.to_vec(),
        };
        let document = Document::open(bytes, &options)?;
        let pages = u32::try_from(document.page_count()?)
            .map_err(|_| Error::limit("page count exceeds protocol range"))?;
        let handle = self.next_handle;
        self.documents
            .insert(handle, pagura::render_context(Arc::new(document)));
        self.next_handle += 1;
        self.retained_bytes = retained;
        Ok(Response::Open {
            document: handle,
            pages,
        })
    }

    fn extracted(&mut self, document: u32, page: u32) -> Result<Arc<pagura::TextPage>> {
        let key = (document, page);
        if let Some(text) = self.text_cache.get(key) {
            return Ok(text);
        }
        let context = self
            .documents
            .get(&document)
            .ok_or(Error::invalid_argument("unknown document handle").at(Position::Buffer(5)))?;
        let text = Arc::new(
            context.text_page(
                context
                    .document()
                    .page(index(page)?)
                    .map_err(|error| error.at(Position::Buffer(9)))?,
            )?,
        );
        self.text_cache
            .insert_charged(key, Arc::clone(&text), crate::reading::text_charge(&text));
        Ok(text)
    }

    fn reading(
        &mut self,
        request: crate::reading::Request<'_>,
    ) -> Result<crate::reading::Response> {
        use crate::reading::{Request, Response, Writer};
        if matches!(request, Request::Bounds(..))
            && !matches!(
                self.version,
                Some(
                    protocol::VERSION_1_3
                        | protocol::VERSION_1_4
                        | protocol::VERSION_1_5
                        | protocol::VERSION_1_6
                        | protocol::VERSION_1_7
                )
            )
        {
            return Err(Error::invalid_argument(
                "content bounds require protocol 1.3",
            ));
        }
        if !matches!(
            self.version,
            Some(
                protocol::VERSION_1_2
                    | protocol::VERSION_1_3
                    | protocol::VERSION_1_4
                    | protocol::VERSION_1_5
                    | protocol::VERSION_1_6
                    | protocol::VERSION_1_7
            )
        ) {
            return Err(Error::invalid_argument(
                "reading request requires protocol 1.2",
            ));
        }
        let cap = self.limits.decoded_bytes.min(32 * 1024 * 1024);
        match request {
            Request::Bounds(doc, page) => {
                let key = (doc, page);
                let list = if let Some(list) = self.cache.get(key) {
                    list
                } else {
                    let context = self.documents.get(&doc).ok_or(
                        Error::invalid_argument("unknown document handle").at(Position::Buffer(5)),
                    )?;
                    let page = context
                        .document()
                        .page(index(page)?)
                        .map_err(|error| error.at(Position::Buffer(9)))?;
                    let list = Arc::new(context.compile_page(page)?);
                    self.cache.insert(key, Arc::clone(&list));
                    list
                };
                let bounds = list.content_bounds()?;
                let mut out = Writer::new(cap);
                out.u8(u8::from(bounds.is_some()))?;
                if let Some(bounds) = bounds {
                    for value in [bounds.x0(), bounds.y0(), bounds.x1(), bounds.y1()] {
                        out.f64(value)?;
                    }
                }
                Ok(Response::new(144, out))
            }
            Request::Text(doc, page) => {
                crate::reading::page_text(self.extracted(doc, page)?.as_ref(), cap)
            }
            Request::Search(search) => self.search(search, cap),
            Request::Links(doc, page) => crate::reading::links(self.document(doc)?, page, cap),
            Request::Outlines(doc) => crate::reading::outlines(self.document(doc)?, cap),
            Request::Labels(doc) => crate::reading::labels(self.document(doc)?, cap),
            Request::Stats => {
                let stats = self.stats();
                let (text_bytes, text_pages) = self.text_cache.accounting();
                let mut out = Writer::new(cap);
                out.u64(stats.source_bytes)?;
                out.u64(stats.cache_bytes)?;
                out.u32(stats.documents)?;
                out.u32(stats.lists)?;
                out.u64(u64::try_from(text_bytes).unwrap_or(u64::MAX))?;
                out.count(text_pages)?;
                Ok(Response::new(143, out))
            }
        }
    }

    fn search(
        &mut self,
        search: crate::reading::Search<'_>,
        cap: usize,
    ) -> Result<crate::reading::Response> {
        use crate::reading::{Response, Writer};
        let options = search.validate(self.document(search.document)?.page_count()?)?;
        let mut out = Writer::new(cap);
        out.push(&[0; 13])?;
        let mut count = 0;
        for page in search.start..search.end {
            let text = self.extracted(search.document, page)?;
            let matches = text.find(search.query, options)?;
            let offset = if page == search.start {
                index(search.offset)?
            } else {
                0
            };
            if offset > matches.len() {
                return Err(
                    Error::invalid_argument("search match cursor outside page results")
                        .at(Position::Buffer(17)),
                );
            }
            for (ordinal, found) in matches.iter().enumerate().skip(offset) {
                out.u32(page)?;
                out.count(found.range.start)?;
                out.count(found.range.end)?;
                out.count(found.quads.len())?;
                for quad in &found.quads {
                    for point in quad.0 {
                        out.f64(point.x)?;
                        out.f64(point.y)?;
                    }
                }
                count += 1;
                if count == search.limit {
                    let (next, offset) = if ordinal + 1 == matches.len() {
                        (page + 1, 0)
                    } else {
                        (page, ordinal + 1)
                    };
                    out.cursor(
                        next,
                        u32::try_from(offset)
                            .map_err(|_| Error::limit("search match cursor exceeds u32"))?,
                        next == search.end,
                        count,
                    )?;
                    return Ok(Response::new(139, out));
                }
            }
        }
        out.cursor(search.end, 0, true, count)?;
        Ok(Response::new(139, out))
    }

    fn cancelled(&self, generation: Option<u32>) -> Option<Response> {
        generation
            .filter(|_| self.cancellation.is_cancelled())
            .map(Response::Skipped)
    }

    fn require_editing(&self) -> Result<()> {
        if matches!(
            self.version,
            Some(protocol::VERSION_1_5 | protocol::VERSION_1_6 | protocol::VERSION_1_7)
        ) {
            Ok(())
        } else {
            Err(Error::invalid_argument(
                "form editing requires protocol 1.5",
            ))
        }
    }

    fn require_extensions(&self) -> Result<()> {
        if matches!(
            self.version,
            Some(
                protocol::VERSION_1_1
                    | protocol::VERSION_1_2
                    | protocol::VERSION_1_3
                    | protocol::VERSION_1_4
                    | protocol::VERSION_1_5
                    | protocol::VERSION_1_6
                    | protocol::VERSION_1_7
            )
        ) {
            Ok(())
        } else {
            Err(Error::invalid_argument("request requires protocol 1.1"))
        }
    }

    fn document(&self, handle: u32) -> Result<&Document> {
        self.document_at(handle, 5)
    }

    fn document_at(&self, handle: u32, offset: usize) -> Result<&Document> {
        self.documents
            .get(&handle)
            .map(pagura::RenderContext::document)
            .ok_or(Error::invalid_argument("unknown document handle").at(Position::Buffer(offset)))
    }

    fn send(&self, output: &mut impl Write, id: u32, response: &Response) -> io::Result<()> {
        let bytes = protocol::encode(id, response, self.max_frame_bytes())
            .or_else(|error| protocol::encode(id, &Response::Error(error), self.max_frame_bytes()))
            .map_err(io::Error::other)?;
        output.write_all(&bytes)?;
        output.flush()
    }

    fn fail(&self, output: &mut impl Write, id: u32, error: Error) -> io::Result<SessionEnd> {
        self.send(output, id, &Response::Error(error))?;
        Ok(SessionEnd::ProtocolError)
    }
}

fn index(value: u32) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::invalid_argument("page index exceeds address space"))
}

fn read_exact(input: &mut impl Read, bytes: &mut [u8]) -> io::Result<bool> {
    match input.read_exact(bytes) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
