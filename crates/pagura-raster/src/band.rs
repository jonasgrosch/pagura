//! The band rasterizer (ADR 0034): marks are prepared once per render into
//! device-space [`Shape`]s binned by band, then rasterized band by band into
//! [`BandCoverage`]: full interior spans and partial cells per row.
//!
//! # Coverage
//!
//! A pixel's coverage is its exact covered area (curves flattened within
//! 1/256 pixel, glyph quadratics integrated against the true curve),
//! quantized once with [`quantize_unit`](crate::quantize_unit). Each mark's
//! edges, listed per strip of 16 rows on the global grid, add their signed
//! areas to a strip of cells with a bitmap of touched cells per row, in the
//! edges' own order and without sorting; a prefix sum from the mark's
//! leftmost column gives each cell's integral of the winding number, and
//! runs between touched cells leave as spans.
//!
//! The signed area is the covered area wherever the winding takes at most
//! two neighbouring values. Preparation certifies that for the whole shape
//! (winding 0 or one sign everywhere, or stroke pieces that meet without
//! overlapping); otherwise it analyses each cell: one monotone chain per
//! cell is enough, and the cells where two chains meet go to an exact local
//! resolver, whose results the shape keeps. Clips are prepared once per clip
//! state into rows of outside, inside and boundary runs; rectangular clips
//! become edge clipping, and where a mark and a clip boundary both cross a
//! pixel the resolver integrates their joint geometry.
//!
//! Every cell is a function of the geometry, the cell and the rule alone:
//! neither the band height nor the requested region changes any arithmetic,
//! only which cells are visited. Rasterization is infallible and does not
//! allocate: preparation validates, charges and bounds everything.

mod cells;
mod certify;
mod clip;
mod convex;
mod coverage;
mod curve;
mod curved;
mod edges;
mod glyph;
mod raster;
mod rect;
mod repeat;
mod row;
mod shape;
mod union;

pub use clip::ClipShape;
pub use coverage::{BandCoverage, Capacity, CoverageRow, RowOut, Span};
pub use curved::ClipSource;
pub(crate) use edges::{Mapped, Polygon};
pub use glyph::GlyphEdges;
pub use raster::BandRasterizer;
pub use repeat::StrokeTemplates;
pub use shape::Shape;
pub(crate) use shape::{RASTER_WORK, RENDER_BYTES};

use pagura_core::Limits;

/// A rectangle of device pixels, `x0..x1` × `y0..y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DeviceRect {
    /// First column.
    pub x0: u32,
    /// First row.
    pub y0: u32,
    /// One past the last column.
    pub x1: u32,
    /// One past the last row.
    pub y1: u32,
}

impl DeviceRect {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }

    /// Whether the rectangle holds no pixel.
    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// The pixels in both rectangles; empty (all zero) when they are
    /// disjoint.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Self {
        let rect = Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        if rect.is_empty() {
            Self::default()
        } else {
            rect
        }
    }
}

/// The pixels a render produces and the global band grid they are cut into.
/// Band `k` holds device rows `k × height .. (k + 1) × height`, whatever the
/// region, so coverage never depends on how a page is partitioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Area {
    /// The requested device pixels.
    pub region: DeviceRect,
    /// Rows per band.
    pub band_height: u32,
}

impl Area {
    /// The bands the region meets.
    pub fn bands(&self) -> std::ops::Range<u32> {
        if self.region.is_empty() || self.band_height == 0 {
            return 0..0;
        }
        self.region.y0 / self.band_height..(self.region.y1 - 1) / self.band_height + 1
    }

    /// The region's rows within band `band`.
    pub fn band_rows(&self, band: u32) -> std::ops::Range<u32> {
        let start = band.saturating_mul(self.band_height).max(self.region.y0);
        let end = band
            .saturating_add(1)
            .saturating_mul(self.band_height)
            .min(self.region.y1);
        start..end.max(start)
    }
}

/// Stroke adjustment (10.7.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Adjustment {
    /// Strokes are outlined as drawn.
    #[default]
    None,
    /// Thin strokes are adjusted to whole device pixels.
    Adjusted,
}

/// The options shared by every shape of one render.
#[derive(Debug, Clone, Copy)]
pub struct ShapeOptions<'a> {
    /// The pixels and band grid of the render.
    pub area: Area,
    /// Per-fill budgets (`path_segments`, `raster_work`, `mask_sources`).
    pub limits: &'a Limits,
}

/// What preparing one shape needs: the render's options and the clip state
/// the mark is painted in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Prep<'a> {
    pub(crate) area: Area,
    pub(crate) limits: &'a Limits,
    pub(crate) clip: Option<&'a ClipShape>,
}

impl<'a> Prep<'a> {
    pub(crate) fn new(options: &ShapeOptions<'a>, clip: Option<&'a ClipShape>) -> Self {
        Self {
            area: options.area,
            limits: options.limits,
            clip,
        }
    }
}

/// What a mark covers: its shape, the whole area for a shading without a
/// bounding box, each restricted to a clip.
#[derive(Debug, Clone, Copy)]
pub struct MarkCoverage<'a> {
    /// The mark's own shape; `None` covers the whole area.
    pub shape: Option<&'a Shape>,
    /// The clip state. A shape uses the clip it was prepared against
    /// (`Shape::*_with_clip`), which must be this one; `None` keeps
    /// everything.
    pub clip: Option<&'a ClipShape>,
}

impl MarkCoverage<'_> {
    /// Work units rasterizing every band of `area` costs at most, for the
    /// render-work reservation.
    pub fn work(&self, area: &Area) -> usize {
        match (self.shape, self.clip) {
            (Some(shape), _) => shape.work(),
            (None, Some(clip)) => clip.work(),
            (None, None) => area.region.height() as usize * 2,
        }
    }
}

/// Test coverage of a prepared shape using its compact inspection window.
/// Coordinates and accumulation stay in device space, including large origins.
#[cfg(test)]
pub(crate) fn inspected_coverage(shape: &Shape, area: Area) -> Vec<u8> {
    let scan = match &shape.body {
        shape::Body::Edges(body) => body.scan,
        shape::Body::Glyph(body) => body.scan,
        shape::Body::Certified(_) => (shape.bounds.x0 as usize, shape.bounds.x1 as usize),
    };
    let mut bytes = crate::testing::bytes();
    let mut cells = cells::Cells::new(scan.1.saturating_sub(scan.0), &mut bytes).unwrap();
    let region = area.region;
    let width = region.width() as usize;
    let mut coverage = vec![0; width * region.height() as usize];
    raster::inspect(shape, &mut cells, |y, x0, x1, value, _| {
        let row = (y - region.y0) as usize * width;
        let first = row + x0 - region.x0 as usize;
        let last = row + x1 - region.x0 as usize;
        coverage[first..last].fill(crate::quantize_unit(value));
    })
    .unwrap();
    coverage
}

#[cfg(test)]
mod tests;
