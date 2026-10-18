//! Stable tags for image capabilities outside the current sample model.
use pagura_core::Feature;

/// JPEG 2000 precision or component geometry cannot be represented.
pub const JPX_SAMPLE_LAYOUT: Feature = Feature::new("image.JPX-sample-layout");
/// JPEG 2000 colour information cannot be represented.
pub const JPX_COLOUR: Feature = Feature::new("image.JPX-colour");

/// An unfiltered inline image needs resource colour-space resolution to size its data.
pub const INLINE_COLOUR_LENGTH: Feature = Feature::new("image.inline-colour-length");
