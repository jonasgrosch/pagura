#![forbid(unsafe_code)]
//! PDF-agnostic, antialiased 2D rasterization (ADR 0004).
//!
//! Callers describe geometry in device space through a [`Transform`];
//! this crate turns paths into exact-area pixel coverage and composites
//! paint into a [`Pixmap`]. It knows nothing about PDF objects, colour
//! spaces or content streams.

mod adjust;
mod bounds;
mod coverage;
mod empty_fill;
mod flatten;
mod geometry;
mod glyph;
mod paint;
mod path;
mod pixmap;
mod raster;
mod stroke;

pub use geometry::{Point, Transform};
pub use glyph::{GlyphContour, GlyphGeometry};
pub use paint::{Paint, Rgba8};
pub use path::{FillRegion, Path, PathBuilder, Segment};
pub use pixmap::Pixmap;
pub use raster::{
    ClipRegion, Coverage, FillRule, LineCap, LineJoin, Mask, Rasterizer, Rectangle, Stroke,
    adjusted_stroke_outline, empty_fill_outline, fill_path, fill_path_at, stroke_outline,
    stroke_path, stroke_path_adjusted_at, stroke_path_at, tiled_stroke_outline,
};

#[cfg(test)]
mod tests;

/// Stable raster capability tags.
pub mod features {
    use pagura_core::Feature;
    /// Retained capability tag for device stroke fitting policies.
    pub const STROKE_ADJUSTMENT: Feature = Feature::new("raster.stroke-adjustment-geometry");
}
