#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! A slow, obviously correct reference renderer for differential tests
//! (ADR 0034). It renders a recorded `pagura_interpret::Scene` in f64 over
//! the whole region with no bands, storage formats or speed tricks:
//!
//! - coverage is the exact area of each pixel's intersection with the mark
//!   and every clipping path, by slab integration, quantized once per mark
//!   as the renderer's coverage bytes are;
//! - curves are flattened at the documented 1/256-pixel tolerance, and the
//!   curves of glyph outlines the renderer integrates as splines 64 times
//!   finer, against their true curves;
//! - every mark composites pixel by pixel with the formulas of clause 11:
//!   blend modes, constant alpha, alpha-is-shape, soft masks, isolated,
//!   non-isolated and knockout groups, transfer functions, overprint with
//!   spot channels, tiling instances by exact geometry;
//! - colours, shadings and images go through `pagura-color`'s definitions
//!   and the contract's sampling definitions (ADR 0004);
//! - the output is composited onto the background and quantized once.
//!
//! [`exact`] gives the coverage of single fills, strokes and glyph
//! placements for differential tests of the band rasterizer.
//!
//! Test only; never a product dependency.

mod area;
mod blend;
mod buffer;
mod color;
pub mod exact;
mod flatten;
mod glyph;
mod image;
mod overprint;
mod pixel;
mod render;
mod request;
mod shading;
mod shapes;

pub use area::Region;
pub use render::render;
pub use request::{Overprint, Quantization, Rendering, Request};
