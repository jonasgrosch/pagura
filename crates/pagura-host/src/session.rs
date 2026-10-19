//! Session lifetime, version gating and request dispatch.

mod documents;
mod render;
mod transport;

use crate::config::SessionConfig;
use crate::protocol::{self, Request, RequestFrame, Response, SessionStats, Version, layout};
use crate::reading;
use documents::Documents;
use pagura::{Error, Position, Result};
use std::io::{self, Read, Write};
use transport::Incoming;

/// Why a session stopped. Operational PDF errors do not stop a session.
///
/// PROTOCOL.md defines exactly these two ends (exit status 0 and 1), so the
/// enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// EOF exactly between frames.
    Eof,
    /// One terminal protocol error was emitted.
    ProtocolError,
}

/// Where the handshake stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    AwaitingHello,
    Ready(Version),
}

/// One synchronous session with immutable documents and monotonically assigned handles.
#[derive(Debug)]
pub struct Session {
    config: SessionConfig,
    state: State,
    last_request: u32,
    documents: Documents,
    cancellation: crate::Cancellation,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(SessionConfig::default())
    }
}

impl Session {
    /// Creates a session. Each document receives `config.engine`; the
    /// session as a whole is bounded by `config.session` (ADR 0031).
    pub fn new(config: SessionConfig) -> Self {
        Self {
            config,
            state: State::AwaitingHello,
            last_request: 0,
            documents: Documents::new(&config),
            cancellation: crate::Cancellation::default(),
        }
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
    pub fn stats(&self) -> SessionStats {
        self.documents.stats()
    }

    /// Maximum body size of any frame ([`crate::SessionLimits::max_frame_bytes`]).
    pub const fn max_frame_bytes(&self) -> usize {
        self.config.session.max_frame_bytes
    }

    /// Serve until EOF or a malformed/out-of-order frame. Every response is
    /// flushed, including the one terminal structured protocol error.
    ///
    /// # Errors
    /// Returns transport I/O failures; malformed input is instead a terminal
    /// error frame and `SessionEnd::ProtocolError`. EOF inside a frame is malformed.
    /// Consumes the session, releasing all documents even after an I/O failure.
    pub fn serve<R: Read, W: Write>(
        mut self,
        mut input: R,
        mut output: W,
    ) -> io::Result<SessionEnd> {
        loop {
            let body = match transport::read(&mut input, self.max_frame_bytes())? {
                Incoming::Body(body) => body,
                Incoming::Eof => return Ok(SessionEnd::Eof),
                Incoming::Malformed(error) => return self.fail(&mut output, 0, error),
            };
            let id = transport::request_id(&body);
            let frame = match protocol::decode(&body, &self.config.engine) {
                Ok(frame) => frame,
                Err(error) => return self.fail(&mut output, id, error),
            };
            if let Err(error) = self.order(&frame) {
                return self.fail(&mut output, frame.id, error);
            }
            let response = self.execute(frame.request).unwrap_or_else(Response::Error);
            // The document owns its source now; release a potentially large
            // request buffer before allocating the encoded response.
            drop(body);
            self.send(&mut output, id, &response)?;
        }
    }

    /// Checks identifiers and the handshake; failures end the session.
    fn order(&mut self, frame: &RequestFrame<'_>) -> Result<()> {
        if frame.id == 0 || frame.id <= self.last_request {
            return Err(Error::invalid_argument(
                "request identifiers must increase and be nonzero",
            )
            .at(Position::Buffer(layout::ID_OFFSET)));
        }
        match (self.state, frame.request) {
            (State::AwaitingHello, Request::Hello(version)) => {
                let version = Version::try_from(version.get())
                    .map_err(|error| error.at(version.position()))?;
                self.state = State::Ready(version);
            }
            (State::Ready(_), Request::Hello(version)) => {
                return Err(
                    Error::invalid_argument("invalid or repeated protocol handshake")
                        .at(version.position()),
                );
            }
            (State::AwaitingHello, _) => {
                return Err(Error::invalid_argument("hello must be the first request")
                    .at(Position::Buffer(layout::KIND_OFFSET)));
            }
            (State::Ready(_), _) => (),
        }
        self.last_request = frame.id;
        Ok(())
    }

    /// Runs one ordered request; failures are operational error responses.
    fn execute(&mut self, request: Request<'_>) -> Result<Response> {
        let version = self.require(request.kind().since())?;
        match request {
            Request::Hello(_) => Ok(Response::Hello(version)),
            Request::Open {
                bytes,
                recovery,
                password,
            } => self.documents.open(bytes, recovery, password),
            Request::PageInfo(page) => self.documents.page_info(page),
            Request::RenderTile(tile) => self.render(tile, None),
            Request::Generation {
                document,
                generation,
            } => self.documents.select_generation(document, generation),
            Request::GenerationTile { generation, tile } => self.generation_tile(generation, tile),
            Request::Diagnostics(handle) => self
                .documents
                .diagnostics(handle)
                .map(Response::Diagnostics),
            Request::DiagnosticDetails(handle) => self
                .documents
                .diagnostics(handle)
                .map(Response::DiagnosticDetails),
            Request::Close(handle) => self.documents.close(handle),
            Request::Stats => Ok(Response::Stats(self.stats())),
            Request::Reading(request) => self.reading(request).map(Response::Reading),
        }
    }

    /// The negotiated version, if it is at least `minimum`.
    fn require(&self, minimum: Version) -> Result<Version> {
        match self.state {
            State::Ready(version) if version >= minimum => Ok(version),
            _ => Err(minimum.required()),
        }
    }

    fn reading(&mut self, request: reading::Request<'_>) -> Result<reading::Response> {
        let cap = self.config.session.reading_response_bytes;
        match request {
            reading::Request::Text(page) => {
                let text = self.documents.text(page)?;
                reading::page_text(&text, cap)
            }
            reading::Request::Search(search) => {
                let pages = self.documents.document(search.document)?.page_count()?;
                let options = search.validate(pages)?;
                reading::search(&search, options, cap, |page| self.documents.text(page))
            }
            reading::Request::Links(page) => {
                reading::links(self.documents.document(page.document)?, page.page, cap)
            }
            reading::Request::Outlines(handle) => {
                reading::outlines(self.documents.document(handle)?, cap)
            }
            reading::Request::Labels(handle) => {
                reading::labels(self.documents.document(handle)?, cap)
            }
            reading::Request::Stats => {
                reading::stats(self.stats(), self.documents.text_accounting(), cap)
            }
            reading::Request::Bounds(page) => {
                let list = self.documents.compiled(page, || true)?;
                reading::bounds(&list, cap)
            }
        }
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

#[cfg(test)]
mod tests;
