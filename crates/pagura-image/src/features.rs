//! Stable tags for image capabilities outside the current sample model.
use pagura_core::Feature;

/// Embedded opacity requires separately addressable alpha samples.
pub const EMBEDDED_ALPHA: Feature = Feature::new("image.embedded-alpha");
/// JPEG 2000 precision or component geometry cannot be represented.
pub const JPX_SAMPLE_LAYOUT: Feature = Feature::new("image.JPX-sample-layout");
/// JPEG 2000 colour information cannot be represented.
pub const JPX_COLOUR: Feature = Feature::new("image.JPX-colour");
/// Arithmetic lossless or hierarchical JPEG coding (T.81 H/J).
pub const JPEG_ARITHMETIC: Feature = Feature::new("jpeg.arithmetic");
/// DCT JPEG precision outside the supported eight-/twelve-bit processes.
pub const JPEG_PRECISION: Feature = Feature::new("jpeg.precision");
/// Hierarchical JPEG coding or height deferred to DNL.
pub const JPEG_PROCESS: Feature = Feature::new("jpeg.process");
/// CCITT two-dimensional extension mode.
pub const CCITT_EXTENSION: Feature = Feature::new("ccitt.extension");

/// JPEG component layouts outside the one-, three- and four-plane model.
pub const JPEG_COMPONENTS: Feature = Feature::new("jpeg.components");

/// An unfiltered inline image needs resource colour-space resolution to size its data.
pub const INLINE_COLOUR_LENGTH: Feature = Feature::new("image.inline-colour-length");
