//! Stable tags for appearance generation that is not implemented.
//!
//! This crate owns the `appearance.` area and, among feature tags, the
//! `annotation.` area.

use pagura_core::Feature;

/// Generating an appearance for an annotation without `AP` outside the
/// supported subtypes and borders (12.5.6).
pub const ANNOTATION_APPEARANCE: Feature = Feature::new("annotation.appearance");

/// Oblique, reflected or degenerate text transforms and negative font sizes
/// in a generated form appearance (12.7.4.3).
pub const APPEARANCE_TEXT_MATRIX: Feature = Feature::new("appearance.text-matrix");

/// Default appearance operators outside the supported subset (12.7.4.3).
pub const APPEARANCE_DEFAULT_APPEARANCE: Feature = Feature::new("appearance.default-appearance");

/// Field text without a reversible simple horizontal font encoding.
pub const APPEARANCE_FONT_ENCODING: Feature = Feature::new("appearance.font-encoding");

/// Rounded, beveled and inset widget border generation.
pub const APPEARANCE_BORDER_STYLE: Feature = Feature::new("appearance.border-style");

/// Appearance generation for a rotated widget.
pub const APPEARANCE_WIDGET_ROTATION: Feature = Feature::new("appearance.widget-rotation");

/// Inline images, multiple variable text sections or crossed content scopes
/// in an appearance being updated.
pub const APPEARANCE_UPDATE: Feature = Feature::new("appearance.update");

/// Every tag above.
pub const ALL: &[Feature] = &[
    ANNOTATION_APPEARANCE,
    APPEARANCE_TEXT_MATRIX,
    APPEARANCE_DEFAULT_APPEARANCE,
    APPEARANCE_FONT_ENCODING,
    APPEARANCE_BORDER_STYLE,
    APPEARANCE_WIDGET_ROTATION,
    APPEARANCE_UPDATE,
];
