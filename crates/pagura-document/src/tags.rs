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

/// A complete indirect value followed by the next object or xref, without endobj
/// (7.3.10). EOF alone never establishes a recoverable object boundary.
pub const OBJECT_END: Repair = Repair::new("object.end-missing");
/// An unknown catalog Type inferred from its Pages reference (7.7.2).
/// Known conflicting structural types are never replaced.
pub const CATALOG_TYPE_INVALID: Repair = Repair::new("catalog.type-invalid");
/// A missing page-tree Type inferred from Kids or leaf attributes (7.7.3).
pub const PAGE_TYPE: Repair = Repair::new("pages.type-missing");

/// Required MediaBox is missing/invalid: use a valid CropBox or Letter (Table 31).
pub const PAGE_MEDIA_BOX: Repair = Repair::new("pages.media-box");
/// Invalid/disjoint CropBox defaults to MediaBox (Table 31, 14.11.2).
pub const PAGE_CROP_BOX: Repair = Repair::new("pages.crop-box");
/// Repeated node occurrences are preserved in Kids order, within work/page limits (7.7.3).
pub const PAGE_REPEAT: Repair = Repair::new("pages.repeated-node");
/// A cyclic child edge is omitted, provided identifiable leaves survive (7.7.3.2).
pub const PAGE_CYCLE: Repair = Repair::new("pages.cycle");
/// Invalid or undefined Kids entries are omitted; a lone reference is wrapped (7.7.3.2).
pub const PAGE_KIDS: Repair = Repair::new("pages.kids-invalid");
/// Missing Kids recovered from explicit Parent backlinks in object-number order (7.7.3.2).
pub const PAGE_KIDS_REBUILT: Repair = Repair::new("pages.kids-rebuilt");
/// An explicit Parent disagrees with the traversed Kids edge (Tables 30 and 31).
pub const PAGE_PARENT: Repair = Repair::new("pages.parent-mismatch");
/// A Template named in Kids is displayed as a Page (7.7.3.3, 12.7.7).
pub const PAGE_TYPE_INVALID: Repair = Repair::new("pages.type-invalid");

/// Missing white space between endobj and the next object header (7.2.3, 7.3.10).
pub const OBJECT_SEPARATOR: Repair = Repair::new("object.separator-missing");

/// A direct Kids dictionary uses its container's object identity (7.7.3.2).
/// Page indexes distinguish occurrences; direct resources keep separate identities.
pub const PAGE_DIRECT: Repair = Repair::new("pages.direct-kid");

/// Unknown/missing object-stream Type inferred from xref and a valid header (7.5.7).
pub const OBJECT_STREAM_TYPE: Repair = Repair::new("object.stream-type");
/// A leading self header entry is discarded while retaining the physical stream
/// and all original indexes. Other self references and offset aliases fail (7.5.7).
pub const OBJECT_STREAM_SELF_ENTRY: Repair = Repair::new("object.stream-self-entry");

/// Missing V=2 Length uses the unique legal size that authenticates the same
/// credential, instead of the 40-bit default (7.6.2 Table 20, 7.6.4.3.2).
pub const KEY_LENGTH_MISSING: Repair = Repair::new("security.key-length");

/// A plaintext Catalog numbered zero uses the exact positive Root xref slot
/// (7.3.10, 7.5.4, Table 15); other identities and encrypted files are rejected.
pub const CATALOG_OBJECT_NUMBER: Repair = Repair::new("catalog.object-number");

/// Malformed required file identifiers preserved for authenticated recovery (Table 15).
pub const DOCUMENT_ID: Repair = Repair::new("security.document-id");
/// Nonconforming crypt-filter Type recovered from its explicit CF membership (Table 25).
pub const CRYPT_TYPE: Repair = Repair::new("security.crypt-filter-type");
/// Missing required crypt-filter Length inferred from its authenticated file key (Table 25).
pub const CRYPT_LENGTH: Repair = Repair::new("security.crypt-filter-length");
/// A nonstandard Standard-handler crypt-filter name (7.6.4.1).
pub const CRYPT_NAME: Repair = Repair::new("security.crypt-filter-name");
/// EFOpen-only Standard-handler filter accepted as an extension (7.6.4.1, Table 25).
pub const CRYPT_EVENT: Repair = Repair::new("security.crypt-filter-event");
/// Password authenticated but Algorithm 13 permissions consistency failed.
pub const PERMS_MISMATCH: Repair = Repair::new("security.perms-mismatch");
/// Nonstandard V=4 short RC4 key size (7.6.2 Table 20).
pub const SHORT_KEY: Repair = Repair::new("security.short-key");
/// Short RC4 file key padded to 16 bytes for object derivation (7.6.3.2).
pub const KEY_PADDING: Repair = Repair::new("security.key-padding");

/// R2 permission bits violate Table 21's selector; raw hash bytes are preserved.
pub const PERMISSION_SELECTOR: Repair = Repair::new("security.permission-selector");

/// Malformed tree Limits replaced by actual descendant bounds (7.9.6–7.9.7).
pub const TREE_LIMITS: Repair = Repair::new("tree.limits");
/// Tree keys reordered; first physical duplicate retained (7.9.6–7.9.7).
pub const TREE_ORDER: Repair = Repair::new("tree.order");
/// A repeated tree node is omitted (7.9.6–7.9.7).
pub const TREE_CYCLE: Repair = Repair::new("tree.cycle");
/// Direct children and mixed local-entry/child nodes are traversed (7.9.6–7.9.7).
pub const TREE_STRUCTURE: Repair = Repair::new("tree.structure");
/// Forward outline chains prevail over inconsistent reciprocal links (12.3.3).
pub const OUTLINE_LINKS: Repair = Repair::new("outline.links");
/// Repeated outline items are omitted (12.3.3).
pub const OUTLINE_CYCLE: Repair = Repair::new("outline.cycle");
/// Outline Count magnitudes corrected while signs are retained (12.3.3).
pub const OUTLINE_COUNT: Repair = Repair::new("outline.count");
/// Action Next back edges omitted; acyclic reuse is retained (12.6.2).
pub const ACTION_CYCLE: Repair = Repair::new("action.cycle");
/// An A entry takes precedence over a conflicting Dest (12.3.3, 12.5.6.5).
pub const TARGET_CONFLICT: Repair = Repair::new("navigation.target-conflict");
/// Invalid Unicode or undefined PDFDocEncoding bytes become U+FFFD (7.9.2.2).
pub const NAVIGATION_TEXT: Repair = Repair::new("navigation.text");
/// Malformed QuadPoints ignored in favour of Rect (12.5.6.5, Table 176).
pub const LINK_QUADS: Repair = Repair::new("link.quad-points");

/// A local destination uses a valid zero-based page number (12.3.2.2).
pub const DESTINATION_PAGE_NUMBER: Repair = Repair::new("destination.page-number");

/// Incorrect optional outline root Type ignored in Outlines context (12.3.3).
pub const OUTLINE_TYPE: Repair = Repair::new("outline.type");
