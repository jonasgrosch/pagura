#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! PDF-agnostic, antialiased 2D rasterization (ADR 0004, ADR 0034).
//!
//! Callers describe geometry in user space and map it to device space
//! through a [`Transform`]: paths, fill rules, strokes and glyph outlines.
//! The [band rasterizer](BandRasterizer) turns prepared [`Shape`]s and
//! [`ClipShape`]s into exact-area coverage, band by band; [`Pixmap`] is the
//! 8-bit output surface. It knows nothing about PDF objects, colour spaces or
//! content streams.

mod adjust;
mod band;
mod bounds;
mod empty_fill;
pub mod features;
mod flatten;
mod geometry;
mod path;
mod pixmap;
mod quantize;
mod stroke;

pub use band::{
    Adjustment, Area, BandCoverage, BandRasterizer, Capacity, ClipShape, CoverageRow, DeviceRect,
    GlyphEdges, GlyphMask, GlyphMasks, GlyphOrigins, MarkCoverage, Precision, RunGlyph, Shape,
    ShapeOptions, Span,
};
pub use empty_fill::empty_fill_outline;
pub use geometry::{Point, Transform};
pub use path::{FillRegion, FillRule, NoCurrentPoint, Path, PathBuilder, Segment};
pub use pixmap::{Pixmap, Rgba8};
pub use quantize::quantize_unit;
pub use stroke::{LineCap, LineJoin, Stroke, adjusted_stroke_outline, stroke_outline};

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
