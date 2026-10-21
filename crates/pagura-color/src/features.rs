//! Stable colour-management capability tags. This crate owns the
//! `colorspace.` and `function.` areas.

use pagura_core::Feature;

/// ICC profile processing outside the supported forward matrix/TRC and LUT subset.
pub const ICC_BASED: Feature = Feature::new("colorspace.ICCBased");

/// Nonzero calibrated black points require black-point compensation.
pub const BLACK_POINT: Feature = Feature::new("colorspace.BlackPoint");

/// Colour space families outside Table 61, which a later PDF version or an
/// extension may define.
pub const UNKNOWN_FAMILY: Feature = Feature::new("colorspace.unknown");

/// Type 4 calculator programs whose stack effect or operand kinds depend on
/// their data, so that they cannot be validated before evaluation (7.10.5).
pub const CALCULATOR: Feature = Feature::new("function.calculator");

/// Every tag above.
pub const ALL: &[Feature] = &[ICC_BASED, BLACK_POINT, UNKNOWN_FAMILY, CALCULATOR];
