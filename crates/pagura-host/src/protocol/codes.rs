//! Every wire code space as an enum (PROTOCOL.md). The numbers appear only
//! here; `web/protocol_constants.mjs` is generated from these tables.

use pagura::{DiagnosticKind, Error, ErrorKind, Recovery, Result};

/// Defines a fieldless wire enum with its code table, `TryFrom` for decoding
/// and the variant names the JavaScript constants use.
macro_rules! wire_codes {
    (
        $(#[$meta:meta])*
        pub enum $name:ident: $repr:ty, unknown = $unknown:expr;
        $($(#[$doc:meta])* $variant:ident = $code:literal,)+
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr($repr)]
        #[non_exhaustive]
        pub enum $name {
            $($(#[$doc])* $variant = $code,)+
        }

        impl $name {
            /// Every code, in ascending order.
            pub const ALL: &[Self] = &[$(Self::$variant,)+];

            /// The code on the wire.
            pub const fn code(self) -> $repr {
                self as $repr
            }

            /// The variant's name, as `web/protocol_constants.mjs` spells it.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($variant),)+
                }
            }
        }

        impl From<$name> for $repr {
            fn from(value: $name) -> Self {
                value.code()
            }
        }

        impl TryFrom<$repr> for $name {
            type Error = Error;

            /// # Errors
            /// Returns an error without a position for an unassigned code;
            /// the decoder places it.
            fn try_from(code: $repr) -> Result<Self> {
                match code {
                    $($code => Ok(Self::$variant),)+
                    _ => Err($unknown),
                }
            }
        }
    };
}

wire_codes! {
    /// A protocol version, negotiated by hello. Later versions only add
    /// requests, so `>=` is "supports".
    pub enum Version: u32, unknown = Error::invalid_argument("invalid or repeated protocol handshake");
    /// The compatible base version.
    V1 = 1,
    /// Viewport generations and session accounting.
    V1_1 = 0x0001_0001,
    /// Reading operations.
    V1_2 = 0x0001_0002,
    /// Painted content bounds.
    V1_3 = 0x0001_0003,
    /// Typed diagnostic snapshots.
    V1_4 = 0x0001_0004,
    /// Form field inspection and atomic filled-form saves.
    V1_5 = 0x0001_0005,
    /// Page-local form widget geometry.
    V1_6 = 0x0001_0006,
    /// Actual form text presentation metrics.
    V1_7 = 0x0001_0007,
    /// Document metadata and explicit embedded-file reads.
    V1_8 = 0x0001_0008,
    /// Complete PNG page export.
    V1_9 = 0x0001_0009,
    /// Explicit attachment size and checksum comparisons.
    V1_10 = 0x0001_000a,
    /// Page attachment annotation discovery and explicit payload operations.
    V1_11 = 0x0001_000b,
    /// Scoped files, object metadata, collections, image transfer and reauthentication.
    V1_12 = 0x0001_000c,
    /// Immutable range sources and owned pull-driven jobs.
    V2 = 0x0002_0000,
}

wire_codes! {
    /// Observation from an explicit attachment parameter check (1.10).
    pub enum AttachmentCheckStatus: u8, unknown = Error::syntax("unknown attachment check status");
    /// No declaration is present.
    NotDeclared = 0,
    /// The declaration matches decoded bytes.
    Matches = 1,
    /// The declaration differs from decoded bytes.
    Mismatch = 2,
}

impl Version {
    /// The newest version this host speaks.
    pub const LATEST: Self = Self::V2;

    /// The error for a request that needs this version in an older session.
    pub(crate) const fn required(self) -> Error {
        Error::invalid_argument(match self {
            Self::V1 => "request requires a protocol handshake",
            Self::V1_1 => "request requires protocol 1.1",
            Self::V1_2 => "reading request requires protocol 1.2",
            Self::V1_3 => "content bounds require protocol 1.3",
            Self::V1_4 => "typed diagnostics require protocol 1.4",
            Self::V1_5 => "form editing requires protocol 1.5",
            Self::V1_6 => "inline form editing requires protocol 1.6",
            Self::V1_7 => "form text styles require protocol 1.7",
            Self::V1_8 => "document services require protocol 1.8",
            Self::V1_9 => "PNG export requires protocol 1.9",
            Self::V1_10 => "attachment checks require protocol 1.10",
            Self::V1_11 => "page attachment operations require protocol 1.11",
            Self::V1_12 => "extended document services require protocol 1.12",
            Self::V2 => "progressive source jobs require protocol 2.0",
        })
    }
}

wire_codes! {
    /// The kind byte of a request frame.
    pub enum RequestKind: u8, unknown = Error::syntax("unknown request kind");
    /// Negotiate the version.
    Hello = 1,
    /// Open PDF bytes.
    Open = 2,
    /// Describe a page.
    PageInfo = 3,
    /// Render a tile.
    RenderTile = 4,
    /// Repairs discovered so far.
    Diagnostics = 5,
    /// Release a document.
    Close = 6,
    /// Select a viewport generation (1.1).
    Generation = 7,
    /// A tile of a viewport generation (1.1).
    GenerationTile = 8,
    /// Session accounting (1.1).
    Stats = 9,
    /// Page text (1.2).
    PageText = 10,
    /// A search window (1.2).
    Search = 11,
    /// Page links (1.2).
    Links = 12,
    /// Outlines (1.2).
    Outlines = 13,
    /// Page labels (1.2).
    Labels = 14,
    /// Reading accounting (1.2).
    ReadingStats = 15,
    /// Painted content bounds (1.3).
    Bounds = 16,
    /// Repairs and information with their kinds (1.4).
    DiagnosticDetails = 17,
    /// Form controls (1.5).
    FormFields = 18,
    /// Save a filled form (1.5).
    SaveForm = 19,
    /// Page-local form widgets (1.6).
    FormWidgets = 20,
    /// Form text presentation (1.7).
    FormStyles = 21,
    /// Info, language and metadata-stream identity (1.8).
    Metadata = 22,
    /// Catalog EmbeddedFiles entries, without reading payloads (1.8).
    Attachments = 23,
    /// Explicit inert metadata-stream bytes (1.8).
    MetadataBytes = 24,
    /// Explicit catalog attachment and EF key selection (1.8).
    AttachmentBytes = 25,
    /// Complete PNG page image (1.9).
    PagePng = 26,
    /// Check the selected attachment parameters against decoded bytes (1.10).
    AttachmentCheck = 27,
    /// FileAttachment annotations on one page, without reading payloads (1.11).
    PageAttachments = 28,
    /// Explicit page, Annots slot and EF key selection (1.11).
    PageAttachmentBytes = 29,
    /// Check the selected annotation attachment parameters (1.11).
    PageAttachmentCheck = 30,
    /// Exact file collection and complete EF/RF declarations. (1.12).
    Files = 31,
    /// Exact scoped EF or RF payload bytes. (1.12).
    FileBytes = 32,
    /// Exact scoped EF or RF parameter comparisons. (1.12).
    FileCheck = 33,
    /// Metadata packet identity of an explicit owner. (1.12).
    ObjectMetadata = 34,
    /// Metadata packet bytes of an explicit owner. (1.12).
    ObjectMetadataBytes = 35,
    /// Portable collection declarations. (1.12).
    Collection = 36,
    /// Direct page image resources or one explicit image. (1.12).
    Images = 37,
    /// Stored or encoded image asset bytes. (1.12).
    ImageBytes = 38,
    /// Authenticate a new immutable snapshot. (1.12).
    Reauthenticate = 39,
    /// Stored thumbnail declaration of an explicit owner. (1.12).
    Thumbnail = 40,
    /// Register an immutable range source. (2.0).
    SourceCreate = 41,
    /// Supply verified bytes for a registered version. (2.0).
    SourceSupply = 42,
    /// Finalize source coverage. (2.0).
    SourceFinish = 43,
    /// Close a registered source. (2.0).
    SourceClose = 44,
    /// Start an owned document opening. (2.0).
    OpenJob = 45,
    /// Start an exact selected-page lookup. (2.0).
    PageJob = 46,
    /// Start source-dependent preparation and streamed output. (2.0).
    RenderJob = 47,
    /// Advance one owned job unit. (2.0).
    JobAdvance = 48,
    /// Release the outstanding output sequence. (2.0).
    JobAcknowledge = 49,
    /// Cancel one owned job. (2.0).
    JobCancel = 50,
    /// Release a job handle. (2.0).
    JobClose = 51,
    /// Read progressive admission and retention accounting. (2.0).
    JobStats = 52,
}

impl RequestKind {
    /// The first version that accepts this request.
    pub const fn since(self) -> Version {
        match self {
            Self::SourceCreate
            | Self::SourceSupply
            | Self::SourceFinish
            | Self::SourceClose
            | Self::OpenJob
            | Self::PageJob
            | Self::RenderJob
            | Self::JobAdvance
            | Self::JobAcknowledge
            | Self::JobCancel
            | Self::JobClose
            | Self::JobStats => Version::V2,
            Self::Files
            | Self::FileBytes
            | Self::FileCheck
            | Self::ObjectMetadata
            | Self::ObjectMetadataBytes
            | Self::Collection
            | Self::Images
            | Self::ImageBytes
            | Self::Reauthenticate
            | Self::Thumbnail => Version::V1_12,
            Self::Hello
            | Self::Open
            | Self::PageInfo
            | Self::RenderTile
            | Self::Diagnostics
            | Self::Close => Version::V1,
            Self::Generation | Self::GenerationTile | Self::Stats => Version::V1_1,
            Self::PageText
            | Self::Search
            | Self::Links
            | Self::Outlines
            | Self::Labels
            | Self::ReadingStats => Version::V1_2,
            Self::Bounds => Version::V1_3,
            Self::DiagnosticDetails => Version::V1_4,
            Self::FormFields | Self::SaveForm => Version::V1_5,
            Self::FormWidgets => Version::V1_6,
            Self::FormStyles => Version::V1_7,
            Self::PagePng => Version::V1_9,
            Self::AttachmentCheck => Version::V1_10,
            Self::PageAttachments | Self::PageAttachmentBytes | Self::PageAttachmentCheck => {
                Version::V1_11
            }
            Self::Metadata | Self::Attachments | Self::MetadataBytes | Self::AttachmentBytes => {
                Version::V1_8
            }
        }
    }

    /// The ordinary response kind; version two can also return `NeedData`.
    /// Operational failures use [`ResponseKind::Error`].
    pub const fn response(self) -> ResponseKind {
        match self {
            Self::SourceCreate => ResponseKind::SourceCreate,
            Self::SourceSupply => ResponseKind::SourceSupply,
            Self::SourceFinish => ResponseKind::SourceFinish,
            Self::SourceClose => ResponseKind::SourceClose,
            Self::OpenJob | Self::PageJob | Self::RenderJob => ResponseKind::JobCreated,
            Self::JobAdvance => ResponseKind::JobProgress,
            Self::JobAcknowledge => ResponseKind::JobAcknowledge,
            Self::JobCancel => ResponseKind::JobCancel,
            Self::JobClose => ResponseKind::JobClose,
            Self::JobStats => ResponseKind::JobStats,
            Self::Files => ResponseKind::Files,
            Self::FileBytes => ResponseKind::FileBytes,
            Self::FileCheck => ResponseKind::FileCheck,
            Self::ObjectMetadata => ResponseKind::ObjectMetadata,
            Self::ObjectMetadataBytes => ResponseKind::ObjectMetadataBytes,
            Self::Collection => ResponseKind::Collection,
            Self::Images => ResponseKind::Images,
            Self::ImageBytes => ResponseKind::ImageBytes,
            Self::Reauthenticate => ResponseKind::Reauthenticate,
            Self::Thumbnail => ResponseKind::Thumbnail,

            Self::Hello => ResponseKind::Hello,
            Self::Open => ResponseKind::Open,
            Self::PageInfo => ResponseKind::PageInfo,
            Self::RenderTile => ResponseKind::Tile,
            Self::Diagnostics => ResponseKind::Diagnostics,
            Self::Close => ResponseKind::Close,
            Self::Generation => ResponseKind::Generation,
            Self::GenerationTile => ResponseKind::GenerationTile,
            Self::Stats => ResponseKind::Stats,
            Self::PageText => ResponseKind::PageText,
            Self::Search => ResponseKind::Search,
            Self::Links => ResponseKind::Links,
            Self::Outlines => ResponseKind::Outlines,
            Self::Labels => ResponseKind::Labels,
            Self::ReadingStats => ResponseKind::ReadingStats,
            Self::Bounds => ResponseKind::Bounds,
            Self::DiagnosticDetails => ResponseKind::DiagnosticDetails,
            Self::FormFields => ResponseKind::FormFields,
            Self::SaveForm => ResponseKind::SavedForm,
            Self::FormWidgets => ResponseKind::FormWidgets,
            Self::FormStyles => ResponseKind::FormStyles,
            Self::Metadata => ResponseKind::Metadata,
            Self::Attachments => ResponseKind::Attachments,
            Self::MetadataBytes => ResponseKind::MetadataBytes,
            Self::AttachmentBytes => ResponseKind::AttachmentBytes,
            Self::PagePng => ResponseKind::PagePng,
            Self::AttachmentCheck => ResponseKind::AttachmentCheck,
            Self::PageAttachments => ResponseKind::PageAttachments,
            Self::PageAttachmentBytes => ResponseKind::PageAttachmentBytes,
            Self::PageAttachmentCheck => ResponseKind::PageAttachmentCheck,
        }
    }
}

wire_codes! {
    /// The kind byte of a response frame.
    pub enum ResponseKind: u8, unknown = Error::syntax("unknown response kind");
    /// The negotiated version.
    Hello = 129,
    /// A new handle and page count.
    Open = 130,
    /// Page geometry.
    PageInfo = 131,
    /// Tile pixels.
    Tile = 132,
    /// Repairs.
    Diagnostics = 133,
    /// The handle was released.
    Close = 134,
    /// The selected generation.
    Generation = 135,
    /// A generated tile or its skip status.
    GenerationTile = 136,
    /// Session accounting.
    Stats = 137,
    /// Page text.
    PageText = 138,
    /// Search matches and cursor.
    Search = 139,
    /// Page links.
    Links = 140,
    /// Outlines.
    Outlines = 141,
    /// Page labels.
    Labels = 142,
    /// Reading accounting.
    ReadingStats = 143,
    /// Painted content bounds.
    Bounds = 144,
    /// Typed diagnostics.
    DiagnosticDetails = 145,
    /// Form controls.
    FormFields = 146,
    /// Saved PDF bytes.
    SavedForm = 147,
    /// Page-local form widgets.
    FormWidgets = 148,
    /// Form text presentation.
    FormStyles = 149,
    /// Info, language and metadata-stream identity.
    Metadata = 150,
    /// Catalog attachment declarations.
    Attachments = 151,
    /// Optional inert metadata-stream bytes.
    MetadataBytes = 152,
    /// Selected embedded-file bytes.
    AttachmentBytes = 153,
    /// Complete PNG page image (1.9).
    PagePng = 154,
    /// Completed size and checksum observations (1.10).
    AttachmentCheck = 155,
    /// FileAttachment annotation records for one page (1.11).
    PageAttachments = 156,
    /// Explicitly selected page attachment bytes (1.11).
    PageAttachmentBytes = 157,
    /// Page attachment parameter observations (1.11).
    PageAttachmentCheck = 158,
    /// Exact file collection and complete EF/RF declarations. (1.12).
    Files = 159,
    /// Exact scoped EF or RF payload bytes. (1.12).
    FileBytes = 160,
    /// Exact scoped EF or RF parameter comparisons. (1.12).
    FileCheck = 161,
    /// Metadata packet identity of an explicit owner. (1.12).
    ObjectMetadata = 162,
    /// Metadata packet bytes of an explicit owner. (1.12).
    ObjectMetadataBytes = 163,
    /// Portable collection declarations. (1.12).
    Collection = 164,
    /// Direct page image resources or one explicit image. (1.12).
    Images = 165,
    /// Stored or encoded image asset bytes. (1.12).
    ImageBytes = 166,
    /// Authenticate a new immutable snapshot. (1.12).
    Reauthenticate = 167,
    /// Stored thumbnail declaration of an explicit owner. (1.12).
    Thumbnail = 168,
    /// Progressive SourceCreate response (2.0).
    SourceCreate = 169,
    /// Progressive SourceSupply response (2.0).
    SourceSupply = 170,
    /// Progressive SourceFinish response (2.0).
    SourceFinish = 171,
    /// Progressive SourceClose response (2.0).
    SourceClose = 172,
    /// Progressive JobCreated response (2.0).
    JobCreated = 173,
    /// Progressive JobProgress response (2.0).
    JobProgress = 174,
    /// Progressive JobAcknowledge response (2.0).
    JobAcknowledge = 175,
    /// Progressive JobCancel response (2.0).
    JobCancel = 176,
    /// Progressive JobClose response (2.0).
    JobClose = 177,
    /// Progressive JobStats response (2.0).
    JobStats = 178,
    /// Progressive NeedData response (2.0).
    NeedData = 179,
    /// A structured failure.
    Error = 255,
}

wire_codes! {
    /// Explicit EF filename-key selection (Table 43); no platform inference.
    pub enum AttachmentKey: u8, unknown = Error::syntax("unknown attachment filename key");
    /// F filename.
    File = 0,
    /// UF Unicode filename.
    Unicode = 1,
    /// Legacy DOS filename.
    Dos = 2,
    /// Legacy Mac filename.
    Mac = 3,
    /// Legacy Unix filename.
    Unix = 4,
}

impl AttachmentKey {
    /// The corresponding PDF filename key, without a slash.
    pub const fn pdf_name(self) -> &'static [u8] {
        match self {
            Self::File => b"F",
            Self::Unicode => b"UF",
            Self::Dos => b"DOS",
            Self::Mac => b"Mac",
            Self::Unix => b"Unix",
        }
    }
}

wire_codes! {
    /// The category byte of an error response, one per [`ErrorKind`].
    pub enum ErrorCategory: u8, unknown = Error::syntax("unknown error category");
    /// Malformed input.
    Syntax = 1,
    /// A budget was exceeded.
    Limit = 2,
    /// A feature is not supported.
    Unsupported = 3,
    /// A referenced object is missing.
    MissingObject = 4,
    /// A reference cycle.
    Cycle = 5,
    /// The password did not authenticate.
    Password = 6,
    /// The caller passed an invalid argument.
    InvalidArgument = 7,
}

impl TryFrom<&ErrorKind> for ErrorCategory {
    type Error = Error;

    /// # Errors
    /// A kind added to pagura-core has no category until a protocol version
    /// assigns one, and cannot be encoded faithfully.
    fn try_from(kind: &ErrorKind) -> Result<Self> {
        Ok(match kind {
            ErrorKind::Syntax => Self::Syntax,
            ErrorKind::Limit => Self::Limit,
            ErrorKind::Unsupported(_) => Self::Unsupported,
            ErrorKind::MissingObject => Self::MissingObject,
            ErrorKind::Cycle => Self::Cycle,
            ErrorKind::Password => Self::Password,
            ErrorKind::InvalidArgument => Self::InvalidArgument,
            _ => {
                return Err(Error::invalid_argument(
                    "error kind has no wire code in this protocol version",
                ));
            }
        })
    }
}

wire_codes! {
    /// The kind byte of a typed diagnostic record (1.4).
    pub enum DiagnosticCode: u8, unknown = Error::syntax("unknown diagnostic kind");
    /// A repair that was applied.
    Repair = 0,
    /// Information that changes nothing.
    Information = 1,
}

impl TryFrom<&DiagnosticKind> for DiagnosticCode {
    type Error = Error;

    /// # Errors
    /// A kind added to pagura-core has no code until a protocol version
    /// assigns one.
    fn try_from(kind: &DiagnosticKind) -> Result<Self> {
        match kind {
            DiagnosticKind::Repair(_) => Ok(Self::Repair),
            DiagnosticKind::Information(_) => Ok(Self::Information),
            _ => Err(Error::invalid_argument(
                "diagnostic kind has no wire code in this protocol version",
            )),
        }
    }
}

wire_codes! {
    /// The position kind byte of an error or diagnostic context.
    pub enum PositionKind: u8, unknown = Error::syntax("unknown position kind");
    /// No offset.
    Unknown = 0,
    /// A file offset.
    File = 1,
    /// An offset in a decoded stream.
    Stream = 2,
    /// An offset in a request body or another buffer.
    Buffer = 3,
}

wire_codes! {
    /// The status byte of a generated tile response (1.1).
    pub enum TileStatus: u8, unknown = Error::syntax("unknown tile status");
    /// A stale or cancelled tile, without pixels.
    Skipped = 0,
    /// Pixels follow.
    Rendered = 1,
}

wire_codes! {
    /// The fit byte of a destination (Table 149).
    pub enum FitCode: u8, unknown = Error::syntax("unknown destination fit");
    /// XYZ left, top, zoom.
    Xyz = 0,
    /// Fit.
    Fit = 1,
    /// FitH top.
    FitH = 2,
    /// FitV left.
    FitV = 3,
    /// FitR left, bottom, right, top.
    FitR = 4,
    /// FitB.
    FitB = 5,
    /// FitBH top.
    FitBh = 6,
    /// FitBV left.
    FitBv = 7,
}

wire_codes! {
    /// The type byte of a target action (12.6).
    pub enum ActionCode: u8, unknown = Error::syntax("unknown action type");
    /// An internal destination.
    GoTo = 1,
    /// A URI.
    Uri = 2,
    /// A named action.
    Named = 3,
    /// A remote destination, inert.
    GoToR = 4,
    /// A launch action, inert.
    Launch = 5,
    /// A script, inert; no script bytes are sent.
    JavaScript = 6,
    /// Any other action, inert.
    Other = 7,
}

/// Defines a bit-field newtype with named flags.
macro_rules! wire_flags {
    (
        $(#[$meta:meta])*
        pub struct $name:ident;
        $($(#[$doc:meta])* $flag:ident = $bit:literal,)+
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
        pub struct $name(u8);

        impl $name {
            $($(#[$doc])* pub const $flag: Self = Self($bit);)+
            /// Every flag with its name, as `web/protocol_constants.mjs` spells it.
            pub const ALL: &[(&str, Self)] = &[$((stringify!($flag), Self::$flag),)+];

            /// The flags, or `None` if an undefined bit is set.
            pub const fn from_bits(bits: u8) -> Option<Self> {
                if (bits & !(0 $(| $bit)+)) == 0 { Some(Self(bits)) } else { None }
            }

            /// The byte on the wire.
            pub const fn bits(self) -> u8 {
                self.0
            }

            /// Whether every flag of `other` is set.
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            /// Sets every flag of `other`.
            pub const fn insert(&mut self, other: Self) {
                self.0 |= other.0;
            }
        }
    };
}

wire_flags! {
    /// The option bits of a search request (1.2).
    pub struct SearchFlags;
    /// Match case exactly.
    CASE_SENSITIVE = 1,
    /// Match whole words only.
    WHOLE_WORDS = 2,
    /// Join words hyphenated across a line break.
    JOIN_HYPHENATED = 4,
}

wire_flags! {
    /// The style bits of an outline item (Table 151).
    pub struct OutlineStyle;
    /// Italic text.
    ITALIC = 1,
    /// Bold text.
    BOLD = 2,
}

wire_flags! {
    /// The flags byte of an open request. Without flags the document is
    /// validated strictly; a later protocol version may assign more bits,
    /// and an unassigned bit is a terminal syntax error.
    pub struct OpenFlags;
    /// Reported recovery instead of strict validation (ADR 0003).
    REPAIR = 1,
}

impl OpenFlags {
    /// The recovery policy the flags select.
    pub const fn recovery(self) -> Recovery {
        if self.contains(Self::REPAIR) {
            Recovery::Repair
        } else {
            Recovery::Strict
        }
    }
}

wire_codes! {
    /// Background of an exported PNG page (1.9).
    pub enum PngBackground: u8, unknown = Error::syntax("unknown PNG background");
    /// Composite onto opaque white.
    White = 0,
    /// Retain straight alpha with transparent unpainted pixels.
    Transparent = 1,
}

wire_codes! {
    /// File collection selectors (1.12).
    pub enum FileScopeCode: u8, unknown = Error::syntax("unknown FileScopeCode code");
    /// Catalog selection.
    Catalog = 0,
    /// Associated selection.
    Associated = 1,
    /// PageAssociated selection.
    PageAssociated = 2,
    /// ObjectAssociated selection.
    ObjectAssociated = 3,
    /// PageAnnotations selection.
    PageAnnotations = 4,
}

wire_codes! {
    /// Metadata and thumbnail owners (1.12).
    pub enum MetadataOwnerCode: u8, unknown = Error::syntax("unknown MetadataOwnerCode code");
    /// Document selection.
    Document = 0,
    /// Page selection.
    Page = 1,
    /// Object selection.
    Object = 2,
}

wire_codes! {
    /// File payload selectors (1.12).
    pub enum PayloadKind: u8, unknown = Error::syntax("unknown PayloadKind code");
    /// Embedded selection.
    Embedded = 0,
    /// Related selection.
    Related = 1,
}

wire_codes! {
    /// Image transfer modes (1.12).
    pub enum ImageTransferCode: u8, unknown = Error::syntax("unknown ImageTransferCode code");
    /// Stored selection.
    Stored = 0,
    /// Encoded selection.
    Encoded = 1,
}

wire_codes! {
    /// Image object selection (1.12).
    pub enum ImageAssetCode: u8, unknown = Error::syntax("unknown ImageAssetCode code");
    /// Image selection.
    Image = 0,
    /// Thumbnail selection.
    Thumbnail = 1,
}

wire_codes! {
    /// Final output representation of a progressive render (2.0).
    pub enum OutputCode: u8, unknown = Error::syntax("unknown progressive output kind");
    /// Final premultiplied RGBA8 bands in device row order.
    Bands = 0,
    /// One straight RGBA8 PNG, in canonical chunk order.
    Png = 1,
}

wire_codes! {
    /// Readiness policy carried by a source-backed document (2.0).
    pub enum ValidationCode: u8, unknown = Error::syntax("unknown validation scope");
    /// Validate all source objects before publishing the document.
    CompleteDocument = 0,
    /// Validate authoritative structure and each requested dependency.
    RequestedOperation = 1,
}

wire_codes! {
    /// Explicit progress event of an owned job (2.0).
    pub enum JobEventCode: u8, unknown = Error::syntax("unknown job event");
    /// One CPU unit completed with no result yet.
    Yielded = 0,
    /// A document handle is ready under its stated policy.
    Opened = 1,
    /// Exact selected-page geometry is ready.
    Page = 2,
    /// All source reads and render preflight are complete.
    Prepared = 3,
    /// One final immutable band with an acknowledgement sequence.
    Band = 4,
    /// One encoded PNG chunk with an acknowledgement sequence.
    Png = 5,
    /// Successful terminal completion, after all output was acknowledged.
    Complete = 6,
    /// Explicit terminal cancellation.
    Cancelled = 7,
    /// An outstanding lease prevents more CPU work.
    Backpressured = 8,
}
