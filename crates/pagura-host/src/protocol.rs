//! Protocol wire types and bounded serialization (ADR 0005, PROTOCOL.md).
//!
//! Requests decode into typed values whose fields remember their body
//! offsets, so errors point at the field that caused them without a second
//! copy of the layout. Responses encode through one bounded writer.

mod codes;
mod decode;
mod encode;
// The generator of web/protocol_constants.mjs, compiled only for its drift
// test and for the example behind the `codegen` feature.
#[cfg(any(test, feature = "codegen"))]
mod javascript;

pub use codes::{
    ActionCode, AttachmentCheckStatus, AttachmentKey, DiagnosticCode, ErrorCategory, FileScopeCode,
    FitCode, ImageAssetCode, ImageTransferCode, JobEventCode, MetadataOwnerCode, OpenFlags,
    OutlineStyle, OutputCode, PayloadKind, PngBackground, PositionKind, RequestKind, ResponseKind,
    SearchFlags, TileStatus, ValidationCode, Version,
};
pub(crate) use decode::Reader;
pub use decode::decode;
pub use encode::encode;

use pagura::{Diagnostic, Error, Position, Recovery, Tile};

/// Fixed frame layout, shared by the decoder, the encoder and the budgets.
pub(crate) mod layout {
    /// Frame bytes before the body: its `u32` length.
    pub(crate) const LENGTH_BYTES: usize = 4;
    /// Body bytes before the payload: the kind byte and the request id.
    pub(crate) const HEADER_BYTES: usize = 5;
    /// Offset of the kind byte in a body.
    pub(crate) const KIND_OFFSET: usize = 0;
    /// Offset of the request id in a body.
    pub(crate) const ID_OFFSET: usize = 1;
    /// A PNG body before its bytes: header and byte count.
    pub(crate) const PNG_OVERHEAD: usize = HEADER_BYTES + 4;
    /// Framing errors are reported at the start of the frame (PROTOCOL.md).
    pub(crate) const FRAME_START: usize = 0;
    /// The largest body the `u32` length field can describe.
    pub(crate) const MAX_BODY_BYTES: usize = u32::MAX as usize;
    /// An open body without its blobs: header, recovery byte, two lengths.
    pub(crate) const OPEN_OVERHEAD: usize = HEADER_BYTES + 1 + 4 + 4;
    /// A tile body without its pixels: header, width and height.
    pub(crate) const TILE_OVERHEAD: usize = HEADER_BYTES + 4 + 4;
    /// A generated tile body without its pixels: status and generation too.
    pub(crate) const GENERATION_TILE_OVERHEAD: usize = TILE_OVERHEAD + 1 + 4;
}

/// The wire tables as an ECMAScript module, for the example that regenerates
/// `web/protocol_constants.mjs`; only the `codegen` feature exposes it.
#[cfg(feature = "codegen")]
pub fn javascript_constants() -> String {
    javascript::javascript_constants()
}

/// A decoded request field and the body offset it was read from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field<T> {
    value: T,
    offset: usize,
}

impl<T: Copy> Field<T> {
    /// A field holding `value`, read at body `offset`.
    pub const fn new(value: T, offset: usize) -> Self {
        Self { value, offset }
    }

    /// The decoded value.
    pub const fn get(self) -> T {
        self.value
    }

    /// The offset of the field's first byte in the request body.
    pub const fn offset(self) -> usize {
        self.offset
    }

    /// The field's position, for errors about it.
    pub const fn position(self) -> Position {
        Position::Buffer(self.offset)
    }
}

/// A page of an open document, as named by a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PageRef {
    /// Session-local document handle.
    pub document: Field<u32>,
    /// Zero-based page index.
    pub page: Field<u32>,
}

impl PageRef {
    /// The page `page` of the document `document`.
    pub const fn new(document: Field<u32>, page: Field<u32>) -> Self {
        Self { document, page }
    }
}

impl<T: Copy + Eq> Eq for Field<T> {}

impl Field<u32> {
    /// The value as an index into pages or matches.
    pub(crate) fn index(self) -> pagura::Result<usize> {
        usize::try_from(self.value)
            .map_err(|_| Error::invalid_argument("index exceeds address space").at(self.position()))
    }
}

/// The payload of a render request, shared by kinds 4 and 8.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TileRequest {
    /// The page to render.
    pub page: PageRef,
    /// Device pixels per default user-space unit.
    pub scale: Field<f64>,
    /// Device-pixel rectangle, top-left origin (ADR 0004).
    pub tile: Field<Tile>,
}

/// A complete PNG page export (1.9), independent of viewport generations.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PngRequest {
    /// Source page, including its effective crop box, rotation and UserUnit.
    pub page: PageRef,
    /// Resolution relative to 72 DPI.
    pub scale: Field<f64>,
    /// Explicit white or transparent background.
    pub background: Field<PngBackground>,
}

/// A borrowed, fully validated request payload.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Request<'a> {
    /// Immutable range sources and owned pull-driven operations (2.0).
    Progressive(crate::progressive::Request<'a>),
    /// Reauthenticate a fresh snapshot without mutating the existing handle (1.12).
    Reauthenticate {
        /// Original immutable document handle.
        document: Field<u32>,
        /// Raw password bytes, bounded by string_bytes and never retained.
        password: &'a [u8],
    },
    /// Negotiate a version before accessing documents. The version is
    /// checked when the request is ordered.
    Hello(Field<u32>),
    /// Open immutable PDF bytes, with the chosen recovery policy and raw password bytes.
    Open {
        /// PDF source bytes (7.5).
        bytes: &'a [u8],
        /// Strict validation or reported recovery (ADR 0003).
        recovery: Recovery,
        /// Password bytes, never interpreted as instructions.
        password: &'a [u8],
    },
    /// Describe a page.
    PageInfo(PageRef),
    /// Render a device-pixel rectangle onto opaque white.
    RenderTile(TileRequest),
    /// Select the newest viewport generation for a document (1.1).
    Generation {
        /// Session-local document handle.
        document: Field<u32>,
        /// Monotonically increasing, nonzero generation.
        generation: Field<u32>,
    },
    /// A tile which may be skipped before compilation or painting (1.1).
    GenerationTile {
        /// Viewport generation previously selected for the document.
        generation: Field<u32>,
        /// The tile, laid out as in a render request.
        tile: TileRequest,
    },
    /// Snapshot repairs discovered so far (all versions).
    Diagnostics(Field<u32>),
    /// Snapshot repairs and information with their kind (1.4).
    DiagnosticDetails(Field<u32>),
    /// Release a document; handles are never reused.
    Close(Field<u32>),
    /// Snapshot session retention accounting (1.1).
    Stats,
    /// Reading operations (1.2, 1.3 and 1.8).
    Reading(crate::reading::Request<'a>),
    /// Inspect bounded form controls (1.5).
    FormFields(Field<u32>),
    /// Apply a batch to a temporary filler and return saved bytes (1.5).
    SaveForm {
        /// Immutable source handle.
        document: Field<u32>,
        /// Bounded editing payload described in PROTOCOL.md.
        changes: Field<&'a [u8]>,
    },
    /// Inspect editable widgets on a page (1.6).
    FormWidgets(PageRef),
    /// Inspect text presentation from stored widget appearances (1.7).
    FormStyles(PageRef),
    /// Export a complete page as straight RGBA8 PNG (1.9).
    PagePng(PngRequest),
}

impl Request<'_> {
    /// The request's kind byte.
    pub const fn kind(&self) -> RequestKind {
        match self {
            Self::Progressive(request) => request.kind(),
            Self::Hello(_) => RequestKind::Hello,
            Self::Reauthenticate { .. } => RequestKind::Reauthenticate,
            Self::Open { .. } => RequestKind::Open,
            Self::PageInfo(_) => RequestKind::PageInfo,
            Self::RenderTile(_) => RequestKind::RenderTile,
            Self::Generation { .. } => RequestKind::Generation,
            Self::GenerationTile { .. } => RequestKind::GenerationTile,
            Self::Diagnostics(_) => RequestKind::Diagnostics,
            Self::DiagnosticDetails(_) => RequestKind::DiagnosticDetails,
            Self::Close(_) => RequestKind::Close,
            Self::Stats => RequestKind::Stats,
            Self::Reading(request) => request.kind(),
            Self::FormFields(_) => RequestKind::FormFields,
            Self::SaveForm { .. } => RequestKind::SaveForm,
            Self::FormWidgets(_) => RequestKind::FormWidgets,
            Self::FormStyles(_) => RequestKind::FormStyles,
            Self::PagePng(_) => RequestKind::PagePng,
        }
    }
}

/// Session-owned retention accounting, not allocator or total-heap telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionStats {
    /// Original complete input bytes plus sparse-source admission ceilings.
    /// Includes closed handles still pinned by jobs or source leases (2.0).
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
#[non_exhaustive]
pub struct RequestFrame<'a> {
    /// Strictly increasing identifier within the session.
    pub id: u32,
    /// The validated request.
    pub request: Request<'a>,
}

/// Device pixels of a tile response.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Pixels {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major, top-left-origin, straight RGBA8 bytes.
    pub rgba: Vec<u8>,
}

impl Pixels {
    /// A tile of `width` by `height` pixels.
    pub const fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba,
        }
    }
}

/// A successful response or a structured engine or protocol failure.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Response {
    /// Bounded progressive source/job event (2.0).
    Progressive(Payload),
    /// The negotiated version.
    Hello(Version),
    /// New document handle and page count.
    Open {
        /// Nonzero session-local handle.
        document: u32,
        /// Number of pages.
        pages: u32,
    },
    /// Typed page geometry (7.7.3.3).
    PageInfo {
        /// Normalized effective crop box: x0, y0, x1, y1.
        crop: [f64; 4],
        /// Clockwise rotation in degrees, a multiple of 90.
        rotation: u16,
        /// Physical scale of user space.
        user_unit: f64,
        /// Device width and height at scale 1, including rotation and user unit.
        size: (u32, u32),
    },
    /// A complete tile; failures never return partial pixels.
    Tile(Pixels),
    /// The selected viewport generation (1.1).
    Generation(u32),
    /// A stale or cancelled tile was not rendered (1.1).
    Skipped(u32),
    /// Complete pixels for a viewport generation (1.1).
    GenerationTile {
        /// Viewport generation.
        generation: u32,
        /// The pixels.
        pixels: Pixels,
    },
    /// Retention accounting (1.1).
    Stats(SessionStats),
    /// A bounded reading response, encoded by the reading adapter.
    Reading(crate::reading::Response),
    /// Encoded bounded form controls (1.5).
    FormFields(Vec<u8>),
    /// Complete saved PDF bytes; the original handle remains unchanged (1.5).
    SavedForm(Vec<u8>),
    /// Bounded page-local widget geometry (1.6).
    FormWidgets(Vec<u8>),
    /// Bounded native text editor metrics (1.7).
    FormStyles(Vec<u8>),
    /// Complete PNG bytes, without partial output on failure (1.9).
    PagePng(Vec<u8>),
    /// Repairs in discovery order. The legacy layout has no kind byte, so
    /// informational records are not encoded.
    Diagnostics(Vec<Diagnostic>),
    /// All diagnostics in discovery order, with explicit kinds (1.4).
    DiagnosticDetails(Vec<Diagnostic>),
    /// The handle has been released.
    Close,
    /// Structured failure, preserving context from the engine.
    Error(Error),
}

impl Response {
    /// The response's kind byte.
    pub const fn kind(&self) -> ResponseKind {
        match self {
            Self::Progressive(response) => response.kind(),
            Self::Hello(_) => ResponseKind::Hello,
            Self::Open { .. } => ResponseKind::Open,
            Self::PageInfo { .. } => ResponseKind::PageInfo,
            Self::Tile(_) => ResponseKind::Tile,
            Self::Generation(_) => ResponseKind::Generation,
            Self::Skipped(_) | Self::GenerationTile { .. } => ResponseKind::GenerationTile,
            Self::Stats(_) => ResponseKind::Stats,
            Self::Reading(response) => response.kind(),
            Self::FormFields(_) => ResponseKind::FormFields,
            Self::SavedForm(_) => ResponseKind::SavedForm,
            Self::FormWidgets(_) => ResponseKind::FormWidgets,
            Self::FormStyles(_) => ResponseKind::FormStyles,
            Self::PagePng(_) => ResponseKind::PagePng,
            Self::Diagnostics(_) => ResponseKind::Diagnostics,
            Self::DiagnosticDetails(_) => ResponseKind::DiagnosticDetails,
            Self::Close => ResponseKind::Close,
            Self::Error(_) => ResponseKind::Error,
        }
    }
}

/// A bounded adapter payload. Only the host's typed operation adapters can
/// construct its response kind and bytes, through the canonical capped writer.
#[derive(Debug, Clone, PartialEq)]
pub struct Payload {
    kind: ResponseKind,
    payload: Vec<u8>,
}

impl Payload {
    pub(crate) fn new(kind: ResponseKind, writer: crate::wire::Writer) -> Self {
        Self {
            kind,
            payload: writer.finish(),
        }
    }

    /// The response's kind byte.
    pub const fn kind(&self) -> ResponseKind {
        self.kind
    }

    /// The encoded payload, after kind and id.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

#[cfg(test)]
mod tests;
