//! Experimental host execution of exact N16 spans. Enabled by `gpu-demo`.
//!
//! Preparation and coverage use the CPU renderer's sources, including joint
//! clipping and true quadratic glyphs. A host runs the resulting integer
//! program on a GPU. Admission is conservative: only an RGB N16 page of
//! opaque solid Normal marks, without groups, masks, overprint or transfer.
//! Everything else keeps its CPU rendering. No precision is downgraded.
//!
//! The packet is versioned, little-endian u32 words. Its layout and matching
//! WGSL executor live in `benchmarks/gpu-demo/`. Packet construction is an
//! additional fallible preparation step; GPU execution starts only afterward.

use crate::PreparedScene;
use crate::execute::{Active, Scratch, clamp};
use crate::format::Format;
use crate::plan::{Cover, ItemKind, Mark, Run};
use pagura_color::Process;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Feature, Limits, Meter, Result};
use pagura_interpret::BlendMode;
use pagura_raster::{BandCoverage, BandRasterizer, DeviceRect};

const HEADER: usize = 16;
const STRIDE: usize = 8;
const BIN_WIDTH: u32 = 32;
const FULL: u32 = u32::MAX;

/// A completely prepared, bounded, immutable host-side N16 program.
#[derive(Debug)]
pub struct Program {
    words: Vec<u32>,
}

impl Program {
    /// Versioned packet words; serialize each word little-endian.
    pub fn words(&self) -> &[u32] {
        &self.words
    }
}

impl PreparedScene {
    /// Why the experimental GPU cannot execute this request, or `None`.
    ///
    /// This checks the prepared representation, including its precision
    /// decision. A host must use the CPU for any returned reason.
    pub fn gpu_fallback_reason(&self) -> Option<&'static str> {
        let plan = &self.plan;
        let Some(page) = plan.groups.first() else {
            return Some("no page buffer");
        };
        if page.format != Format::N16 {
            return Some("the page requires F32 or F64 precision");
        }
        if page.layout.process != Process::Rgb || page.layout.spots != 0 {
            return Some("the page requires non-RGB or spot channels");
        }
        if plan.groups.len() != 1 || page.knockout || !plan.masks.is_empty() {
            return Some("groups, patterns or soft masks require the CPU");
        }
        if !plan.output.transfers.is_empty() || !plan.output.spots.is_empty() {
            return Some("transfer functions or overprint require the CPU");
        }
        for &id in &page.items {
            let Some(item) = plan.items.get(u32_to_usize(id)) else {
                return Some("invalid prepared item");
            };
            let mark = match &item.kind {
                ItemKind::Mark(mark) => mark,
                ItemKind::Run(run) => &run.mark,
                ItemKind::Group(_) => return Some("group composition requires the CPU"),
            };
            if mark.words.is_none() {
                return Some("sampled paint or dynamic opacity requires the CPU");
            }
            let composite = &mark.compositing;
            if mark.records
                || composite.alpha != 1.0
                || composite.blend != BlendMode::Normal
                || composite.mask.is_some()
                || composite.overprint != 0
            {
                return Some("the mark requires CPU compositing");
            }
        }
        None
    }

    /// Prepares the exact coverage and ordered N16 spans for a host GPU.
    ///
    /// `limits.render_bytes` separately bounds all temporary and retained
    /// packet storage, and `limits.render_work` bounds bin references. The
    /// original preparation's charges are unchanged. The host must also
    /// bound GPU buffers and dispatch dimensions against its device limits.
    ///
    /// # Errors
    ///
    /// `Unsupported(gpu.demo)` if admission refuses the page; `Limit` when
    /// scratch or packet storage cannot be reserved or indexed by u32.
    pub fn gpu_program(&self, scratch: &mut Scratch, limits: &Limits) -> Result<Program> {
        if let Some(reason) = self.gpu_fallback_reason() {
            return Err(Error::unsupported(Feature::new("gpu.demo"), reason));
        }
        let mut builder = Builder::new(self.region(), limits);
        let (workspace, rasterizer) = scratch.fit(&self.plan)?;
        let mut coverage = std::mem::take(&mut workspace.coverage);
        let mut run_shapes = std::mem::take(&mut workspace.run_shapes);
        let active = workspace
            .active
            .first_mut()
            .ok_or(Error::invalid_argument("no page schedule"))?;
        active.reset();
        let result = self.collect(
            &mut builder,
            rasterizer,
            &mut coverage,
            &mut run_shapes,
            active,
        );
        workspace.coverage = coverage;
        workspace.run_shapes = run_shapes;
        result?;
        builder.finish(self.background())
    }

    fn collect(
        &self,
        builder: &mut Builder,
        rasterizer: &mut BandRasterizer,
        coverage: &mut BandCoverage,
        run_shapes: &mut Vec<u32>,
        active: &mut Active,
    ) -> Result<()> {
        let plan = &self.plan;
        let Some(page) = plan.groups.first() else {
            return Err(Error::invalid_argument("no prepared page"));
        };
        for band in plan.area.bands() {
            let rows = plan.area.band_rows(band);
            let rect = DeviceRect {
                y0: rows.start,
                y1: rows.end,
                ..plan.area.region
            };
            active.advance(page, &plan.items, band);
            let mut next = 0;
            while let Some(position) = active.get(next) {
                next += 1;
                let Some(item) = page
                    .items
                    .get(u32_to_usize(position))
                    .and_then(|&id| plan.items.get(u32_to_usize(id)))
                else {
                    return Err(Error::invalid_argument("invalid prepared item"));
                };
                match &item.kind {
                    ItemKind::Mark(mark) => {
                        self.shapes
                            .rasterize(rasterizer, mark.cover, band, coverage);
                        builder.coverage(coverage, rect, mark)?;
                    }
                    ItemKind::Run(run) => {
                        self.collect_run(
                            builder, rasterizer, coverage, run_shapes, run, rect, band,
                        )?;
                    }
                    ItemKind::Group(_) => {
                        return Err(Error::invalid_argument("GPU admission missed a group"));
                    }
                }
            }
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "borrowed scratch and one band's glyph run"
    )]
    fn collect_run(
        &self,
        builder: &mut Builder,
        rasterizer: &mut BandRasterizer,
        coverage: &mut BandCoverage,
        alone: &mut Vec<u32>,
        run: &Run,
        rect: DeviceRect,
        band: u32,
    ) -> Result<()> {
        // Same non-overlapping run batching as execute/glyphs.rs. Overlapping
        // placements remain separate stores in their original order.
        let meets = |glyph: &crate::plan::Glyph| {
            glyph.bands[0] <= band
                && band <= glyph.bands[1]
                && !glyph.bounds.intersect(&rect).is_empty()
        };
        alone.clear();
        alone.extend(
            run.glyphs
                .iter()
                .filter(|g| g.alone && meets(g))
                .map(|g| g.shape),
        );
        let mut next = 0;
        while let Some(rest) = alone.get(next..).filter(|rest| !rest.is_empty()) {
            let taken =
                self.shapes
                    .rasterize_run(rasterizer, rest, run.mark.cover.clip, band, coverage);
            next += taken.max(1);
            builder.coverage(coverage, rect, &run.mark)?;
        }
        for glyph in run.glyphs.iter().filter(|g| !g.alone && meets(g)) {
            self.shapes.rasterize(
                rasterizer,
                Cover {
                    shape: Some(glyph.shape),
                    clip: run.mark.cover.clip,
                },
                band,
                coverage,
            );
            builder.coverage(coverage, rect, &run.mark)?;
        }
        Ok(())
    }
}

/// Temporary row spans and packed coverage before ordered binning.
struct Builder {
    region: DeviceRect,
    spans: Vec<[u32; STRIDE]>,
    cells: Vec<u32>,
    bytes: Meter,
    work: Meter,
}

impl Builder {
    fn new(region: DeviceRect, limits: &Limits) -> Self {
        Self {
            region,
            spans: Vec::new(),
            cells: Vec::new(),
            bytes: Meter::new(limits.render_bytes, "GPU packet memory budget exceeded"),
            work: Meter::new(limits.render_work, "GPU bin work budget exceeded"),
        }
    }

    fn coverage(&mut self, coverage: &BandCoverage, rect: DeviceRect, mark: &Mark) -> Result<()> {
        let words = mark
            .words
            .ok_or(Error::invalid_argument("GPU mark has no N16 words"))?;
        for row in coverage
            .rows()
            .filter(|row| row.y >= rect.y0 && row.y < rect.y1)
        {
            for span in row.spans() {
                let Some((x, cover)) = clamp(span, rect) else {
                    continue;
                };
                let offset = if let Some(cells) = cover.coverage() {
                    let offset = index(
                        self.cells
                            .len()
                            .checked_mul(4)
                            .ok_or(Error::limit("GPU coverage overflow"))?,
                    )?;
                    reserve(&mut self.bytes, &mut self.cells, cells.len().div_ceil(4))?;
                    for chunk in cells.chunks(4) {
                        let mut bytes = [0; 4];
                        bytes[..chunk.len()].copy_from_slice(chunk);
                        self.cells.push(u32::from_le_bytes(bytes));
                    }
                    offset
                } else {
                    FULL
                };
                reserve(&mut self.bytes, &mut self.spans, 1)?;
                self.spans.push([
                    row.y - self.region.y0,
                    x - self.region.x0,
                    index(cover.len())?,
                    offset,
                    pack(words[0], words[1]),
                    pack(words[2], words[3]),
                    0,
                    0,
                ]);
            }
        }
        Ok(())
    }

    fn finish(mut self, background: [u8; 4]) -> Result<Program> {
        let width = self.region.width();
        let columns = width.div_ceil(BIN_WIDTH);
        let bins = u32_to_usize(columns)
            .checked_mul(u32_to_usize(self.region.height()))
            .ok_or(Error::limit("GPU bin count overflow"))?;
        let mut counts = Vec::<u32>::new();
        self.bytes.reserve(&mut counts, bins)?;
        counts.resize(bins, 0);
        for span in &self.spans {
            let begin = u32_to_usize(span[0]) * u32_to_usize(columns);
            for bin in span[1] / BIN_WIDTH..=(span[1] + span[2] - 1) / BIN_WIDTH {
                self.work.charge(1)?;
                let count = counts
                    .get_mut(begin + u32_to_usize(bin))
                    .ok_or(Error::invalid_argument("GPU span outside region"))?;
                *count = count
                    .checked_add(1)
                    .ok_or(Error::limit("GPU bin overflow"))?;
            }
        }
        let references = counts.iter().try_fold(0_usize, |sum, &count| {
            sum.checked_add(u32_to_usize(count))
                .ok_or(Error::limit("GPU references overflow"))
        })?;
        let spans_at = HEADER;
        let cells_at = spans_at
            .checked_add(
                self.spans
                    .len()
                    .checked_mul(STRIDE)
                    .ok_or(Error::limit("GPU spans overflow"))?,
            )
            .ok_or(Error::limit("GPU packet overflow"))?;
        let bins_at = cells_at
            .checked_add(self.cells.len())
            .ok_or(Error::limit("GPU packet overflow"))?;
        let refs_at = bins_at
            .checked_add(
                bins.checked_mul(2)
                    .ok_or(Error::limit("GPU bins overflow"))?,
            )
            .ok_or(Error::limit("GPU packet overflow"))?;
        let length = refs_at
            .checked_add(references)
            .ok_or(Error::limit("GPU packet overflow"))?;
        let mut words = Vec::new();
        self.bytes.reserve(&mut words, length)?;
        words.resize(length, 0);
        let bg = background.map(|b| u16::from(b) * 257);
        words[..HEADER].copy_from_slice(&[
            0x3155_5047,
            1,
            width,
            self.region.height(),
            columns,
            pack(bg[0], bg[1]),
            pack(bg[2], bg[3]),
            index(spans_at)?,
            index(cells_at)?,
            index(bins_at)?,
            index(refs_at)?,
            index(self.spans.len())?,
            index(self.cells.len())?,
            index(bins)?,
            index(references)?,
            index(length)?,
        ]);
        for (out, span) in words[spans_at..cells_at]
            .as_chunks_mut::<STRIDE>()
            .0
            .iter_mut()
            .zip(&self.spans)
        {
            out.copy_from_slice(span);
        }
        words[cells_at..bins_at].copy_from_slice(&self.cells);
        let mut offset = 0;
        for (i, count) in counts.iter_mut().enumerate() {
            let end = offset + *count;
            words[bins_at + i * 2] = offset;
            words[bins_at + i * 2 + 1] = end;
            *count = offset;
            offset = end;
        }
        for (i, span) in self.spans.iter().enumerate() {
            let begin = u32_to_usize(span[0]) * u32_to_usize(columns);
            for bin in span[1] / BIN_WIDTH..=(span[1] + span[2] - 1) / BIN_WIDTH {
                let cursor = &mut counts[begin + u32_to_usize(bin)];
                words[refs_at + u32_to_usize(*cursor)] = index(i)?;
                *cursor += 1;
            }
        }
        Ok(Program { words })
    }
}

fn pack(lo: u16, hi: u16) -> u32 {
    u32::from(lo) | (u32::from(hi) << 16)
}

fn index(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error::limit("GPU packet exceeds u32"))
}

/// Grow in fixed chunks so one span never causes a reallocating append.
fn reserve<T>(meter: &mut Meter, values: &mut Vec<T>, additional: usize) -> Result<()> {
    let available = values.capacity().saturating_sub(values.len());
    if available < additional {
        meter.reserve(values, additional.max(256))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
