//! Device-cell exceptions stay with each view, never in scaled vector data.
use super::{ItemKind, PreparedScene, index};
use pagura_core::{Error, Feature, Limits, Meter, Result};
use pagura_raster::Shape;

impl PreparedScene {
    /// Exports sparse Exact winding/clip corrections and source identities for
    /// one GPU view. No ordinary coverage rows or edge payloads are produced.
    ///
    /// # Errors
    /// Unsupported semantics or bounded allocation/work errors.
    pub fn gpu_world_patches(&self, limits: &Limits) -> Result<Vec<u32>> {
        if let Some(reason) = self.gpu_fallback_reason() {
            return Err(Error::unsupported(Feature::new("gpu.world"), reason));
        }
        let mut b = Builder {
            marks: Vec::new(),
            cells: Vec::new(),
            bytes: Meter::new(limits.render_bytes, "world view byte budget"),
            work: Meter::new(limits.render_work, "world view work budget"),
        };
        for &id in &self.plan.groups[0].items {
            let item = &self.plan.items[id as usize];
            match &item.kind {
                ItemKind::Mark(mark) => b.mark(
                    item.serial,
                    mark.cover
                        .shape
                        .and_then(|s| self.shapes.shapes.get(s as usize)),
                    mark.words,
                )?,
                ItemKind::Run(run) => {
                    for glyph in &run.glyphs {
                        b.mark(
                            glyph.serial,
                            self.shapes.shapes.get(glyph.shape as usize),
                            run.mark.words,
                        )?;
                    }
                }
                ItemKind::Group(_) => {
                    return Err(Error::invalid_argument("world view missed group"));
                }
            }
        }
        let cells_at = 8 + b.marks.len();
        let total = cells_at
            .checked_add(b.cells.len())
            .ok_or(Error::limit("world view size"))?;
        let mut out = Vec::new();
        b.bytes.reserve(&mut out, total)?;
        out.extend([
            0x3150_5747,
            1,
            self.region().width(),
            self.region().height(),
            index(b.marks.len() / 16)?,
            index(b.cells.len() / 4)?,
            8,
            index(cells_at)?,
        ]);
        out.extend(b.marks);
        out.extend(b.cells);
        Ok(out)
    }
}
struct Builder {
    marks: Vec<u32>,
    cells: Vec<u32>,
    bytes: Meter,
    work: Meter,
}
impl Builder {
    fn mark(&mut self, serial: u32, shape: Option<&Shape>, color: Option<[u16; 4]>) -> Result<()> {
        let shape = shape.ok_or(Error::unsupported(
            Feature::new("gpu.world"),
            "unbounded world view",
        ))?;
        let color = color.ok_or(Error::invalid_argument("world view solid colour"))?;
        let geometry = shape.gpu_geometry();
        if geometry.has_clip_rows() {
            return Err(Error::unsupported(
                Feature::new("gpu.world"),
                "non-rectangular world view clip",
            ));
        }
        let bounds = shape.bounds();
        let start = index(self.cells.len() / 4)?;
        for y in bounds.y0..bounds.y1 {
            self.work.charge(1)?;
            for &(x, value) in geometry.corrections(y) {
                self.work.charge(1)?;
                self.bytes.reserve(&mut self.cells, 4)?;
                self.cells.extend([y, x, x + 1, u32::from(value)]);
            }
        }
        let placement = geometry.shared().map(|(_, p)| p);
        let x = placement.map_or(0.0, |p| p.x).to_bits().to_le_bytes();
        let y = placement.map_or(0.0, |p| p.y).to_bits().to_le_bytes();
        let word = |b: [u8; 8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        self.bytes.reserve(&mut self.marks, 16)?;
        self.marks.extend([
            serial,
            bounds.x0,
            bounds.y0,
            bounds.x1,
            bounds.y1,
            u32::from(color[0]) | u32::from(color[1]) << 16,
            u32::from(color[2]) | u32::from(color[3]) << 16,
            u32::from(placement.is_some()),
            word(x, 0),
            word(x, 4),
            word(y, 0),
            word(y, 4),
            start,
            index(self.cells.len() / 4)?,
            0,
            0,
        ]);
        Ok(())
    }
}
