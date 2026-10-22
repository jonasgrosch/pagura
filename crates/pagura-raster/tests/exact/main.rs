#![expect(
    clippy::unwrap_used,
    reason = "geometry setup and comparisons fail the test loudly"
)]
//! The band rasterizer against the reference's exact coverage
//! (`pagura_reference::exact`): fills, strokes, clipped marks and glyph
//! placements over generated geometry and font outlines, each also checked
//! for invariance across band heights and sub-regions and against its work
//! bound.

mod clips;
mod fills;
mod glyphs;
mod strokes;
mod support;
