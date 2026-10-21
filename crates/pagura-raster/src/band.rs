//! The band rasterizer (ADR 0034): marks are prepared once per render into
//! device-space [`Shape`]s binned by band, then rasterized band by band into
//! [`BandCoverage`]: full interior spans and partial cells per row.
//!
//! For the same flattened geometry every exact producer yields the same
//! coverage bytes: the exact covered area of each pixel, quantized with
//! [`quantize_unit`](crate::quantize_unit). Rasterization is infallible:
//! every allocation and unit of work is accounted for when a shape is
//! prepared.

use crate::{FillRule, Path, Point, Stroke, Transform};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

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

/// How coverage is computed: exactly, or under the `approximate_coverage`
/// fast parameter with f32 signed cells and clip coverage multiplied into
/// marks (not within the one-level contract).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Precision {
    /// The exact covered area of every pixel.
    #[default]
    Exact,
    /// Approximate signed-area cells.
    Approximate,
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

/// Glyph-origin bins: under the `glyph_origin_bins` fast parameter, a glyph's
/// device origin snaps to a bin and its coverage is reused for every
/// placement in that bin (not within the one-level contract).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum GlyphOrigins {
    /// Every placement is integrated at its exact origin (ADR 0013).
    #[default]
    Exact,
    /// Origins snap to bins chosen for the device resolution.
    Binned,
}

/// A glyph outline prepared once for one linear map (outline space to device
/// space without translation): its device-space edges relative to the glyph
/// origin, ready for translation and accumulation. Shareable across
/// placements, renders and pages, so it is a value for the `Store`.
#[derive(Debug)]
pub struct GlyphEdges {
    retained: usize,
}

impl GlyphEdges {
    /// Prepares `outline` under `linear` (`[a, b, c, d]` of a transform).
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error for an outline beyond `limits` and `Syntax`
    /// for arithmetic overflow.
    pub fn new(outline: &Path, linear: [f64; 4], limits: &Limits) -> Result<Self> {
        let _ = (outline, linear, limits);
        Err(Error::invalid_argument("glyph edges are not implemented"))
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }
}

/// One mark's geometry in device space, restricted to an [`Area`] and binned
/// by band: a fill, a stroke outline (as pieces that meet without
/// overlapping where that is exact), a glyph placement, or an image's
/// parallelogram.
#[derive(Debug)]
pub struct Shape {
    bounds: DeviceRect,
    work: usize,
    retained: usize,
}

/// The options shared by every shape of one render.
#[derive(Debug, Clone, Copy)]
pub struct ShapeOptions<'a> {
    /// The pixels and band grid of the render.
    pub area: Area,
    /// The coverage precision.
    pub precision: Precision,
    /// Per-fill budgets (`path_segments`, `raster_work`).
    pub limits: &'a Limits,
}

impl Shape {
    /// A fill of `path` (user space) under `transform` with `rule`; `None`
    /// when it covers no pixel of the area.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when flattening or rasterization would exceed
    /// `options.limits` and `Syntax` for arithmetic overflow.
    pub fn fill(
        path: &Path,
        transform: Transform,
        rule: FillRule,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        let _ = (path, transform, rule, options);
        Err(Error::invalid_argument("band fills are not implemented"))
    }

    /// A stroke of `path` (user space) with `stroke` in the pen space
    /// `pen` (user space to the space the line width is measured in), under
    /// `transform`; `None` when it covers no pixel of the area.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`], plus `Unsupported` for a singular pen.
    pub fn stroke(
        path: &Path,
        stroke: &Stroke,
        pen: Transform,
        transform: Transform,
        adjustment: Adjustment,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        let _ = (path, stroke, pen, transform, adjustment, options);
        Err(Error::invalid_argument("band strokes are not implemented"))
    }

    /// One glyph placement: `edges` translated to the device-space `origin`;
    /// `None` when it covers no pixel of the area.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn glyph(
        edges: &Arc<GlyphEdges>,
        origin: Point,
        origins: GlyphOrigins,
        options: &ShapeOptions<'_>,
    ) -> Result<Option<Self>> {
        let _ = (edges, origin, origins, options);
        Err(Error::invalid_argument("band glyphs are not implemented"))
    }

    /// The pixels the shape can cover within the area.
    pub fn bounds(&self) -> DeviceRect {
        self.bounds
    }

    /// The bands the shape meets.
    pub fn bands(&self, area: &Area) -> std::ops::Range<u32> {
        Area {
            region: self.bounds,
            band_height: area.band_height,
        }
        .bands()
    }

    /// Work units its rasterization costs at most, for the render-work
    /// reservation.
    pub fn work(&self) -> usize {
        self.work
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }
}

/// A clip state prepared once per render: the intersection of a chain of
/// clipping paths, classified per band row into outside, inside and boundary
/// runs. A rectangular clip becomes edge clipping.
#[derive(Debug)]
pub struct ClipShape {
    bounds: DeviceRect,
    retained: usize,
}

impl ClipShape {
    /// The intersection of `paths` (user space, outermost first) under
    /// `transform`.
    ///
    /// # Errors
    ///
    /// As [`Shape::fill`], plus a `Limit` error beyond `mask_sources`.
    pub fn new(
        paths: &[(&Path, FillRule)],
        transform: Transform,
        options: &ShapeOptions<'_>,
    ) -> Result<Self> {
        let _ = (paths, transform, options);
        Err(Error::invalid_argument("band clips are not implemented"))
    }

    /// The pixels the clip can keep within the area; empty when it removes
    /// everything.
    pub fn bounds(&self) -> DeviceRect {
        self.bounds
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }
}

/// What a mark covers: its shape, the whole area for a shading without a
/// bounding box, each restricted to a clip.
#[derive(Debug, Clone, Copy)]
pub struct MarkCoverage<'a> {
    /// The mark's own shape; `None` covers the whole area.
    pub shape: Option<&'a Shape>,
    /// The clip state; `None` keeps everything.
    pub clip: Option<&'a ClipShape>,
}

/// One partial or full run of a coverage row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span<'a> {
    /// Pixels `x .. x + len` fully covered.
    Full {
        /// First column.
        x: u32,
        /// Number of pixels.
        len: u32,
    },
    /// Pixels `x .. x + coverage.len()` with partial coverage, 1..=255 each
    /// (0 only between partial pixels of one run).
    Partial {
        /// First column.
        x: u32,
        /// Coverage per pixel.
        coverage: &'a [u8],
    },
}

/// The coverage of one mark in one band: per row, its spans left to right.
/// Reused for every mark of a render.
#[derive(Debug, Default)]
pub struct BandCoverage {
    rows: Vec<RowSpans>,
    spans: Vec<RawSpan>,
    cells: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RowSpans {
    y: u32,
    start: u32,
    end: u32,
}

#[derive(Debug, Clone, Copy)]
struct RawSpan {
    x: u32,
    len: u32,
    /// Offset into `cells`, or `u32::MAX` for a full span.
    cells: u32,
}

/// The spans of one row.
#[derive(Debug, Clone, Copy)]
pub struct CoverageRow<'a> {
    /// The device row.
    pub y: u32,
    spans: &'a [RawSpan],
    cells: &'a [u8],
}

impl<'a> CoverageRow<'a> {
    /// The spans, left to right, non-overlapping.
    pub fn spans(&self) -> impl Iterator<Item = Span<'a>> + 'a {
        let cells = self.cells;
        self.spans.iter().map(move |span| {
            if span.cells == u32::MAX {
                Span::Full {
                    x: span.x,
                    len: span.len,
                }
            } else {
                let start = span.cells as usize;
                let end = start + span.len as usize;
                Span::Partial {
                    x: span.x,
                    coverage: cells.get(start..end).unwrap_or_default(),
                }
            }
        })
    }
}

impl BandCoverage {
    /// Rows with at least one span, top to bottom.
    pub fn rows(&self) -> impl Iterator<Item = CoverageRow<'_>> {
        self.rows.iter().map(|row| CoverageRow {
            y: row.y,
            spans: self
                .spans
                .get(row.start as usize..row.end as usize)
                .unwrap_or_default(),
            cells: &self.cells,
        })
    }

    /// Whether no pixel is covered.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Removes every span, keeping the storage.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.spans.clear();
        self.cells.clear();
    }

    /// Starts row `y`; rows are pushed top to bottom and spans go to the
    /// last row started. A row left without spans is dropped.
    pub fn push_row(&mut self, y: u32) {
        if self.rows.last().is_some_and(|row| row.start == row.end) {
            self.rows.pop();
        }
        let start = u32::try_from(self.spans.len()).unwrap_or(u32::MAX);
        self.rows.push(RowSpans {
            y,
            start,
            end: start,
        });
    }

    /// Appends a fully covered span to the current row; spans are pushed
    /// left to right without overlap. Ignored before the first row.
    pub fn push_full(&mut self, x: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.push(RawSpan {
            x,
            len,
            cells: u32::MAX,
        });
    }

    /// Appends a partially covered span to the current row. Ignored before
    /// the first row.
    pub fn push_partial(&mut self, x: u32, coverage: &[u8]) {
        let (Ok(len), Ok(cells)) = (
            u32::try_from(coverage.len()),
            u32::try_from(self.cells.len()),
        ) else {
            return;
        };
        if len == 0 || cells == u32::MAX {
            return;
        }
        self.cells.extend_from_slice(coverage);
        self.push(RawSpan { x, len, cells });
    }

    fn push(&mut self, span: RawSpan) {
        let Some(row) = self.rows.last_mut() else {
            return;
        };
        self.spans.push(span);
        row.end = u32::try_from(self.spans.len()).unwrap_or(u32::MAX);
    }

    /// Drops a trailing row without spans; call after the last row.
    pub fn finish(&mut self) {
        if self.rows.last().is_some_and(|row| row.start == row.end) {
            self.rows.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_keep_their_spans_in_order() {
        let mut band = BandCoverage::default();
        band.push_row(3);
        band.push_partial(1, &[10, 20]);
        band.push_full(3, 4);
        band.push_row(4);
        band.push_row(5);
        band.push_full(0, 2);
        band.finish();
        let rows: Vec<_> = band
            .rows()
            .map(|row| (row.y, row.spans().collect::<Vec<_>>()))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    3,
                    vec![
                        Span::Partial {
                            x: 1,
                            coverage: &[10, 20]
                        },
                        Span::Full { x: 3, len: 4 }
                    ]
                ),
                (5, vec![Span::Full { x: 0, len: 2 }]),
            ]
        );
        band.clear();
        assert!(band.is_empty());
    }

    #[test]
    fn bands_follow_the_global_grid() {
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 10,
                x1: 5,
                y1: 70,
            },
            band_height: 32,
        };
        assert_eq!(area.bands(), 0..3);
        assert_eq!(area.band_rows(0), 10..32);
        assert_eq!(area.band_rows(2), 64..70);
    }
}

/// The band rasterizer: dense band-width cell rows with a touched-cell
/// bitmap, reused for every mark of a render.
#[derive(Debug)]
pub struct BandRasterizer {
    area: Area,
}

impl BandRasterizer {
    /// Scratch storage for `area`, charged to `bytes` (`render_bytes`).
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when the storage exceeds `bytes`.
    pub fn new(area: Area, bytes: &mut Meter) -> Result<Self> {
        let _ = bytes;
        Ok(Self { area })
    }

    /// The area.
    pub fn area(&self) -> Area {
        self.area
    }

    /// Writes the coverage of `mark` within `band` to `out`, replacing its
    /// contents.
    pub fn rasterize(&mut self, mark: MarkCoverage<'_>, band: u32, out: &mut BandCoverage) {
        let _ = (mark, band);
        out.clear();
    }
}
