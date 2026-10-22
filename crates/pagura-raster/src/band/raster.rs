//! The band rasterizer: one dense cell row, reused for every mark and band.

use super::approximate::{self, Multiplied};
use super::binned;
use super::clip::{INSIDE, Rows, Segment};
use super::edges::Bounds;
use super::rect;
use super::row::{Row, Sink};
use super::shape::{Body, EdgeBody, Shape};
use super::{Area, BandCoverage, ClipShape, MarkCoverage};
use pagura_core::{Meter, Result};

/// The band rasterizer: a dense band-width cell row with a touched-cell
/// bitmap, reused for every mark of a render.
#[derive(Debug)]
pub struct BandRasterizer {
    area: Area,
    row: Row,
    cells: approximate::Scratch,
    cache: binned::Cache,
    work: usize,
}

impl BandRasterizer {
    /// Scratch storage for `area`, charged to `bytes` (`render_bytes`):
    /// one row of cells up to the region's right edge and its bitmap, one
    /// row of approximate cells, and the bounded cache of binned glyph
    /// masks. The charge depends on the region's right edge only, never on
    /// the band height or the region's rows.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when the storage exceeds `bytes`.
    pub fn new(area: Area, bytes: &mut Meter) -> Result<Self> {
        let width = super::row::width(area.region.x1)?;
        Ok(Self {
            area,
            row: Row::new(width, bytes)?,
            cells: approximate::Scratch::new(width, bytes)?,
            cache: binned::Cache::new(bytes)?,
            work: 0,
        })
    }

    /// The area.
    pub fn area(&self) -> Area {
        self.area
    }

    /// Work units spent by every [`Self::rasterize`] call so far, as
    /// [`Shape::work`] and [`MarkCoverage::work`] bound them.
    pub fn work_done(&self) -> usize {
        self.work
    }

    /// Bytes retained.
    pub fn retained_bytes(&self) -> usize {
        self.row
            .retained_bytes()
            .saturating_add(self.cells.retained_bytes())
            .saturating_add(self.cache.retained_bytes())
    }

    /// Writes the coverage of `mark` within `band` to `out`, replacing its
    /// contents.
    pub fn rasterize(&mut self, mark: MarkCoverage<'_>, band: u32, out: &mut BandCoverage) {
        out.clear();
        let rows = self.area.band_rows(band);
        let region = self.area.region;
        match (mark.shape, mark.clip) {
            (None, None) => {
                for y in rows {
                    out.push_row(y);
                    out.put_run(region.x0, region.width(), u8::MAX);
                    self.work = self.work.saturating_add(2);
                }
            }
            (None, Some(clip)) => self.clip(clip, rows, out),
            (Some(shape), clip) => {
                debug_assert!(
                    match (shape.clip.as_ref().or(shape.multiplied.as_ref()), clip) {
                        (Some(a), Some(b)) => a.same(b),
                        (None, None) => true,
                        _ => false,
                    },
                    "a shape is rasterized with the clip it was prepared against"
                );
                debug_assert_eq!(shape.area, self.area, "a shape of another area");
                let bounds = shape.bounds;
                let rows = rows.start.max(bounds.y0)..rows.end.min(bounds.y1);
                if rows.is_empty() {
                    out.finish();
                    return;
                }
                match &shape.body {
                    Body::Rect(r) => self.rect(*r, shape, rows, out),
                    Body::Edges(edges) => self.edges(edges, shape, (band, rows), out),
                    Body::Cells(cells) => {
                        self.work = self.work.saturating_add(self.cells.rasterize(
                            cells,
                            shape,
                            (band, rows),
                            out,
                        ));
                    }
                    Body::Binned(placement) => {
                        self.work = self.work.saturating_add(self.cache.rasterize(
                            placement,
                            shape,
                            rows,
                            (out, self.cells.row_buffer()),
                        ));
                    }
                }
            }
        }
        out.finish();
    }

    fn rect(
        &mut self,
        r: Bounds,
        shape: &Shape,
        rows: std::ops::Range<u32>,
        out: &mut BandCoverage,
    ) {
        for y in rows {
            out.push_row(y);
            let mut sink = sink(out, shape, y, self.cells.row_buffer());
            let runs = rect::emit_row(r, y, &mut sink);
            self.work = self
                .work
                .saturating_add(runs + 2)
                .saturating_add(sink.extra());
        }
    }

    fn edges(
        &mut self,
        edges: &EdgeBody,
        shape: &Shape,
        (band, rows): (u32, std::ops::Range<u32>),
        out: &mut BandCoverage,
    ) {
        let _ = band;
        let count = edges.lines.len();
        for y in rows {
            let top = f64::from(y);
            let mut cells = 0;
            let items = edges.index.row(y);
            for &item in items {
                let item = item as usize;
                cells += match edges.lines.get(item) {
                    Some(&line) => self.row.line(line, top),
                    None => edges
                        .curves
                        .get(item - count)
                        .map_or(0, |&q| self.row.curve(q, top)),
                };
            }
            out.push_row(y);
            let mut sink = sink(out, shape, y, self.cells.row_buffer());
            let touched = self.row.emit(edges.scan, shape.rule, &mut sink);
            let words = (edges.scan.1.saturating_sub(edges.scan.0)) / 64 + 2;
            self.work = self
                .work
                .saturating_add(items.len())
                .saturating_add(cells)
                .saturating_add(touched.saturating_mul(3))
                .saturating_add(words + 2)
                .saturating_add(sink.extra());
        }
    }

    fn clip(&mut self, clip: &ClipShape, rows: std::ops::Range<u32>, out: &mut BandCoverage) {
        let bounds = clip.bounds();
        let rows = rows.start.max(bounds.y0)..rows.end.min(bounds.y1);
        for y in rows {
            out.push_row(y);
            let mut sink = Out {
                coverage: out,
                x0: bounds.x0 as usize,
                x1: bounds.x1 as usize,
            };
            if let Some(product) = clip.product() {
                let runs = product.emit_row(
                    y,
                    clip.rect(),
                    (bounds.x0 as usize, bounds.x1 as usize),
                    &mut sink,
                );
                self.work = self.work.saturating_add(runs + 2);
                continue;
            }
            if let Some(rows) = clip.rows() {
                let segments = rows.row(y);
                for s in segments {
                    let value = if s.cell == INSIDE {
                        u8::MAX
                    } else {
                        rows.cell(s.cell).map_or(0, |(c, _, _)| c.value)
                    };
                    sink.run(s.x0 as usize, s.x1 as usize, value);
                }
                self.work = self.work.saturating_add(segments.len() + 2);
                continue;
            }
            let whole = Bounds {
                left: f64::from(bounds.x0),
                top: f64::from(bounds.y0),
                right: f64::from(bounds.x1),
                bottom: f64::from(bounds.y1),
            };
            let rect = clip.rect().map_or(whole, |r| r.intersect(whole));
            let runs = rect::emit_row(rect, y, &mut sink);
            self.work = self.work.saturating_add(runs + 2);
        }
    }
}

/// Writes runs to the current row of a coverage, within columns `x0..x1`.
pub(crate) struct Out<'a> {
    pub(crate) coverage: &'a mut BandCoverage,
    pub(crate) x0: usize,
    pub(crate) x1: usize,
}

impl Sink for Out<'_> {
    #[inline]
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        let a = x0.max(self.x0);
        let b = x1.min(self.x1);
        if a < b {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "columns lie within the u32 region"
            )]
            self.coverage.put_run(a as u32, (b - a) as u32, value);
        }
    }
}

/// The sink of one row of `shape`: its cells resolved at preparation and
/// its clip's rows, where it has them, over the output, or its multiplied
/// clip.
pub(crate) fn sink<'a>(
    out: &'a mut BandCoverage,
    shape: &'a Shape,
    y: u32,
    buffer: &'a mut Vec<u8>,
) -> RowSink<'a> {
    if shape.multiplied.is_some() {
        return RowSink::Multiplied(Multiplied::new(out, shape, y, buffer));
    }
    let rows = shape
        .clip
        .as_ref()
        .filter(|_| shape.overlay)
        .and_then(ClipShape::rows);
    RowSink::Overlay(Overlay {
        out: Out {
            coverage: out,
            x0: shape.bounds.x0 as usize,
            x1: shape.bounds.x1 as usize,
        },
        stored: shape.stored.row(y),
        next: 0,
        clip: rows.map(|rows| (rows, rows.row(y))),
        segment: 0,
        extra: 0,
    })
}

/// A shape row's sink.
pub(crate) enum RowSink<'a> {
    Overlay(Overlay<'a>),
    Multiplied(Multiplied<'a>),
}

impl RowSink<'_> {
    pub(crate) fn extra(&self) -> usize {
        match self {
            Self::Overlay(o) => o.extra(),
            Self::Multiplied(m) => m.work(),
        }
    }
}

impl Sink for RowSink<'_> {
    #[inline]
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        match self {
            Self::Overlay(o) => o.run(x0, x1, value),
            Self::Multiplied(m) => m.run(x0, x1, value),
        }
    }
}

/// Runs of a shape's row with its resolved cells substituted and its clip's
/// rows applied: inside runs keep the shape's value, a boundary cell takes
/// the clip's value where the shape covers it fully, outside is dropped.
pub(crate) struct Overlay<'a> {
    out: Out<'a>,
    stored: &'a [(u32, u8)],
    next: usize,
    clip: Option<(&'a Rows, &'a [Segment])>,
    segment: usize,
    extra: usize,
}

impl Overlay<'_> {
    /// Work units spent on resolved cells and clip runs.
    pub(crate) fn extra(&self) -> usize {
        self.extra
    }
}

impl Sink for Overlay<'_> {
    fn run(&mut self, x0: usize, x1: usize, value: u8) {
        if self.stored.is_empty() && self.clip.is_none() {
            self.out.run(x0, x1, value);
            return;
        }
        let mut x = x0;
        while x < x1 {
            while self
                .stored
                .get(self.next)
                .is_some_and(|&(s, _)| (s as usize) < x)
            {
                self.next += 1;
                self.extra += 1;
            }
            let stored = self.stored.get(self.next).copied();
            if let Some((s, v)) = stored
                && s as usize == x
            {
                self.out.run(x, x + 1, v);
                self.next += 1;
                self.extra += 1;
                x += 1;
                continue;
            }
            let limit = stored.map_or(x1, |(s, _)| (s as usize).min(x1));
            let Some((rows, segments)) = self.clip else {
                self.out.run(x, limit, value);
                x = limit;
                continue;
            };
            while segments
                .get(self.segment)
                .is_some_and(|s| s.x1 as usize <= x)
            {
                self.segment += 1;
                self.extra += 1;
            }
            match segments.get(self.segment) {
                None => x = limit,
                Some(s) if s.x0 as usize > x => x = limit.min(s.x0 as usize),
                Some(s) if s.cell == INSIDE => {
                    let end = limit.min(s.x1 as usize);
                    self.out.run(x, end, value);
                    x = end;
                }
                Some(s) => {
                    let clip = rows.cell(s.cell).map_or(0, |(c, _, _)| c.value);
                    let v = if value == u8::MAX { clip } else { 0 };
                    self.out.run(x, x + 1, v);
                    x += 1;
                }
            }
        }
    }
}
