#![forbid(unsafe_code)]
//! PDF-agnostic, antialiased 2D rasterization (ADR 0004).
//!
//! Callers describe geometry in device space through a [`Transform`];
//! this crate turns paths into exact-area pixel coverage and composites
//! paint into a [`Pixmap`]. It knows nothing about PDF objects, colour
//! spaces or content streams.

mod bounds;
mod coverage;
mod flatten;
mod geometry;
mod paint;
mod path;
mod pixmap;
mod raster;
mod stroke;

pub use geometry::{Point, Transform};
pub use paint::{Paint, Rgba8};
pub use path::{Path, PathBuilder, Segment};
pub use pixmap::Pixmap;
pub use raster::{
    FillRule, LineCap, LineJoin, Mask, Stroke, fill_path, fill_path_at, stroke_path, stroke_path_at,
};

#[cfg(test)]
mod tests;
