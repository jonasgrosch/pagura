//! Stable tags for informational diagnostics: permitted behaviour that is
//! reported in both recovery modes and never needs repair permission
//! (`DiagnosticKind::Information`, ADR 0003).

/// An explicitly provided replacement outline program draws a non-embedded
/// font (9.6.2.2, 9.7.4.2; ADR 0009). Substitution is permitted PDF reader
/// behaviour, not a repair of malformed input.
pub const SUBSTITUTED: &str = "font.substituted";

/// A substitute cannot draw every CID reachable through the encoding CMap.
/// Reported once at font load, including notdef coverage, rather than retaining
/// a document reference or reporting repeatedly during extraction. Missing
/// values draw no mark and retain their original PDF metrics in both modes.
/// This describes a provider coverage limitation, not malformed PDF input.
pub const CID_GLYPH_MISSING: &str = "font.cid-glyph-missing";

/// Every information tag defined by this crate.
pub const ALL: &[&str] = &[SUBSTITUTED, CID_GLYPH_MISSING];
