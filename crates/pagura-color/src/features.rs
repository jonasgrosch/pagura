//! Stable colour-management capability tags.

use pagura_core::Feature;

/// ICC profile processing outside the supported forward matrix/TRC and LUT subset.
pub const ICC_BASED: Feature = Feature::new("colorspace.ICCBased");

/// Every tag above.
pub const ALL: &[Feature] = &[ICC_BASED];
