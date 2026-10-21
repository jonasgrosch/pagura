//! Stable rendering capability tags of the renderer. This crate owns the
//! `shading.` area.

use pagura_core::Feature;

/// Shadings whose shading space or `Matrix` maps to device space singularly.
pub const SHADING_TRANSFORM: Feature = Feature::new("shading.transform");

/// Every tag above.
pub const ALL: &[Feature] = &[SHADING_TRANSFORM];
