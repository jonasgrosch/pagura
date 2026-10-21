//! Stable tags for rendering capabilities the renderer does not implement.
//! This crate owns the `shading.` area; the other tags keep the names the
//! earlier renderer reported for the same faults, so callers and fixtures
//! see no change, and move here from `pagura-render` with the code that
//! raises them.

use pagura_core::Feature;

/// Shadings whose shading space or `Matrix` maps to device space singularly.
pub const SHADING_TRANSFORM: Feature = Feature::new("shading.transform");

/// Stroking under a singular CTM cannot recover the pen's user space.
pub const SINGULAR_STROKE: Feature = Feature::new("graphics.singular-stroke");

/// Transfer-function selection (11.7.5.2) where it is not defined: across a
/// fractional boundary of a mark that selects a transfer function, for
/// pattern paints and for simulated overprinting on a page that selects
/// transfer functions.
pub const TRANSPARENCY_TRANSFER: Feature = Feature::new("transparency.transfer");

/// Tiling patterns whose device lattice cannot be placed (singular matrix).
pub const PATTERN_TILING: Feature = Feature::new("pattern.tiling");

/// An image mask painted with a tiling pattern (8.9.6.2).
pub const PATTERN_IMAGE_MASK: Feature = Feature::new("pattern.image-mask");

/// Every tag above.
pub const ALL: &[Feature] = &[
    SHADING_TRANSFORM,
    SINGULAR_STROKE,
    TRANSPARENCY_TRANSFER,
    PATTERN_TILING,
    PATTERN_IMAGE_MASK,
];
