//! Stable feature and repair tags reported by this crate.

use pagura_core::{Feature, Repair};

/// Cross-reference streams (clause 7.5.8).
pub const XREF_STREAM: Feature = Feature::new("xref.stream");
/// Incremental updates (`/Prev`, clause 7.5.6).
pub const INCREMENTAL: Feature = Feature::new("xref.incremental");
/// Hybrid-reference files (`/XRefStm`, clause 7.5.8.4).
pub const HYBRID: Feature = Feature::new("xref.hybrid");
/// Objects stored in object streams (clause 7.5.7).
pub const OBJECT_STREAM: Feature = Feature::new("object.compressed");
/// Stream data in external files (`/F`, clause 7.3.8.2).
pub const EXTERNAL_STREAM: Feature = Feature::new("stream.external");
/// Encrypted documents (clause 7.6).
pub const ENCRYPTION: Feature = Feature::new("security.encryption");

/// Bytes other than white space after the final `%%EOF`.
pub const TRAILING_BYTES: Repair = Repair::new("file.trailing-bytes");
/// A page tree `/Count` that disagrees with the leaves found.
pub const PAGE_COUNT: Repair = Repair::new("pages.count-mismatch");

/// Header found after leading junk (ADR 0003).
pub const HEADER_OFFSET: Repair = Repair::new("file.header-offset");
/// Cross-reference data reconstructed from unambiguous indirect objects.
pub const XREF_REBUILT: Repair = Repair::new("xref.rebuilt");
/// A nearby matching object header replaces an incorrect offset.
pub const OFFSET_CORRECTED: Repair = Repair::new("xref.offset-corrected");
/// A stream terminator replaces an incorrect length.
pub const LENGTH_CORRECTED: Repair = Repair::new("stream.length-corrected");
/// A missing trailer was recovered from a catalog.
pub const TRAILER_RECONSTRUCTED: Repair = Repair::new("trailer.reconstructed");
/// Unknown non-Standard security handler; the filter name is not a static tag.
pub const UNKNOWN_SECURITY: Feature = Feature::new("security.unknown");
/// Adobe public-key security handler.
pub const PUBSEC: Feature = Feature::new("security.Adobe-PubSec");
/// Recovery cannot safely locate an indirectly sized stream without xref data.
pub const REBUILD_INDIRECT_LENGTH: Feature = Feature::new("recovery.indirect-length");

/// Nonstandard data between the PDF version and its line terminator (7.5.2).
pub const HEADER_FORMAT: Repair = Repair::new("file.header-format");
/// Permissions written as an unsigned 32-bit integer rather than signed (7.6).
pub const PERMISSIONS_UNSIGNED: Repair = Repair::new("security.permissions-unsigned");
/// Binary security fields recovered without literal-string EOL normalization.
pub const LITERAL_HASH: Repair = Repair::new("security.literal-hash");
/// Zero padding beyond the fixed password-hash length is discarded (7.6).
pub const HASH_PADDING: Repair = Repair::new("security.hash-padding");
/// A catalog lacking Type, validated through its Pages reference (7.7.2).
pub const CATALOG_TYPE: Repair = Repair::new("catalog.type-missing");
/// Bare CR after the stream keyword, instead of LF or CRLF (7.3.8.1).
pub const STREAM_EOL: Repair = Repair::new("stream.eol");

/// Xref reconstruction resynchronized an indirectly sized or damaged stream at
/// a validated terminator/object boundary without a preceding EOL (7.3.8).
pub const STREAM_BOUNDARY: Repair = Repair::new("xref.stream-boundary");
