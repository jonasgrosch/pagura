//! Fixed vectorization, independent of later view resolution. Experimental:
//! grid-specific winding corrections are counted, not scaled as pixel masks.
use crate::{
    PreparedScene,
    plan::{ItemKind, Mark},
};
use pagura_core::{Error, Feature, Limits, Meter, Result};
use pagura_raster::{DeviceRect, FillRule};
mod geometry;
mod patches;
const STRIDE: usize = 24;
const STEP: f64 = 8.0;
const PAD: f64 = 2.25; // More than half a pixel at 72 DPI in the 300-DPI basis.

impl PreparedScene {
    /// Exports one fixed vector packet for views from 72 through 300 DPI.
    /// The caller prepares the canonical scene at 300 DPI. Unlike Exact,
    /// this experiment integrates source winding without device-cell patches.
    /// Non-rectangular clips are refused. Strokes must be scale independent;
    /// the host separately rejects hairlines and automatic stroke adjustment.
    ///
    /// # Errors
    /// Unsupported semantics and bounded allocation/work failures.
    pub fn gpu_world_program(&self, limits: &Limits) -> Result<Vec<u32>> {
        if let Some(reason) = self.gpu_fallback_reason() {
            return Err(Error::unsupported(Feature::new("gpu.world"), reason));
        }
        if self.region().x0 != 0 || self.region().y0 != 0 || self.region().is_empty() {
            return Err(Error::unsupported(
                Feature::new("gpu.world"),
                "world vectors require an origin-zero whole-page request",
            ));
        }
        if self.background()[3] != 255 {
            return Err(Error::unsupported(
                Feature::new("gpu.world"),
                "world vectors require an opaque backdrop",
            ));
        }
        let mut b = Builder::new(self.region(), limits)?;
        for &id in &self.plan.groups[0].items {
            match &self.plan.items[id as usize].kind {
                ItemKind::Mark(mark) => b.mark(
                    self,
                    mark.cover.shape,
                    mark,
                    self.plan.items[id as usize].serial,
                )?,
                ItemKind::Run(run) => {
                    for glyph in &run.glyphs {
                        b.mark(self, Some(glyph.shape), &run.mark, glyph.serial)?;
                    }
                }
                ItemKind::Group(_) => {
                    return Err(Error::invalid_argument("world admission missed group"));
                }
            }
        }
        b.charge_shader()?;
        b.finish(self.background())
    }
}

struct Builder {
    region: DeviceRect,
    columns: u32,
    marks: Vec<u32>,
    edges: Vec<u32>,
    rows: Vec<u32>,
    row_refs: Vec<u32>,
    bins: Vec<Vec<u32>>,
    sources: Vec<(*const pagura_raster::GlyphEdges, Source)>,
    source_count: u32,
    omitted_cells: u32,
    bytes: Meter,
    work: Meter,
}
#[derive(Clone, Copy)]
struct Source {
    row: u32,
    count: u32,
    top: f64,
    edges: u32,
    id: u32,
}
fn index(n: usize) -> Result<u32> {
    u32::try_from(n).map_err(|_| Error::limit("world index exceeds u32"))
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "intentional finite f32 experiment"
)]
fn float(n: f64) -> Result<u32> {
    let v = n as f32;
    if v.is_finite() && n.abs() <= 16_777_216.0 {
        Ok(v.to_bits())
    } else {
        Err(Error::limit("world coordinate exceeds f32"))
    }
}
impl Builder {
    fn new(region: DeviceRect, limits: &Limits) -> Result<Self> {
        let mut b = Self {
            region,
            columns: region.width().div_ceil(32),
            marks: Vec::new(),
            edges: Vec::new(),
            rows: Vec::new(),
            row_refs: Vec::new(),
            bins: Vec::new(),
            sources: Vec::new(),
            source_count: 0,
            omitted_cells: 0,
            bytes: Meter::new(limits.render_bytes, "world byte budget"),
            work: Meter::new(limits.render_work, "world work budget"),
        };
        b.work.charge(
            (region.width() as usize)
                .checked_mul(region.height() as usize)
                .ok_or(Error::limit("world pixels"))?,
        )?;
        let count = (b.columns as usize)
            .checked_mul(region.height().div_ceil(8) as usize)
            .ok_or(Error::limit("world bins"))?;
        b.bytes.reserve(&mut b.bins, count)?;
        b.bins.resize_with(count, Vec::new);
        Ok(b)
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one bounded mark and its immutable source"
    )]
    fn mark(
        &mut self,
        p: &PreparedScene,
        key: Option<u32>,
        mark: &Mark,
        serial: u32,
    ) -> Result<()> {
        let shape = key
            .and_then(|k| p.shapes.shapes.get(k as usize))
            .ok_or(Error::unsupported(
                Feature::new("gpu.world"),
                "unbounded world mark",
            ))?;
        let bounds = shape.bounds().intersect(&self.region);
        if bounds.is_empty() {
            return Ok(());
        }
        let geometry = shape.gpu_geometry();
        if geometry.device_points() {
            return Err(Error::unsupported(
                Feature::new("gpu.world"),
                "glyph point cells are resolution dependent",
            ));
        }
        if geometry.has_clip_rows() {
            return Err(Error::unsupported(
                Feature::new("gpu.world"),
                "non-rectangular world clip",
            ));
        }
        let origin = [bounds.x0, bounds.y0];
        let (source, offset) = if let Some((glyph, placement)) = geometry.shared() {
            let key = std::ptr::from_ref(glyph);
            let source = match self.sources.binary_search_by_key(&key, |&(key, _)| key) {
                Ok(i) => self.sources[i].1,
                Err(i) => {
                    let start = self.edges.len() / 8;
                    glyph.gpu_edges(|points, curve| self.edge(points, curve, [0, 0]))?;
                    let s = self.source(start)?;
                    self.bytes.reserve(&mut self.sources, 1)?;
                    self.sources.insert(i, (key, s));
                    s
                }
            };
            (
                source,
                [
                    placement.x - f64::from(origin[0]),
                    placement.y - f64::from(origin[1]),
                ],
            )
        } else {
            let start = self.edges.len() / 8;
            geometry.edges(|points, curve| self.edge(points, curve, origin))?;
            (self.source(start)?, [0.0, 0.0])
        };
        for y in bounds.y0..bounds.y1 {
            self.work.charge(1)?;
            self.omitted_cells = self
                .omitted_cells
                .checked_add(index(geometry.correction_count(y))?)
                .ok_or(Error::limit("world correction count"))?;
        }
        let clip = geometry.rectangle_clip();
        let relative = clip.map_or([0.0; 4], |r| {
            [
                r[0] - f64::from(origin[0]) - offset[0],
                r[1] - f64::from(origin[1]) - offset[1],
                r[2] - f64::from(origin[0]) - offset[0],
                r[3] - f64::from(origin[1]) - offset[1],
            ]
        });
        let color = mark
            .words
            .ok_or(Error::invalid_argument("world solid colour"))?;
        let id = index(self.marks.len() / STRIDE)?;
        self.bytes.reserve(&mut self.marks, STRIDE)?;
        self.marks.extend([
            bounds.x0,
            bounds.y0,
            bounds.x1,
            bounds.y1,
            source.row,
            source.count,
            u32::from(clip.is_some()),
            u32::from(geometry.rule() == FillRule::EvenOdd),
            origin[0],
            origin[1],
            u32::from(color[0]) | u32::from(color[1]) << 16,
            u32::from(color[2]) | u32::from(color[3]) << 16,
            float(source.top)?,
            source.edges,
            source.id,
            0,
            float(offset[0])?,
            float(offset[1])?,
            serial,
            u32::from(geometry.shared().is_some()),
            float(relative[0])?,
            float(relative[1])?,
            float(relative[2])?,
            float(relative[3])?,
        ]);
        let x0 = bounds.x0.saturating_sub(3) / 32;
        let x1 = bounds.x1.saturating_add(3).min(self.region.x1).div_ceil(32);
        let y0 = bounds.y0.saturating_sub(3) / 8;
        let y1 = bounds.y1.saturating_add(3).min(self.region.y1).div_ceil(8);
        for y in y0..y1 {
            for x in x0..x1 {
                self.work.charge(1)?;
                let bin = &mut self.bins[(y * self.columns + x) as usize];
                self.bytes.reserve(bin, 1)?;
                bin.push(id);
            }
        }
        Ok(())
    }
    fn finish(mut self, background: [u8; 4]) -> Result<Vec<u32>> {
        let ma = 24;
        let ea = ma + self.marks.len();
        let ra = ea + self.edges.len();
        let rra = ra + self.rows.len();
        let ba = rra + self.row_refs.len();
        let bpa = ba + self.bins.len() * 2;
        let refs: usize = self.bins.iter().map(Vec::len).sum();
        let total = bpa.checked_add(refs).ok_or(Error::limit("world packet"))?;
        let mut out = Vec::new();
        self.bytes.reserve(&mut out, total)?;
        out.extend([
            0x3455_5047,
            1,
            self.region.width(),
            self.region.height(),
            self.columns,
            index(self.bins.len())?,
            72,
            300,
            index(ma)?,
            index(ea)?,
            index(ra)?,
            index(ba)?,
            index(bpa)?,
            index(self.marks.len() / STRIDE)?,
            index(self.edges.len() / 8)?,
            index(self.rows.len() / 2)?,
            index(refs)?,
            index(total)?,
            u32::from_le_bytes(background),
            8,
            index(rra)?,
            index(self.row_refs.len())?,
            self.source_count,
            self.omitted_cells,
        ]);
        out.extend(self.marks);
        out.extend(self.edges);
        out.extend(self.rows);
        out.extend(self.row_refs);
        let mut start = 0;
        for bin in &self.bins {
            let end = start + index(bin.len())?;
            out.extend([start, end]);
            start = end;
        }
        for bin in self.bins {
            out.extend(bin);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
