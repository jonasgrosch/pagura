//! Stable tags for codec capabilities outside the implemented processes.
use pagura_core::Feature;

/// Arithmetic JPEG coding (T.81 Annex D).
pub const JPEG_ARITHMETIC: Feature = Feature::new("jpeg.arithmetic");
/// JPEG precision other than eight bits per component.
pub const JPEG_PRECISION: Feature = Feature::new("jpeg.precision");
/// Lossless or hierarchical JPEG coding.
pub const JPEG_PROCESS: Feature = Feature::new("jpeg.process");
/// JPEG component layouts outside the one-, three- and four-plane model.
pub const JPEG_COMPONENTS: Feature = Feature::new("jpeg.components");
/// CCITT two-dimensional extension mode (T.4 4.2.1.3.4, T.6 2.2.4).
pub const CCITT_EXTENSION: Feature = Feature::new("ccitt.extension");
/// A JBIG2 extension segment marked necessary whose type is unknown
/// (T.88 7.4.14).
pub const JBIG2_EXTENSION: Feature = Feature::new("jbig2.extension");
/// The extended adaptive template of later T.88 amendments.
pub const JBIG2_EXTENDED_TEMPLATE: Feature = Feature::new("jbig2.extended-template");

/// Every feature tag of this crate.
pub const ALL: &[Feature] = &[
    JPEG_ARITHMETIC,
    JPEG_PRECISION,
    JPEG_PROCESS,
    JPEG_COMPONENTS,
    CCITT_EXTENSION,
    JBIG2_EXTENSION,
    JBIG2_EXTENDED_TEMPLATE,
];
