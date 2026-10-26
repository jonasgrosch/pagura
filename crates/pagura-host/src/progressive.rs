//! Progressive source and job messages over the existing session protocol.
//! No source registration contains a URL or grants I/O capabilities.

mod config;
pub use config::ProgressiveLimits;

use crate::protocol::{Field, PageRef, PngBackground, RequestKind};
use pagura::{Recovery, Tile, Validation};

/// One validated progressive request. Source and job handles never reuse an ID.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Request<'a> {
    /// Register a fixed representation before supplying its byte ranges.
    SourceCreate {
        /// Fixed source length, subject to file and target-size admission.
        length: Field<u64>,
        /// Opaque representation token, not an address or credential.
        version: Field<[u8; 32]>,
    },
    /// Atomically supply bytes for the registered immutable representation.
    SourceSupply {
        /// Registered source handle.
        source: Field<u32>,
        /// Must match the registration's immutable version.
        version: Field<[u8; 32]>,
        /// Absolute start of the supplied range.
        offset: Field<u64>,
        /// Caller-owned bytes copied only for previously missing intervals.
        bytes: Field<&'a [u8]>,
    },
    /// Declare input complete. Any remaining hole becomes terminal.
    SourceFinish(Field<u32>),
    /// Close input. Issued leases stay readable; dependent future reads fail.
    SourceClose(Field<u32>),
    /// Start the canonical source-backed document opener.
    Open {
        /// Registered source.
        source: Field<u32>,
        /// Explicit validation readiness scope.
        validation: Validation,
        /// Strict validation or reported recovery.
        recovery: Recovery,
        /// Raw password bytes, retained only by the opening job.
        password: &'a [u8],
    },
    /// Resolve one exact page occurrence without enumerating later subtrees.
    Page(PageRef),
    /// Load, prepare and stream one page or tile through the canonical producer.
    Render {
        /// Source document and exact page ordinal.
        page: PageRef,
        /// Device pixels per default user-space unit.
        scale: Field<f64>,
        /// White or transparent background.
        background: Field<PngBackground>,
        /// Final premultiplied bands or encoded PNG chunks.
        output: Field<crate::protocol::OutputCode>,
        /// Optional device rectangle; absent selects the complete page.
        tile: Option<Field<Tile>>,
    },
    /// Advance one bounded unit. A wait completes this request.
    Advance(Field<u32>),
    /// Release exactly the outstanding output sequence before more CPU work.
    Acknowledge {
        /// Job that owns the output slot.
        job: Field<u32>,
        /// Exact output sequence delivered for that slot.
        sequence: Field<u64>,
    },
    /// Cancel one job without closing a shared source or document.
    Cancel(Field<u32>),
    /// Release a job handle and all its remaining demand and output.
    Close(Field<u32>),
    /// Snapshot bounded job/source admission and actual source residency.
    Stats,
}

impl Request<'_> {
    /// The canonical request kind.
    pub const fn kind(self) -> RequestKind {
        match self {
            Self::SourceCreate { .. } => RequestKind::SourceCreate,
            Self::SourceSupply { .. } => RequestKind::SourceSupply,
            Self::SourceFinish(_) => RequestKind::SourceFinish,
            Self::SourceClose(_) => RequestKind::SourceClose,
            Self::Open { .. } => RequestKind::OpenJob,
            Self::Page(_) => RequestKind::PageJob,
            Self::Render { .. } => RequestKind::RenderJob,
            Self::Advance(_) => RequestKind::JobAdvance,
            Self::Acknowledge { .. } => RequestKind::JobAcknowledge,
            Self::Cancel(_) => RequestKind::JobCancel,
            Self::Close(_) => RequestKind::JobClose,
            Self::Stats => RequestKind::JobStats,
        }
    }
}
