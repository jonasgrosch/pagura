//! Stable rasterization capability tags. This crate owns the `raster.` area.

use pagura_core::Feature;

/// A stroke whose pen space collapses (a singular line-width transform), so
/// that no pen shape is defined (8.4.3.2).
pub const SINGULAR_PEN: Feature = Feature::new("raster.singular-pen");

/// Every tag above.
pub const ALL: &[Feature] = &[SINGULAR_PEN];
