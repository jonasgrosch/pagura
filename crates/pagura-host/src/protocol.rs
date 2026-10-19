//! Protocol v1 wire types and bounded serialization (ADR 0005).

use pagura::{
    Diagnostic, DiagnosticKind, Error, ErrorKind, ObjectId, Position, Recovery, Result, Tile,
};

/// The compatible base session version.
pub const VERSION: u32 = 1;
/// Protocol 1.1, adding viewport generations and stats without changing v1 frames.
pub const VERSION_1_1: u32 = 0x0001_0001;
/// Protocol 1.2, adding reading operations without changing older layouts.
pub const VERSION_1_2: u32 = 0x0001_0002;
/// Protocol 1.3, adding painted content bounds without changing older layouts.
pub const VERSION_1_3: u32 = 0x0001_0003;
/// Protocol 1.4, adding typed diagnostic snapshots without changing older layouts.
pub const VERSION_1_4: u32 = 0x0001_0004;
/// Kind byte for structured error responses.
pub const ERROR_KIND: u8 = 255;

/// A borrowed, fully validated request payload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Request<'a> {
    /// Negotiate a version before accessing documents.
    Hello(u32),
    /// Open immutable PDF bytes, with the chosen recovery policy and raw password bytes.
    Open {
        /// PDF source bytes (ISO 32000-2 clause 7.5).
        bytes: &'a [u8],
        /// Strict validation or reported recovery (ADR 0003).
        recovery: Recovery,
        /// Password bytes, never interpreted as instructions.
        password: &'a [u8],
    },
    /// Describe a zero-based page.
    PageInfo {
        /// Session-local document handle.
        document: u32,
        /// Zero-based page index.
        page: u32,
    },
    /// Render a device-pixel rectangle, onto opaque white.
    RenderTile {
        /// Session-local document handle.
        document: u32,
        /// Zero-based page index.
        page: u32,
        /// Device pixels per default user-space unit.
        scale: f64,
        /// Device-pixel rectangle (ADR 0004).
        tile: Tile,
    },
    /// Select the newest viewport generation for a document (v1.1).
    Generation {
        /// Session-local document handle.
        document: u32,
        /// Monotonically increasing, nonzero generation.
        generation: u32,
    },
    /// A tile which may be skipped before compilation or painting (v1.1).
    GenerationTile {
        /// Viewport generation previously selected for the document.
        generation: u32,
        /// Session-local document handle.
        document: u32,
        /// Zero-based page index.
        page: u32,
        /// Pixels per user-space unit.
        scale: f64,
        /// Device rectangle.
        tile: Tile,
    },
    /// Snapshot repairs discovered so far (all protocol versions).
    Diagnostics(u32),
    /// Snapshot repair and informational diagnostics with their kind (v1.4).
    DiagnosticDetails(u32),
    /// Release a document; handles are never reused.
    Close(u32),
    /// Snapshot session retention accounting (v1.1).
    Stats,
    /// Reading operations, with additions gated by the negotiated version.
    Reading(crate::reading::Request<'a>),
}

/// Session-owned retention accounting, not allocator or total-heap telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStats {
    /// Original input bytes owned by open documents.
    pub source_bytes: u64,
    /// Estimated compiled-list retention charges.
    pub cache_bytes: u64,
    /// Open document count.
    pub documents: u32,
    /// Retained compiled-list count.
    pub lists: u32,
}

/// A request and its nonzero correlation identifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RequestFrame<'a> {
    /// Strictly increasing identifier within the session.
    pub id: u32,
    /// The validated request.
    pub request: Request<'a>,
}

/// A successful response or a structured engine/protocol failure.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// Negotiated version.
    Hello(u32),
    /// New document handle and page count.
    Open {
        /// Nonzero session-local handle.
        document: u32,
        /// Number of pages.
        pages: u32,
    },
    /// Typed page geometry (ISO 32000-2 clause 7.7.3.3).
    PageInfo {
        /// Normalized effective crop box: x0, y0, x1, y1.
        crop: [f64; 4],
        /// Clockwise rotation in degrees, a multiple of 90.
        rotation: u16,
        /// Physical scale of user space.
        user_unit: f64,
        /// Device size at scale 1, including rotation and user unit.
        size: (u32, u32),
    },
    /// Complete straight RGBA8 tile; failures never return partial pixels.
    Tile {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// Row-major, top-left-origin, straight RGBA8 bytes.
        rgba: Vec<u8>,
    },
    /// Selected viewport generation (v1.1).
    Generation(u32),
    /// A stale tile was not compiled or rendered (v1.1).
    Skipped(u32),
    /// Complete pixels for a viewport generation (v1.1).
    GenerationTile {
        /// Viewport generation.
        generation: u32,
        /// Tile width.
        width: u32,
        /// Tile height.
        height: u32,
        /// Straight RGBA8 bytes.
        rgba: Vec<u8>,
    },
    /// Retention accounting (v1.1).
    Stats(SessionStats),
    /// Bounded reading response, encoded by the reading adapter.
    Reading(crate::reading::Response),
    /// Repairs in discovery order.
    Diagnostics(Vec<Diagnostic>),
    /// All diagnostics in discovery order, with explicit kinds (v1.4).
    DiagnosticDetails(Vec<Diagnostic>),
    /// The handle has been released.
    Close,
    /// Structured failure, preserving context from the engine.
    Error(Error),
}

/// Decode a body (excluding the four-byte length). No allocation is needed.
///
/// # Errors
/// Returns positioned `Syntax` for truncated, unknown or noncanonical payloads,
/// or `Limit` for oversized PDF/password byte strings.
pub fn decode(body: &[u8], limits: pagura::Limits) -> Result<RequestFrame<'_>> {
    let mut reader = Reader::new(body);
    let kind = reader.byte()?;
    let id = reader.u32()?;
    let request = match kind {
        1 => Request::Hello(reader.u32()?),
        2 => {
            let recovery = match reader.byte()? {
                0 => Recovery::Strict,
                1 => Recovery::Repair,
                _ => {
                    return Err(Error::syntax("unknown recovery mode")
                        .at(Position::Buffer(reader.offset.saturating_sub(1))));
                }
            };
            let password = reader.blob(limits.string_bytes)?;
            let bytes = reader.blob(limits.file_bytes)?;
            Request::Open {
                bytes,
                recovery,
                password,
            }
        }
        3 => Request::PageInfo {
            document: reader.u32()?,
            page: reader.u32()?,
        },
        4 => Request::RenderTile {
            document: reader.u32()?,
            page: reader.u32()?,
            scale: reader.f64()?,
            tile: Tile {
                x: reader.u32()?,
                y: reader.u32()?,
                width: reader.u32()?,
                height: reader.u32()?,
            },
        },
        5 => Request::Diagnostics(reader.u32()?),
        6 => Request::Close(reader.u32()?),
        7 => Request::Generation {
            document: reader.u32()?,
            generation: reader.u32()?,
        },
        8 => Request::GenerationTile {
            generation: reader.u32()?,
            document: reader.u32()?,
            page: reader.u32()?,
            scale: reader.f64()?,
            tile: Tile {
                x: reader.u32()?,
                y: reader.u32()?,
                width: reader.u32()?,
                height: reader.u32()?,
            },
        },
        9 => Request::Stats,
        17 => Request::DiagnosticDetails(reader.u32()?),
        10..=16 => Request::Reading(crate::reading::decode(kind, &mut reader, limits)?),
        _ => return Err(Error::syntax("unknown request kind").at(Position::Buffer(0))),
    };
    if reader.offset != body.len() {
        return Err(reader.error("trailing request payload"));
    }
    Ok(RequestFrame { id, request })
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(self.offset))
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| self.error("payload offset overflows"))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| self.error("truncated request payload"))?;
        self.offset = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| self.error("truncated request payload"))
    }
    pub(crate) fn byte(&mut self) -> Result<u8> {
        Ok(u8::from_le_bytes(self.array()?))
    }
    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.array()?))
    }
    pub(crate) fn blob(&mut self, cap: usize) -> Result<&'a [u8]> {
        let position = Position::Buffer(self.offset);
        let count = usize::try_from(self.u32()?)
            .map_err(|_| Error::limit("byte string length overflows").at(position))?;
        if count > cap {
            return Err(Error::limit("byte string budget exceeded").at(position));
        }
        self.take(count)
    }
}

/// Encode a response including its length prefix, checking the cap before allocation.
///
/// # Errors
/// Returns `Limit` if the body exceeds `max_body`, the wire's u32 length, or
/// available memory. Invalid tile byte counts return `InvalidArgument`.
pub fn encode(id: u32, response: &Response, max_body: usize) -> Result<Vec<u8>> {
    let mut out = Encoder {
        bytes: Vec::new(),
        cap: max_body.min(u32::MAX as usize).saturating_add(4),
    };
    out.push(&[0; 4])?;
    out.push(&[match response {
        Response::Hello(_) => 129,
        Response::Open { .. } => 130,
        Response::PageInfo { .. } => 131,
        Response::Tile { .. } => 132,
        Response::Diagnostics(_) => 133,
        Response::DiagnosticDetails(_) => 145,
        Response::Close => 134,
        Response::Generation(_) => 135,
        Response::Skipped(_) | Response::GenerationTile { .. } => 136,
        Response::Stats(_) => 137,
        Response::Reading(response) => response.kind,
        Response::Error(_) => ERROR_KIND,
    }])?;
    out.push(&id.to_le_bytes())?;
    encode_payload(&mut out, response)?;
    let length = u32::try_from(out.bytes.len().saturating_sub(4))
        .map_err(|_| Error::limit("response length overflows"))?;
    let prefix = out
        .bytes
        .get_mut(..4)
        .ok_or(Error::limit("response header missing"))?;
    prefix.copy_from_slice(&length.to_le_bytes());
    Ok(out.bytes)
}

fn encode_payload(out: &mut Encoder, response: &Response) -> Result<()> {
    match response {
        Response::Hello(version) => out.push(&version.to_le_bytes())?,
        Response::Open { document, pages } => {
            out.push(&document.to_le_bytes())?;
            out.push(&pages.to_le_bytes())?;
        }
        Response::PageInfo {
            crop,
            rotation,
            user_unit,
            size,
        } => {
            for coordinate in crop {
                out.push(&coordinate.to_le_bytes())?;
            }
            out.push(&rotation.to_le_bytes())?;
            out.push(&user_unit.to_le_bytes())?;
            out.push(&size.0.to_le_bytes())?;
            out.push(&size.1.to_le_bytes())?;
        }
        Response::Tile {
            width,
            height,
            rgba,
        } => {
            encode_tile(out, *width, *height, rgba)?;
        }
        Response::Diagnostics(items) => {
            let repairs = items.iter().filter(|item| item.repair().is_some());
            out.count(repairs.clone().count())?;
            for item in repairs {
                encode_diagnostic(out, item)?;
            }
        }
        Response::DiagnosticDetails(items) => {
            out.count(items.len())?;
            for item in items {
                out.push(&[match item.kind {
                    DiagnosticKind::Repair(_) => 0,
                    DiagnosticKind::Information(_) => 1,
                    // A kind added to pagura-core needs a code in a new
                    // protocol version; until then it cannot be encoded.
                    _ => {
                        return Err(Error::invalid_argument(
                            "diagnostic kind has no wire code in this protocol version",
                        ));
                    }
                }])?;
                encode_diagnostic(out, item)?;
            }
        }
        Response::Generation(generation) => out.push(&generation.to_le_bytes())?,
        Response::Skipped(generation) => {
            out.push(&[0])?;
            out.push(&generation.to_le_bytes())?;
        }
        Response::GenerationTile {
            generation,
            width,
            height,
            rgba,
        } => {
            out.push(&[1])?;
            out.push(&generation.to_le_bytes())?;
            encode_tile(out, *width, *height, rgba)?;
        }
        Response::Stats(stats) => {
            out.push(&stats.source_bytes.to_le_bytes())?;
            out.push(&stats.cache_bytes.to_le_bytes())?;
            out.push(&stats.documents.to_le_bytes())?;
            out.push(&stats.lists.to_le_bytes())?;
        }
        Response::Reading(response) => out.push(&response.payload)?,
        Response::Close => (),
        Response::Error(error) => {
            out.push(&[match error.kind {
                ErrorKind::Syntax => 1,
                ErrorKind::Limit => 2,
                ErrorKind::Unsupported(_) => 3,
                ErrorKind::MissingObject => 4,
                ErrorKind::Cycle => 5,
                ErrorKind::Password => 6,
                ErrorKind::InvalidArgument => 7,
                // A kind added to pagura-core needs a code in a new protocol
                // version; until then it cannot be encoded faithfully.
                _ => {
                    return Err(Error::invalid_argument(
                        "error kind has no wire code in this protocol version",
                    ));
                }
            }])?;
            out.string(error.feature().map_or("", pagura::Feature::as_str))?;
            out.string(error.repair.map_or("", pagura::Repair::as_str))?;
            out.string(error.message)?;
            out.context(error.object, error.position)?;
        }
    }
    Ok(())
}

fn encode_diagnostic(out: &mut Encoder, item: &Diagnostic) -> Result<()> {
    out.string(item.tag())?;
    out.string(item.message)?;
    out.context(item.object, item.position)
}

fn encode_tile(out: &mut Encoder, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let expected = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || expected.checked_mul(4) != u64::try_from(rgba.len()).ok() {
        return Err(Error::invalid_argument(
            "tile byte count does not match dimensions",
        ));
    }
    out.push(&width.to_le_bytes())?;
    out.push(&height.to_le_bytes())?;
    out.push(rgba)
}

struct Encoder {
    bytes: Vec<u8>,
    cap: usize,
}

impl Encoder {
    fn push(&mut self, bytes: &[u8]) -> Result<()> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|size| size > self.cap)
        {
            return Err(Error::limit("response frame budget exceeded"));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| Error::limit("response allocation failed"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn count(&mut self, count: usize) -> Result<()> {
        self.push(
            &u32::try_from(count)
                .map_err(|_| Error::limit("response count overflows"))?
                .to_le_bytes(),
        )
    }
    fn string(&mut self, string: &str) -> Result<()> {
        self.count(string.len())?;
        self.push(string.as_bytes())
    }
    fn object(&mut self, object: ObjectId) -> Result<()> {
        self.push(&object.number.to_le_bytes())?;
        self.push(&object.generation.to_le_bytes())
    }
    fn context(&mut self, object: Option<ObjectId>, position: Position) -> Result<()> {
        self.push(&[u8::from(object.is_some())])?;
        if let Some(object) = object {
            self.object(object)?;
        }
        let (kind, offset, stream) = match position {
            Position::File(offset) => (1, Some(offset), None),
            Position::Stream { object, offset } => (2, Some(offset), Some(object)),
            Position::Buffer(offset) => (3, Some(offset), None),
            // Unknown, or a position kind added to pagura-core that has no
            // code until a protocol version gives it one.
            _ => (0, None, None),
        };
        self.push(&[kind])?;
        if let Some(offset) = offset {
            self.push(
                &u64::try_from(offset)
                    .map_err(|_| Error::limit("position offset overflows"))?
                    .to_le_bytes(),
            )?;
        }
        if let Some(object) = stream {
            self.object(object)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
