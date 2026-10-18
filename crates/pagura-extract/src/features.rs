//! Stable tags for page-text capabilities not implemented yet (ADR 0003).
//! Each makes the logical view fall back; page text is unaffected. Their
//! error messages start with the tag, because the reported
//! `text.structure-fallback` repair keeps only the original message.
use pagura_core::Feature;

/// Structure references content this page's interpreter did not capture,
/// such as a marked-content sequence in another content stream (14.7.5).
pub const STRUCTURE_CONTENT: Feature = Feature::new("text.structure-content");
/// A nonstandard structure namespace or a namespace-specific role map
/// (14.7.4, 14.8.6.2).
pub const STRUCTURE_NAMESPACE: Feature = Feature::new("text.structure-namespace");
/// Structure replacement text spanning pages or noncontiguous glyphs, or
/// crossing logical content items (14.9.3–14.9.4).
pub const STRUCTURE_REPLACEMENT: Feature = Feature::new("text.structure-replacement");

/// Every feature tag defined by this crate.
pub const ALL: &[Feature] = &[
    STRUCTURE_CONTENT,
    STRUCTURE_NAMESPACE,
    STRUCTURE_REPLACEMENT,
];
