//! Stable tags for informational diagnostics: permitted behaviour that is
//! reported in both recovery modes and never needs repair permission
//! (`DiagnosticKind::Information`, ADR 0003).

/// An untouched field keeps its valid stored appearance because generating
/// a replacement is unsupported (12.7.4.3).
pub const APPEARANCE_KEPT: &str = "form.appearance-kept";

/// Every information tag defined by this crate.
pub const ALL: &[&str] = &[APPEARANCE_KEPT];
