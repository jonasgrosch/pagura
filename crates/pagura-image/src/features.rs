//! Stable tags for image capabilities outside the current sample model.
use pagura_core::Feature;

/// JPEG 2000 precision or component geometry cannot be represented.
pub const JPX_SAMPLE_LAYOUT: Feature = Feature::new("image.JPX-sample-layout");
/// JPEG 2000 color information cannot be represented.
pub const JPX_COLOR: Feature = Feature::new("image.JPX-color");
/// An unfiltered inline image needs resource color-space resolution to size
/// its data.
pub const INLINE_COLOR_LENGTH: Feature = Feature::new("image.inline-color-length");

/// Every feature tag of this crate.
pub const ALL: &[Feature] = &[JPX_SAMPLE_LAYOUT, JPX_COLOR, INLINE_COLOR_LENGTH];
