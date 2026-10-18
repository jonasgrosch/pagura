//! Stable tags for codec capabilities outside the implemented processes.
use pagura_core::Feature;

/// Arithmetic JPEG coding (T.81 Annex D).
pub const JPEG_ARITHMETIC: Feature = Feature::new("jpeg.arithmetic");
/// JPEG precision other than eight bits per component.
pub const JPEG_PRECISION: Feature = Feature::new("jpeg.precision");
/// Lossless or hierarchical JPEG coding.
pub const JPEG_PROCESS: Feature = Feature::new("jpeg.process");
/// CCITT two-dimensional extension mode.
pub const CCITT_EXTENSION: Feature = Feature::new("ccitt.extension");

/// JPEG component layouts outside the one-, three- and four-plane model.
pub const JPEG_COMPONENTS: Feature = Feature::new("jpeg.components");
