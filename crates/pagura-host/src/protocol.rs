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
/// Protocol 1.5, adding form field inspection and atomic filled-form saves.
pub const VERSION_1_5: u32 = 0x0001_0005;
/// Protocol 1.6, adding page-local form widget geometry.
pub const VERSION_1_6: u32 = 0x0001_0006;
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
    /// Inspect bounded form controls (v1.5).
    FormFields(u32),
    /// Inspect editable widgets on a page (v1.6).
    FormWidgets {
        /// Immutable source handle.
        document: u32,
        /// Zero-based page index.
        page: u32,
    },
    /// Apply a batch to a temporary filler and return saved bytes (v1.5).
    SaveForm {
        /// Immutable source handle.
        document: u32,
        /// Bounded editing payload described in PROTOCOL.md.
        changes: &'a [u8],
    },
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
    /// Encoded bounded form controls (v1.5).
    FormFields(Vec<u8>),
    /// Bounded page-local widget geometry.
    FormWidgets(Vec<u8>),
    /// Complete saved PDF bytes; the original handle remains unchanged (v1.5).
    SavedForm(Vec<u8>),
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
        20 => Request::FormWidgets {
            document: reader.u32()?,
            page: reader.u32()?,
        },
        18 => Request::FormFields(reader.u32()?),
        19 => Request::SaveForm {
            document: reader.u32()?,
            changes: reader.blob(limits.document_bytes.min(32 * 1024 * 1024))?,
        },
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
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    pub(crate) fn finish(&self) -> Result<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(self.error("trailing request payload"))
        }
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
        Response::FormFields(_) => 146,
        Response::FormWidgets(_) => 148,
        Response::SavedForm(_) => 147,
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
        Response::FormFields(payload) | Response::FormWidgets(payload) => out.push(payload)?,
        Response::SavedForm(bytes) => {
            out.count(bytes.len())?;
            out.push(bytes)?;
        }
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

pub(crate) struct Encoder {
    bytes: Vec<u8>,
    cap: usize,
}

impl Encoder {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap,
        }
    }
    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<()> {
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
    pub(crate) fn count(&mut self, count: usize) -> Result<()> {
        self.push(
            &u32::try_from(count)
                .map_err(|_| Error::limit("response count overflows"))?
                .to_le_bytes(),
        )
    }
    pub(crate) fn string(&mut self, string: &str) -> Result<()> {
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
mod tests {
    use super::*;
    use pagura::{Feature, Limits, Repair};

    #[test]
    fn hello_has_an_independent_literal_wire_oracle() {
        // host-wire-v1: length=9, response kind=129, id=7, version=1.
        assert_eq!(
            encode(7, &Response::Hello(1), 9).unwrap(),
            [9, 0, 0, 0, 129, 7, 0, 0, 0, 1, 0, 0, 0]
        );
        assert_eq!(
            decode(&[1, 7, 0, 0, 0, 1, 0, 0, 0], Limits::default()).unwrap(),
            RequestFrame {
                id: 7,
                request: Request::Hello(1)
            }
        );
        assert_eq!(
            encode(7, &Response::Hello(1), 8).unwrap_err().kind,
            ErrorKind::Limit
        );
    }

    #[test]
    fn error_wire_preserves_feature_object_and_stream_position() {
        // host-structured-errors: independently constructed byte layout, not a roundtrip.
        let object = ObjectId::new(42, 3);
        let error = Error::unsupported(Feature::new("operator.Tj"), "text")
            .in_object(object)
            .at(Position::Stream {
                object: ObjectId::new(9, 2),
                offset: 17,
            });
        let actual = encode(5, &Response::Error(error), 1024).unwrap();
        let mut expected = vec![255, 5, 0, 0, 0, 3, 11, 0, 0, 0];
        expected.extend(b"operator.Tj");
        expected.extend([0; 4]); // No refused repair.
        expected.extend([4, 0, 0, 0]);
        expected.extend(b"text");
        expected.extend([
            1, 42, 0, 0, 0, 3, 0, 2, 17, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0,
        ]);
        assert_eq!(&actual[4..], expected);
        let diagnostics = Response::Diagnostics(vec![
            Diagnostic::new(Repair::new("syntax.duplicate-key"), "duplicate")
                .at(Position::File(10))
                .in_object(object),
        ]);
        let bytes = encode(6, &diagnostics, 1024).unwrap();
        assert!(bytes.windows(20).any(|s| s == b"syntax.duplicate-key"));
        for position in [Position::Unknown, Position::File(4), Position::Buffer(5)] {
            assert!(encode(8, &Response::Error(Error::syntax("bad").at(position)), 1024).is_ok());
        }
    }

    #[test]
    fn strict_repair_has_an_independent_frozen_v1_wire_oracle() {
        let error = Diagnostic::new(Repair::new("file.trailing-bytes"), "trailing")
            .in_object(ObjectId::new(3, 0))
            .at(Position::File(17))
            .to_error()
            .unwrap();
        let actual = encode(2, &Response::Error(error), 1024).unwrap();
        // kind/id/category, empty feature tag, refused repair, message, context.
        let mut expected = vec![255, 2, 0, 0, 0, 1, 0, 0, 0, 0, 19, 0, 0, 0];
        expected.extend(b"file.trailing-bytes");
        expected.extend([8, 0, 0, 0]);
        expected.extend(b"trailing");
        expected.extend([1, 3, 0, 0, 0, 0, 0, 1, 17, 0, 0, 0, 0, 0, 0, 0]);
        let mut frame = u32::try_from(expected.len())
            .unwrap()
            .to_le_bytes()
            .to_vec();
        frame.extend(expected);
        assert_eq!(actual, frame);
        assert_eq!(
            encode(2, &Response::Error(Error::syntax("bad")), 1024).unwrap()[10..18],
            [0; 8]
        );
    }

    #[test]
    fn every_error_category_and_context_has_a_fixed_wire_code() {
        let kinds = [
            ErrorKind::Syntax,
            ErrorKind::Limit,
            ErrorKind::Unsupported(Feature::new("operator.Tj")),
            ErrorKind::MissingObject,
            ErrorKind::Cycle,
            ErrorKind::Password,
            ErrorKind::InvalidArgument,
        ];
        for (index, kind) in kinds.into_iter().enumerate() {
            let response = Response::Error(Error::new(kind, "failure"));
            let bytes = encode(9, &response, 1024).unwrap();
            assert_eq!(usize::from(bytes[9]), index + 1);
            assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]); // No object, unknown position.
        }
        let bytes = encode(
            1,
            &Response::Error(Error::syntax("bad").at(Position::Buffer(17))),
            1024,
        )
        .unwrap();
        assert_eq!(&bytes[bytes.len() - 10..], &[0, 3, 17, 0, 0, 0, 0, 0, 0, 0]);
        let bytes = encode(
            1,
            &Response::Error(Error::syntax("bad").at(Position::File(5))),
            1024,
        )
        .unwrap();
        assert_eq!(&bytes[bytes.len() - 10..], &[0, 1, 5, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn generation_layouts_are_additive_and_truncations_fail() {
        assert_eq!(
            encode(9, &Response::Skipped(1), 1024).unwrap(),
            [10, 0, 0, 0, 136, 9, 0, 0, 0, 0, 1, 0, 0, 0]
        );
        assert_eq!(
            encode(8, &Response::Generation(2), 1024).unwrap(),
            [9, 0, 0, 0, 135, 8, 0, 0, 0, 2, 0, 0, 0]
        );
        let mut bodies = vec![vec![7, 2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]];
        let mut tile = vec![8, 3, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
        tile.extend(1_f64.to_le_bytes());
        for value in [0_u32, 0, 1, 1] {
            tile.extend(value.to_le_bytes());
        }
        bodies.push(tile);
        for body in bodies {
            assert!(decode(&body, Limits::default()).is_ok());
            for end in 0..body.len() {
                assert!(decode(&body[..end], Limits::default()).is_err());
            }
        }
    }

    #[test]
    fn decoding_rejects_every_truncation_and_trailing_bytes() {
        let bodies = [
            vec![1, 1, 0, 0, 0, 1, 0, 0, 0],
            vec![2, 2, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, b'x'],
            vec![3, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
            vec![
                4, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0,
                0, 0, 1, 0, 0, 0, 1, 0, 0, 0,
            ],
            vec![5, 5, 0, 0, 0, 1, 0, 0, 0],
            vec![6, 6, 0, 0, 0, 1, 0, 0, 0],
        ];
        for body in bodies {
            assert!(decode(&body, Limits::default()).is_ok());
            for cut in 0..body.len() {
                let error = decode(&body[..cut], Limits::default()).unwrap_err();
                assert_eq!(error.kind, ErrorKind::Syntax);
                assert!(matches!(error.position, Position::Buffer(_)));
            }
            let mut extra = body;
            extra.push(0);
            assert_eq!(
                decode(&extra, Limits::default()).unwrap_err().kind,
                ErrorKind::Syntax
            );
        }
        let invalid_mode = [2, 1, 0, 0, 0, 2];
        assert_eq!(
            decode(&invalid_mode, Limits::default()).unwrap_err().kind,
            ErrorKind::Syntax
        );
        let too_long_password = [2, 1, 0, 0, 0, 0, 255, 255, 255, 255];
        assert_eq!(
            decode(&too_long_password, Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
        let bad_tile = Response::Tile {
            width: 1,
            height: 1,
            rgba: vec![0; 3],
        };
        assert_eq!(
            encode(1, &bad_tile, 1024).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
    }
    #[test]
    fn diagnostic_kinds_have_independent_wire_oracles_and_legacy_filtering() {
        let information = Diagnostic::information("test.same", "info");
        let repair = Diagnostic::new(Repair::new("test.same"), "repair");
        let records = vec![information, repair];
        // Typed kind/id/count, then information=1 and repair=0, with Unknown contexts.
        let mut expected = vec![145, 9, 0, 0, 0, 2, 0, 0, 0, 1, 9, 0, 0, 0];
        expected.extend(b"test.same");
        expected.extend([4, 0, 0, 0]);
        expected.extend(b"info");
        expected.extend([0, 0]);
        let mut repair_bytes = vec![9, 0, 0, 0];
        repair_bytes.extend(b"test.same");
        repair_bytes.extend([6, 0, 0, 0]);
        repair_bytes.extend(b"repair");
        repair_bytes.extend([0, 0]);
        expected.push(0);
        expected.extend(&repair_bytes);
        let actual = encode(9, &Response::DiagnosticDetails(records.clone()), 1024).unwrap();
        assert_eq!(&actual[4..], expected);
        assert_eq!(
            encode(
                9,
                &Response::DiagnosticDetails(records.clone()),
                expected.len() - 1
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit
        );
        let mut legacy = vec![133, 9, 0, 0, 0, 1, 0, 0, 0];
        legacy.extend(repair_bytes);
        assert_eq!(
            &encode(9, &Response::Diagnostics(records), 1024).unwrap()[4..],
            legacy
        );
    }
}
