//! Stable colour-management capability tags. This crate owns the
//! `colorspace.` area.

use pagura_core::Feature;

/// ICC profile processing outside the supported forward matrix/TRC and LUT subset.
pub const ICC_BASED: Feature = Feature::new("colorspace.ICCBased");

/// Nonzero calibrated black points require black-point compensation.
pub const BLACK_POINT: Feature = Feature::new("colorspace.BlackPoint");

/// Every tag above.
pub const ALL: &[Feature] = &[ICC_BASED, BLACK_POINT];
