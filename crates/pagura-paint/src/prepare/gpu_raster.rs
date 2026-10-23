//! Experimental GPU coverage packet. Geometry comes from the CPU preparer's
//! shapes; this module never invokes BandRasterizer or creates coverage spans.
use crate::{
    PreparedScene,
    plan::{ItemKind, Mark},
};
use pagura_core::{Error, Feature, Limits, Meter, Result};
use pagura_raster::{DeviceRect, FillRule};

mod shared;

const HEADER: usize = 24;
const MARK: usize = 16;
const EDGE: usize = 8;
const TILE_X: u32 = 32;
const TILE_Y: u32 = 8;

impl PreparedScene {
    /// Exports lines, true quadratics and sparse prepared corrections for GPU
    /// area integration. `local` subtracts each shape's integer origin in f64
    /// before conversion to f32; false measures absolute-coordinate f32.
    /// This is an approximate experimental backend, never Accuracy::Exact.
    ///
    /// # Errors
    /// Unsupported PDF semantics retain the existing CPU admission policy;
    /// packet memory and conservative shader work have independent limits.
    pub fn gpu_raster_program(&self, limits: &Limits, local: bool) -> Result<Vec<u32>> {
        self.gpu_geometry_program(limits, local, false)
    }

    /// Exports shared certified glyph sources and edge references, for a
    /// retained GPU executor. Approximate local f32 coverage, never Exact.
    ///
    /// # Errors
    /// As [`Self::gpu_raster_program`].
    pub fn gpu_shared_program(&self, limits: &Limits) -> Result<Vec<u32>> {
        self.gpu_geometry_program(limits, true, true)
    }

    fn gpu_geometry_program(&self, limits: &Limits, local: bool, shared: bool) -> Result<Vec<u32>> {
        if let Some(reason) = self.gpu_fallback_reason() {
            return Err(Error::unsupported(Feature::new("gpu.raster"), reason));
        }
        if self.background()[3] != 255 {
            return Err(Error::unsupported(
                Feature::new("gpu.raster"),
                "GPU coverage experiment requires an opaque background",
            ));
        }
        let region = self.region();
        let columns = region.width().div_ceil(TILE_X);
        let rows = region.height().div_ceil(TILE_Y);
        let mut b = Builder {
            region,
            local,
            shared,
            edge_refs: Vec::new(),
            sources: Vec::new(),
            columns,
            marks: Vec::new(),
            edges: Vec::new(),
            extras: Vec::new(),
            bins: Vec::new(),
            bytes: Meter::new(limits.render_bytes, "GPU geometry byte budget exceeded"),
            work: Meter::new(limits.render_work, "GPU geometry work budget exceeded"),
            correction_cells: 0,
            clip_runs: 0,
        };
        let count = (columns as usize)
            .checked_mul(rows as usize)
            .ok_or(Error::limit("GPU tile count"))?;
        b.bytes.reserve(&mut b.bins, count)?;
        b.bins.resize_with(count, Vec::new);
        for &id in &self.plan.groups[0].items {
            match &self.plan.items[id as usize].kind {
                ItemKind::Mark(mark) => b.draw(self, mark.cover.shape, mark)?,
                ItemKind::Run(run) => {
                    for glyph in &run.glyphs {
                        b.draw(self, Some(glyph.shape), &run.mark)?;
                    }
                }
                ItemKind::Group(_) => {
                    return Err(Error::invalid_argument("GPU raster admission missed group"));
                }
            }
        }
        b.finish(self.background())
    }
}

struct Builder {
    region: DeviceRect,
    local: bool,
    shared: bool,
    edge_refs: Vec<u32>,
    sources: Vec<(*const pagura_raster::GlyphEdges, [u32; 2])>,
    columns: u32,
    marks: Vec<u32>,
    edges: Vec<u32>,
    // Each correction and clip run has four words: global y, x0, x1, value.
    extras: Vec<u32>,
    bins: Vec<Vec<u32>>,
    bytes: Meter,
    work: Meter,
    correction_cells: u32,
    clip_runs: u32,
}

fn index(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| Error::limit("GPU geometry index"))
}

impl Builder {
    fn draw(&mut self, prepared: &PreparedScene, key: Option<u32>, mark: &Mark) -> Result<()> {
        if self.shared {
            self.shared_mark(prepared, key, mark)
        } else {
            self.mark(prepared, key, mark)
        }
    }

    fn mark(&mut self, prepared: &PreparedScene, key: Option<u32>, mark: &Mark) -> Result<()> {
        let shape = key
            .and_then(|key| prepared.shapes.shapes.get(key as usize))
            .ok_or(Error::unsupported(
                Feature::new("gpu.raster"),
                "unbounded GPU mark is not implemented",
            ))?;
        let bounds = shape.bounds().intersect(&self.region);
        let mut count = 0_usize;
        shape.gpu_geometry().edges(|_, _| {
            self.work.charge(1)?;
            count += 1;
            Ok(())
        })?;
        // Large compound paths need a row index: traversing every page edge
        // at every pixel would hide the actual benefit of GPU coverage.
        if count > 128 && bounds.height() > 4 {
            for y in (bounds.y0..bounds.y1).step_by(4) {
                self.part(
                    prepared,
                    key,
                    mark,
                    DeviceRect {
                        y0: y,
                        y1: (y + 4).min(bounds.y1),
                        ..bounds
                    },
                )?;
            }
            Ok(())
        } else {
            self.part(prepared, key, mark, bounds)
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite prepared coordinates narrowed intentionally for this experiment"
    )]
    fn part(
        &mut self,
        prepared: &PreparedScene,
        shape: Option<u32>,
        mark: &Mark,
        window: DeviceRect,
    ) -> Result<()> {
        let shape = shape
            .and_then(|key| prepared.shapes.shapes.get(key as usize))
            .ok_or(Error::unsupported(
                Feature::new("gpu.raster"),
                "unbounded GPU mark is not implemented",
            ))?;
        let bounds = shape.bounds().intersect(&window);
        if bounds.is_empty() {
            return Ok(());
        }
        let geometry = shape.gpu_geometry();
        let origin = if self.local {
            [bounds.x0, bounds.y0]
        } else {
            [0, 0]
        };
        let edge_start = index(self.edges.len() / EDGE)?;
        geometry.edges(|points, curve| {
            self.work.charge(1)?;
            let bottom = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
            let top = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
            if bottom <= f64::from(bounds.y0) || top >= f64::from(bounds.y1) {
                return Ok(());
            }
            self.bytes.reserve(&mut self.edges, EDGE)?;
            self.edges.push(u32::from(curve));
            for point in points {
                for (v, o) in [(point.x, origin[0]), (point.y, origin[1])] {
                    let v = (v - f64::from(o)) as f32;
                    if !v.is_finite() {
                        return Err(Error::limit("GPU coordinate exceeds f32"));
                    }
                    self.edges.push(v.to_bits());
                }
            }
            self.edges.push(0);
            Ok(())
        })?;
        let edge_end = index(self.edges.len() / EDGE)?;
        let correction_start = index(self.extras.len() / 4)?;
        for y in bounds.y0..bounds.y1 {
            self.work.charge(1)?;
            for &(x, value) in geometry.corrections(y) {
                if x >= bounds.x0 && x < bounds.x1 {
                    self.extra([y, x, x + 1, u32::from(value)])?;
                    self.correction_cells += 1;
                }
            }
        }
        let correction_end = index(self.extras.len() / 4)?;
        let clip_start = correction_end;
        if geometry.has_clip_rows() {
            for y in bounds.y0..bounds.y1 {
                geometry.clips(y, |x0, x1, value| {
                    let (x0, x1) = (x0.max(bounds.x0), x1.min(bounds.x1));
                    if x0 < x1 {
                        self.extra([y, x0, x1, u32::from(value)])?;
                        self.clip_runs += 1;
                    }
                    Ok(())
                })?;
            }
        }
        let clip_end = index(self.extras.len() / 4)?;
        let id = index(self.marks.len() / MARK)?;
        self.bytes.reserve(&mut self.marks, MARK)?;
        let color = mark
            .words
            .ok_or(Error::invalid_argument("GPU raster missing solid color"))?;
        self.marks.extend([
            bounds.x0,
            bounds.y0,
            bounds.x1,
            bounds.y1,
            edge_start,
            edge_end,
            u32::from(geometry.has_clip_rows()),
            u32::from(geometry.rule() == FillRule::EvenOdd),
            origin[0],
            origin[1],
            u32::from(color[0]) | u32::from(color[1]) << 16,
            u32::from(color[2]) | u32::from(color[3]) << 16,
            correction_start,
            correction_end,
            clip_start,
            clip_end,
        ]);
        self.bin(bounds, id, edge_end - edge_start)
    }

    fn bin(&mut self, bounds: DeviceRect, id: u32, edges: u32) -> Result<()> {
        let x0 = (bounds.x0 - self.region.x0) / TILE_X;
        let x1 = (bounds.x1 - self.region.x0).div_ceil(TILE_X);
        let y0 = (bounds.y0 - self.region.y0) / TILE_Y;
        let y1 = (bounds.y1 - self.region.y0).div_ceil(TILE_Y);
        // Conservative pixel-edge evaluations, including partially occupied tiles.
        let visits = (x1 - x0) as usize * (y1 - y0) as usize * (TILE_X * TILE_Y) as usize;
        self.work
            .charge(visits.saturating_mul(edges as usize + 1))?;
        for y in y0..y1 {
            for x in x0..x1 {
                let bin = &mut self.bins[(y * self.columns + x) as usize];
                self.bytes.reserve(bin, 1)?;
                bin.push(id);
            }
        }
        Ok(())
    }

    fn extra(&mut self, value: [u32; 4]) -> Result<()> {
        self.work.charge(1)?;
        self.bytes.reserve(&mut self.extras, 4)?;
        self.extras.extend(value);
        Ok(())
    }

    fn finish(mut self, background: [u8; 4]) -> Result<Vec<u32>> {
        let marks_at = HEADER;
        let edges_at = marks_at + self.marks.len();
        let extras_at = edges_at + self.edges.len();
        let edge_refs_at = extras_at + self.extras.len();
        let bins_at = edge_refs_at + self.edge_refs.len();
        let refs_at = bins_at + self.bins.len() * 2;
        let references: usize = self.bins.iter().map(Vec::len).sum();
        let total = refs_at
            .checked_add(references)
            .ok_or(Error::limit("GPU geometry packet overflow"))?;
        let mut words = Vec::new();
        self.bytes.reserve(&mut words, total)?;
        words.extend([
            0x3255_5047,
            if self.shared { 3 } else { 2 },
            self.region.width(),
            self.region.height(),
            self.columns,
            index(self.bins.len())?,
            self.region.x0,
            self.region.y0,
            index(marks_at)?,
            index(edges_at)?,
            index(extras_at)?,
            index(bins_at)?,
            index(refs_at)?,
            index(self.marks.len() / if self.shared { 20 } else { MARK })?,
            index(self.edges.len() / EDGE)?,
            index(self.extras.len() / 4)?,
            index(references)?,
            index(total)?,
            u32::from_le_bytes(background),
            u32::from(self.local),
            self.correction_cells,
            self.clip_runs,
            if self.shared { index(edge_refs_at)? } else { 0 },
            index(self.edge_refs.len())?,
        ]);
        words.extend(self.marks);
        words.extend(self.edges);
        words.extend(self.extras);
        words.extend(self.edge_refs);
        let mut start = 0;
        for bin in &self.bins {
            let end = start + index(bin.len())?;
            words.extend([start, end]);
            start = end;
        }
        for bin in self.bins {
            words.extend(bin);
        }
        Ok(words)
    }
}
