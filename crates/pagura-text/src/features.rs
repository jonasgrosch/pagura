//! Stable tags for font capabilities not implemented yet (ADR 0003).
use pagura_core::Feature;

/// A font whose program is not embedded and has no substitute yet.
pub const NON_EMBEDDED: Feature = Feature::new("font.non-embedded");
/// Vertical substitute glyph forms are unavailable; embedded vertical fonts
/// keep their existing support.
pub const CID_SUBSTITUTE_VERTICAL: Feature = Feature::new("font.cid-substitute-vertical");
/// A CMap requires PostScript evaluation beyond declarative mappings.
pub const CMAP_PROGRAM: Feature = Feature::new("font.cmap-program");
/// A predefined CMap other than Identity-H/V, whose data is not bundled.
pub const PREDEFINED_CMAP: Feature = Feature::new("font.predefined-cmap");

/// A recovered descriptor cannot prove the source-named glyph without a fallback.
pub const DESCRIPTOR_GLYPH: Feature = Feature::new("font.descriptor-glyph");

/// Every feature tag defined by this crate.
pub const ALL: &[Feature] = &[
    NON_EMBEDDED,
    DESCRIPTOR_GLYPH,
    CID_SUBSTITUTE_VERTICAL,
    CMAP_PROGRAM,
    PREDEFINED_CMAP,
];
