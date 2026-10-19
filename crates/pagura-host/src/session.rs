//! Session lifetime, document budgets and transport boundaries.

use crate::cache::PageCache;
use crate::protocol::{self, Request, Response};
use pagura::{Document, Error, Limits, OpenOptions, Password, Position, RenderOptions, Result};
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
            Request::PageInfo { document, page } => {
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
                if self.version != Some(protocol::VERSION_1_4) {
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
            password: Password::bytes(password),
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
                Some(protocol::VERSION_1_3 | protocol::VERSION_1_4)
            )
        {
            return Err(Error::invalid_argument(
                "content bounds require protocol 1.3",
            ));
        }
        if !matches!(
            self.version,
            Some(protocol::VERSION_1_2 | protocol::VERSION_1_3 | protocol::VERSION_1_4)
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

    fn require_extensions(&self) -> Result<()> {
        if matches!(
            self.version,
            Some(
                protocol::VERSION_1_1
                    | protocol::VERSION_1_2
                    | protocol::VERSION_1_3
                    | protocol::VERSION_1_4
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
mod tests {
    use super::*;
    use pagura_testkit::fixture;

    fn frame(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = u32::try_from(payload.len() + 5)
            .unwrap()
            .to_le_bytes()
            .to_vec();
        bytes.push(kind);
        bytes.extend(id.to_le_bytes());
        bytes.extend(payload);
        bytes
    }
    fn hello() -> Vec<u8> {
        frame(1, 1, &1_u32.to_le_bytes())
    }
    fn open(id: u32, file: &str, strict: bool) -> Vec<u8> {
        let pdf = fixture("classic", file);
        let mut payload = vec![u8::from(!strict), 0, 0, 0, 0];
        payload.extend(u32::try_from(pdf.len()).unwrap().to_le_bytes());
        payload.extend(pdf);
        frame(2, id, &payload)
    }
    fn run(input: &[u8], limits: Limits) -> (SessionEnd, Vec<Vec<u8>>) {
        let mut bytes = Vec::new();
        let end = Session::new(limits)
            .serve(&mut &*input, &mut bytes)
            .unwrap();
        let mut frames = Vec::new();
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            let size = u32::from_le_bytes(remaining[..4].try_into().unwrap()) as usize;
            frames.push(remaining[4..4 + size].to_vec());
            remaining = &remaining[4 + size..];
        }
        (end, frames)
    }

    #[test]
    fn malformed_and_out_of_order_frames_are_terminal() {
        // host-frame-bounds and host-session-order: the following valid hello
        // must never execute after a terminal failure.
        let limits = Limits {
            file_bytes: 1024,
            string_bytes: 8,
            decoded_bytes: 1024,
            pixels: 16,
            ..Limits::default()
        };
        for mut bad in [
            vec![0; 4],
            vec![255; 4],
            frame(0, 2, &[]),
            frame(6, 2, &[0; 4]),
            frame(1, 0, &[1, 0, 0, 0]),
            frame(1, 2, &[2, 0, 0, 0]),
        ] {
            bad.extend(hello());
            let (end, output) = run(&bad, limits);
            assert_eq!(end, SessionEnd::ProtocolError);
            assert_eq!(output.len(), 1);
            assert_eq!(output[0][0], 255);
        }
        for last in [hello(), frame(5, 1, &[0; 4]), frame(5, 0, &[0; 4])] {
            let mut input = hello();
            input.extend(last);
            input.extend(frame(5, 3, &[0; 4]));
            let (end, output) = run(&input, limits);
            assert_eq!(end, SessionEnd::ProtocolError);
            assert_eq!(output.len(), 2);
        }
        let valid = hello();
        for cut in 1..valid.len() {
            let (end, output) = run(&valid[..cut], limits);
            assert_eq!(end, SessionEnd::ProtocolError);
            assert_eq!(output[0][0], 255);
        }
        assert_eq!(run(&[], limits), (SessionEnd::Eof, vec![]));
    }

    #[test]
    fn deterministic_fuzz_frames_never_panic() {
        let limits = Limits {
            file_bytes: 128,
            string_bytes: 128,
            decoded_bytes: 128,
            pixels: 1,
            ..Limits::default()
        };
        let mut state = 0x9384_ace1_u32;
        for size in 0..2048 {
            let mut input = Vec::new();
            for _ in 0..size % 257 {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                input.push(state.to_le_bytes()[0]);
            }
            // Half are length-valid to reach the payload decoder.
            if size % 2 == 0 {
                input = frame(u8::try_from(size % 256).unwrap(), 2, &input);
            }
            input.splice(0..0, hello());
            let _ = run(&input, limits);
        }
    }

    #[test]
    fn handles_budgets_errors_and_repairs_preserve_the_session() {
        let mut input = hello();
        input.extend(open(2, "trailing-bytes.pdf", true)); // strict open fails
        input.extend(open(3, "trailing-bytes.pdf", false)); // successful handle 1
        input.extend(frame(5, 4, &1_u32.to_le_bytes()));
        input.extend(frame(6, 5, &1_u32.to_le_bytes()));
        input.extend(frame(5, 6, &1_u32.to_le_bytes())); // stale handle
        input.extend(open(7, "blank.pdf", true)); // handle 2
        input.extend(frame(3, 8, &[2, 0, 0, 0, 0, 0, 0, 0]));
        input.extend(frame(3, 9, &[2, 0, 0, 0, 255, 255, 255, 255]));
        let (end, responses) = run(&input, Limits::default());
        assert_eq!(end, SessionEnd::Eof);
        assert_eq!(
            responses.iter().map(|r| r[0]).collect::<Vec<_>>(),
            [129, 255, 130, 133, 134, 255, 130, 131, 255]
        );
        // Frozen v1: the strict error has no feature and a refused-repair tag.
        assert_eq!(&responses[1][6..10], &[0; 4]);
        assert_eq!(&responses[1][10..14], &19_u32.to_le_bytes());
        assert_eq!(&responses[1][14..33], b"file.trailing-bytes");
        assert!(
            responses[3]
                .windows(19)
                .any(|s| s == b"file.trailing-bytes")
        );
        assert_eq!(&responses[6][5..9], &2_u32.to_le_bytes());
        let mut input = hello();
        input.extend(open(2, "blank.pdf", true));
        input.extend(open(3, "blank.pdf", true));
        let limits = Limits {
            file_bytes: fixture("classic", "blank.pdf").len(),
            ..Limits::default()
        };
        let (_, responses) = run(&input, limits);
        assert_eq!(responses[1][0], 130);
        assert_eq!(responses[2][5], 2);
    }

    #[test]
    fn bad_render_arguments_are_operational_and_never_return_pixels() {
        let mut input = hello();
        input.extend(open(2, "rectangle.pdf", true));
        for (id, scale, dimensions) in [
            (3, f64::NAN, [0_u32, 0, 1, 1]),
            (4, 1.0, [0, 0, 0, 1]),
            (5, 1.0, [u32::MAX, 0, 1, 1]),
            (6, 1.0, [0, 0, u32::MAX, u32::MAX]),
        ] {
            let mut payload = vec![1, 0, 0, 0, 0, 0, 0, 0];
            payload.extend(scale.to_le_bytes());
            for dimension in dimensions {
                payload.extend(dimension.to_le_bytes());
            }
            input.extend(frame(4, id, &payload));
        }
        input.extend(frame(5, 7, &1_u32.to_le_bytes()));
        let limits = Limits {
            pixels: 64,
            ..Limits::default()
        };
        let (end, responses) = run(&input, limits);
        assert_eq!(end, SessionEnd::Eof);
        assert_eq!(
            responses.iter().map(|r| r[0]).collect::<Vec<_>>(),
            [129, 130, 255, 255, 255, 255, 133]
        );
        assert_eq!(responses[2][5], 7); // InvalidArgument for NaN.
        assert_eq!(responses[3][5], 7); // Empty tile.
        assert_eq!(responses[4][5], 7); // Overflowing tile edge.
        assert_eq!(responses[5][5], 2); // Limit before pixel allocation.
    }

    #[test]
    fn two_pages_have_independent_geometry_and_second_page_pixels() {
        // host-multipage-navigation: page 2's blue square covers device
        // x=[32,64), y=[0,32) on the 64x64 top-left-origin surface.
        let mut input = hello();
        input.extend(open(2, "two-pages.pdf", true));
        for (id, page) in [(3, 0_u32), (4, 1)] {
            let mut payload = 1_u32.to_le_bytes().to_vec();
            payload.extend(page.to_le_bytes());
            input.extend(frame(3, id, &payload));
        }
        let tiles = [(0_u32, 0_u32, 64_u32, 64_u32), (24, 16, 32, 32)];
        for (tile, id) in tiles.iter().zip(5..7) {
            let mut payload = vec![1, 0, 0, 0, 1, 0, 0, 0];
            payload.extend(1_f64.to_le_bytes());
            for dimension in [tile.0, tile.1, tile.2, tile.3] {
                payload.extend(dimension.to_le_bytes());
            }
            input.extend(frame(4, id, &payload));
        }
        let (end, responses) = run(&input, Limits::default());
        assert_eq!(end, SessionEnd::Eof);
        assert_eq!(&responses[1][9..13], &2_u32.to_le_bytes());
        let mut geometry = Vec::new();
        for coordinate in [0_f64, 0., 64., 64.] {
            geometry.extend(coordinate.to_le_bytes());
        }
        geometry.extend(0_u16.to_le_bytes());
        geometry.extend(1_f64.to_le_bytes());
        geometry.extend(64_u32.to_le_bytes());
        geometry.extend(64_u32.to_le_bytes());
        for response in &responses[2..4] {
            assert_eq!(response[0], 131);
            assert_eq!(&response[5..], geometry);
        }
        for ((x, y, width, height), response) in tiles.into_iter().zip(&responses[4..6]) {
            assert_eq!(response[0], 132);
            assert_eq!(&response[5..9], &width.to_le_bytes());
            assert_eq!(&response[9..13], &height.to_le_bytes());
            let mut expected = Vec::new();
            for row in y..y + height {
                for column in x..x + width {
                    expected.extend(if column >= 32 && row < 32 {
                        [0, 0, 255, 255]
                    } else {
                        [255; 4]
                    });
                }
            }
            assert_eq!(&response[13..], expected);
        }
    }

    #[test]
    fn generations_skip_before_compilation_and_cached_tiles_share_one_list() {
        let mut session = Session::default();
        session
            .order(1, &Request::Hello(protocol::VERSION_1_1))
            .unwrap();
        let pdf = fixture("classic", "rectangle.pdf");
        session
            .execute(Request::Open {
                bytes: &pdf,
                recovery: pagura::Recovery::Strict,
                password: &[],
            })
            .unwrap();
        session
            .execute(Request::Generation {
                document: 1,
                generation: 2,
            })
            .unwrap();
        let stale = Request::GenerationTile {
            generation: 1,
            document: 1,
            page: u32::MAX,
            scale: f64::NAN,
            tile: pagura::Tile {
                x: u32::MAX,
                y: 0,
                width: 1,
                height: 1,
            },
        };
        assert_eq!(session.execute(stale).unwrap(), Response::Skipped(1));
        assert!(session.cache.get((1, 0)).is_none());
        let current = Request::GenerationTile {
            generation: 2,
            document: 1,
            page: 0,
            scale: 1.0,
            tile: pagura::Tile {
                x: 0,
                y: 0,
                width: 32,
                height: 32,
            },
        };
        assert!(matches!(
            session.execute(current).unwrap(),
            Response::GenerationTile { .. }
        ));
        let invalid_scale = Request::GenerationTile {
            generation: 2,
            document: 1,
            page: 0,
            scale: f64::NAN,
            tile: pagura::Tile {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        };
        assert_eq!(
            session.execute(invalid_scale).unwrap_err().position,
            Position::Buffer(17)
        );
        let first = session.cache.get((1, 0)).unwrap();
        session.execute(current).unwrap();
        assert!(Arc::ptr_eq(&first, &session.cache.get((1, 0)).unwrap()));
        session.execute(Request::Close(1)).unwrap();
        assert!(session.cache.get((1, 0)).is_none());
        let mut original = Session::default();
        original
            .order(1, &Request::Hello(protocol::VERSION))
            .unwrap();
        assert!(
            original
                .execute(Request::Generation {
                    document: 1,
                    generation: 1
                })
                .is_err()
        );
    }

    #[test]
    fn retention_and_supervisor_cancellation_release_on_close() {
        let control = crate::Cancellation::default();
        let mut session = Session::default().with_cancellation(control.clone());
        session
            .order(1, &Request::Hello(protocol::VERSION_1_1))
            .unwrap();
        let pdf = fixture("classic", "rectangle.pdf");
        session
            .execute(Request::Open {
                bytes: &pdf,
                recovery: pagura::Recovery::Strict,
                password: &[],
            })
            .unwrap();
        session
            .execute(Request::Generation {
                document: 1,
                generation: 1,
            })
            .unwrap();
        let tile = Request::GenerationTile {
            generation: 1,
            document: 1,
            page: 0,
            scale: 1.,
            tile: pagura::Tile {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
        };
        std::thread::scope(|scope| {
            scope.spawn(|| control.cancel()).join().unwrap();
        });
        assert_eq!(session.execute(tile).unwrap(), Response::Skipped(1));
        assert_eq!(session.stats().cache_bytes, 0);
        control.reset();
        assert!(matches!(
            session.execute(tile).unwrap(),
            Response::GenerationTile { .. }
        ));
        assert_eq!(
            session.stats(),
            protocol::SessionStats {
                source_bytes: pdf.len() as u64,
                cache_bytes: 4608,
                documents: 1,
                lists: 1
            }
        );
        session.execute(Request::Close(1)).unwrap();
        assert_eq!(
            session.stats(),
            protocol::SessionStats {
                source_bytes: 0,
                cache_bytes: 0,
                documents: 0,
                lists: 0
            }
        );
        let bytes = protocol::encode(7, &Response::Stats(session.stats()), 1024).unwrap();
        assert_eq!(&bytes[..9], &[29, 0, 0, 0, 137, 7, 0, 0, 0]);
        assert_eq!(&bytes[9..], &[0; 24]);
        assert!(protocol::decode(&[9, 1, 0, 0, 0], Limits::default()).is_ok());
        assert!(protocol::decode(&[9, 1, 0, 0, 0, 0], Limits::default()).is_err());
        let mut base = Session::default();
        base.order(1, &Request::Hello(protocol::VERSION)).unwrap();
        assert!(base.execute(Request::Stats).is_err());
    }

    #[test]
    fn classic_tiles_match_independent_ppm_and_page_geometry() {
        let mut input = hello();
        input.extend(open(2, "rectangle.pdf", true));
        input.extend(frame(3, 3, &[1, 0, 0, 0, 0, 0, 0, 0]));
        let mut payload = vec![1, 0, 0, 0, 0, 0, 0, 0];
        payload.extend(1_f64.to_le_bytes());
        for value in [0_u32, 0, 64, 64] {
            payload.extend(value.to_le_bytes());
        }
        input.extend(frame(4, 4, &payload));
        let (_, responses) = run(&input, Limits::default());
        assert_eq!(responses[2][0], 131);
        let mut geometry = Vec::new();
        for coordinate in [0_f64, 0., 64., 64.] {
            geometry.extend(coordinate.to_le_bytes());
        }
        geometry.extend(0_u16.to_le_bytes());
        geometry.extend(1_f64.to_le_bytes());
        geometry.extend(64_u32.to_le_bytes());
        geometry.extend(64_u32.to_le_bytes());
        assert_eq!(&responses[2][5..], geometry);
        assert_eq!(responses[3][0], 132);
        let ppm = fixture("classic", "rectangle.ppm");
        let expected: Vec<_> = ppm[b"P6\n64 64\n255\n".len()..]
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect();
        assert_eq!(&responses[3][13..], expected);
    }
}
