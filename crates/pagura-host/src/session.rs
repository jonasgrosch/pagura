//! Session lifetime, version gating and request dispatch.

mod documents;
mod render;
mod transport;

use crate::config::SessionConfig;
use crate::protocol::{self, Request, Response, SessionStats, layout};
use crate::reading;
use documents::{Documents, HANDLE};
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

/// One synchronous session with immutable documents and monotonically assigned handles.
#[derive(Debug)]
pub struct Session {
    config: SessionConfig,
    version: Option<u32>,
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
    /// session as a whole is bounded by `config.session` (ADR 0032).
    pub fn new(config: SessionConfig) -> Self {
        Self {
            config,
            version: None,
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
            let frame = match protocol::decode(&body, self.config.engine) {
                Ok(frame) => frame,
                Err(error) => return self.fail(&mut output, id, error),
            };
            if let Err(error) = self.order(frame.id, &frame.request) {
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
    fn order(&mut self, id: u32, request: &Request<'_>) -> Result<()> {
        if id == 0 || id <= self.last_request {
            return Err(Error::invalid_argument(
                "request identifiers must increase and be nonzero",
            )
            .at(Position::Buffer(layout::ID_OFFSET)));
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
                // The version is the first payload field.
                return Err(
                    Error::invalid_argument("invalid or repeated protocol handshake")
                        .at(Position::Buffer(layout::HEADER_BYTES)),
                );
            }
            _ if self.version.is_none() => {
                return Err(Error::invalid_argument("hello must be the first request")
                    .at(Position::Buffer(layout::KIND_OFFSET)));
            }
            _ => (),
        }
        self.last_request = id;
        Ok(())
    }

    /// Runs one ordered request; failures are operational error responses.
    fn execute(&mut self, request: Request<'_>) -> Result<Response> {
        match request {
            Request::Hello(version) => Ok(Response::Hello(version)),
            Request::FormFields(_)
            | Request::SaveForm { .. }
            | Request::FormWidgets { .. }
            | Request::FormStyles { .. } => self.editing(request),
            Request::Open {
                bytes,
                recovery,
                password,
            } => self.documents.open(bytes, recovery, password),
            Request::PageInfo { document, page } => self.documents.page_info(document, page),
            Request::RenderTile {
                document,
                page,
                scale,
                tile,
            } => self.render(document, page, scale, tile, None),
            Request::Generation {
                document,
                generation,
            } => {
                self.require_extensions()?;
                self.documents.select_generation(document, generation)
            }
            Request::GenerationTile {
                generation,
                document,
                page,
                scale,
                tile,
            } => {
                self.require_extensions()?;
                self.generation_tile(generation, document, page, scale, tile)
            }
            Request::Diagnostics(handle) => self
                .documents
                .diagnostics(handle)
                .map(Response::Diagnostics),
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
                self.documents
                    .diagnostics(handle)
                    .map(Response::DiagnosticDetails)
            }
            Request::Close(handle) => self.documents.close(handle),
            Request::Stats => {
                self.require_extensions()?;
                Ok(Response::Stats(self.stats()))
            }
            Request::Reading(request) => self.reading(request).map(Response::Reading),
        }
    }

    fn editing(&self, request: Request<'_>) -> Result<Response> {
        self.require_editing()?;
        match request {
            Request::FormFields(handle) => {
                crate::editing::fields(self.documents.document(handle, HANDLE)?)
                    .map(Response::FormFields)
            }
            Request::SaveForm { document, changes } => {
                crate::editing::save(self.documents.document(document, HANDLE)?, changes)
                    .map(Response::SavedForm)
            }
            Request::FormStyles { document, page }
                if self.version == Some(protocol::VERSION_1_7) =>
            {
                crate::editing::styles(self.documents.document(document, HANDLE)?, page)
                    .map(Response::FormStyles)
            }
            Request::FormWidgets { document, page }
                if matches!(
                    self.version,
                    Some(protocol::VERSION_1_6 | protocol::VERSION_1_7)
                ) =>
            {
                crate::editing::widgets(self.documents.document(document, HANDLE)?, page)
                    .map(Response::FormWidgets)
            }
            _ => Err(Error::invalid_argument(
                "inline form editing requires protocol 1.6",
            )),
        }
    }

    fn reading(&mut self, request: reading::Request<'_>) -> Result<reading::Response> {
        if matches!(request, reading::Request::Bounds(..))
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
        let cap = self.config.session.reading_response_bytes;
        match request {
            reading::Request::Text(document, page) => {
                let text = self.documents.text(document, page, HANDLE)?;
                reading::page_text(&text, cap)
            }
            reading::Request::Search(search) => {
                let pages = self
                    .documents
                    .document(search.document, HANDLE)?
                    .page_count()?;
                let options = search.validate(pages)?;
                // Page errors point at the window's start field.
                reading::search(search, options, cap, |page| {
                    self.documents.text(search.document, page, HANDLE)
                })
            }
            reading::Request::Links(document, page) => {
                reading::links(self.documents.document(document, HANDLE)?, page, cap)
            }
            reading::Request::Outlines(document) => {
                reading::outlines(self.documents.document(document, HANDLE)?, cap)
            }
            reading::Request::Labels(document) => {
                reading::labels(self.documents.document(document, HANDLE)?, cap)
            }
            reading::Request::Stats => {
                reading::stats(self.stats(), self.documents.text_accounting(), cap)
            }
            reading::Request::Bounds(document, page) => {
                let list = self.documents.compiled(document, page, HANDLE, || true)?;
                reading::bounds(&list, cap)
            }
        }
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
