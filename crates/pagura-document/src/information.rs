//! Stable tags for informational diagnostics: permitted behaviour that is
//! reported in both recovery modes and never needs repair permission
//! (`DiagnosticKind::Information`, ADR 0003).

/// A page's `/Contents` refers to an object the file does not define. The
/// reference is null (7.3.10), so the page has no content (Table 31); the
/// report keeps a page whose content went missing from looking complete.
pub const CONTENTS_UNDEFINED: &str = "pages.contents-undefined";

/// Every information tag defined by this crate.
pub const ALL: &[&str] = &[CONTENTS_UNDEFINED];
