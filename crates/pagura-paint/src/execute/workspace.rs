//! Every buffer execution writes: the page's band buffer, the stack of
//! group buffers, soft-mask values, coverage, span scratch and the band
//! rasterizer's rows, sized for one prepared scene and owned by the caller,
//! who reuses them render after render.

use super::{Active, Tops};
use crate::format::Format;
use crate::plan::{Boundary, ItemKind, Layout, Plan};
use crate::source::{PaintScratch, Source, SpanColors};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Meter, Result};
use pagura_raster::{BandCoverage, BandRasterizer, DeviceRect};

/// The storage renders write: band buffers, coverage, span scratch and the
/// band rasterizer's rows. A caller keeps one for each thread it renders on
/// and lends it to every render, which takes the storage it finds sized
/// alike for the scene and makes the rest; preparation has charged what a
/// render of the scene needs. Between renders it holds what the last one
/// used, [`Self::retained_bytes`], for the caller to keep or drop.
#[derive(Debug, Default)]
pub struct Scratch {
    workspace: Option<Workspace>,
    rasterizer: Option<BandRasterizer>,
}

impl Scratch {
    /// Bytes held between renders.
    pub fn retained_bytes(&self) -> usize {
        self.workspace.as_ref().map_or(0, Workspace::bytes)
            + self
                .rasterizer
                .as_ref()
                .map_or(0, BandRasterizer::retained_bytes)
    }

    /// The workspace and rasterizer for `plan`, allocated within the
    /// bytes preparation reserved for them ([`Plan::scratch`]).
    pub(crate) fn fit(&mut self, plan: &Plan) -> Result<(&mut Workspace, &mut BandRasterizer)> {
        // Drop storage outside the new reservation before replacing anything.
        let previous = self
            .workspace
            .take()
            .map(|workspace| Kept::new(plan, workspace));
        let mut bytes = Meter::new(plan.scratch, "render memory budget exceeded");
        let rasterizer = match self.rasterizer.take() {
            Some(rasterizer) if rasterizer.area() == plan.area => {
                BandRasterizer::charge(plan.area, &mut bytes)?;
                rasterizer
            }
            other => {
                // Its storage goes before the new one is made.
                drop(other);
                BandRasterizer::new(plan.area, &mut bytes)?
            }
        };
        let workspace = Workspace::new(plan, plan.sizes, previous, &mut bytes)?;
        let workspace = self.workspace.insert(workspace);
        let rasterizer = self.rasterizer.insert(rasterizer);
        Ok((workspace, rasterizer))
    }
}

/// The page group's band buffer in its storage format.
#[derive(Debug)]
pub(crate) enum PageBuffer {
    /// f64 pixels of the page's layout.
    F64(Vec<f64>),
    /// Premultiplied RGBA words with the background embedded.
    N16(Vec<[u16; 4]>),
    /// Premultiplied RGBA levels with the background embedded.
    F32(Vec<[f32; 4]>),
}

impl PageBuffer {
    /// Bytes per pixel of `format` with `layout`.
    pub(crate) fn pixel_bytes(format: Format, layout: Layout) -> usize {
        match format {
            Format::F64 => layout.stride() * size_of::<f64>(),
            Format::N16 => size_of::<[u16; 4]>(),
            Format::F32 => size_of::<[f32; 4]>(),
        }
    }

    /// A buffer of `pixels` in `format`, charged to `bytes`: `previous`
    /// was already checked and emptied by [`Self::keep`].
    fn new(
        (format, layout): (Format, Layout),
        pixels: usize,
        previous: Option<Self>,
        bytes: &mut Meter,
    ) -> Result<Self> {
        bytes.charge_product(pixels, Self::pixel_bytes(format, layout))?;
        if let Some(page) = previous {
            return Ok(page);
        }
        Ok(match format {
            Format::F64 => Self::F64(allocate(pixels.saturating_mul(layout.stride()))?),
            Format::N16 => Self::N16(allocate(pixels)?),
            Format::F32 => Self::F32(allocate(pixels)?),
        })
    }

    /// Keeps exactly the storage the next page reserves, emptied so its
    /// first band initializes every pixel with the new background.
    fn keep(mut self, (format, layout): (Format, Layout), pixels: usize) -> Option<Self> {
        let fits = match (&mut self, format) {
            (Self::F64(values), Format::F64) => {
                recycle(values, pixels.saturating_mul(layout.stride()))
            }
            (Self::N16(values), Format::N16) => recycle(values, pixels),
            (Self::F32(values), Format::F32) => recycle(values, pixels),
            _ => false,
        };
        fits.then_some(self)
    }

    /// Starts a band of `count` pixels: transparent, or for narrow storage
    /// the premultiplied background. The first band of a render fills them;
    /// later bands find them restored by output ([`Self::restore`]).
    pub(crate) fn reset(&mut self, count: usize, background: [u8; 4], layout: Layout) {
        let words = background.map(|b| u16::from(b) * 257);
        let levels = background.map(f32::from);
        let stride = layout.stride();
        match self {
            Self::F64(values) => grow(values, count * stride, 0.0),
            Self::N16(pixels) => grow(pixels, count, words),
            Self::F32(pixels) => grow(pixels, count, levels),
        }
    }

    /// Restores pixels `range` to what [`Self::reset`] gives them. Output
    /// does so right after reading them, while they are still in cache.
    pub(crate) fn restore(
        &mut self,
        range: std::ops::Range<usize>,
        background: [u8; 4],
        layout: Layout,
    ) {
        let stride = layout.stride();
        match self {
            Self::F64(values) => fill(values, range.start * stride..range.end * stride, 0.0),
            Self::N16(pixels) => fill(pixels, range, background.map(|b| u16::from(b) * 257)),
            Self::F32(pixels) => fill(pixels, range, background.map(f32::from)),
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::F64(values) => values.capacity() * size_of::<f64>(),
            Self::N16(words) => words.capacity() * size_of::<[u16; 4]>(),
            Self::F32(levels) => levels.capacity() * size_of::<[f32; 4]>(),
        }
    }
}

fn fill<T: Copy>(values: &mut [T], range: std::ops::Range<usize>, value: T) {
    if let Some(values) = values.get_mut(range) {
        values.fill(value);
    }
}

/// Extends `values` to `len` with `value`; preparation reserved the room.
fn grow<T: Copy>(values: &mut Vec<T>, len: usize, value: T) {
    if values.len() < len {
        values.resize(len, value);
    }
}

/// An empty extent.
const EMPTY: [u32; 2] = [u32::MAX, 0];

/// The columns of each row of the page's band that stores reached: the
/// rest still holds the background, so neither restoring nor output reads
/// it (write-once output for untouched pixels).
#[derive(Debug, Default)]
pub(crate) struct Extents {
    rows: Vec<[u32; 2]>,
}

impl Extents {
    pub(super) fn new(rows: usize, bytes: &mut Meter) -> Result<Self> {
        let mut values = reserve(rows, bytes)?;
        values.resize(rows, EMPTY);
        Ok(Self { rows: values })
    }

    /// Records a store at columns `start .. end` of `row`, relative to the
    /// region.
    pub(crate) fn mark(&mut self, row: usize, start: u32, end: u32) {
        if let Some(extent) = self.rows.get_mut(row) {
            *extent = [extent[0].min(start), extent[1].max(end)];
        }
    }

    /// The painted columns of `row`, if any, forgotten for the next band.
    pub(crate) fn take(&mut self, row: usize) -> Option<(usize, usize)> {
        let [lo, hi] = std::mem::replace(self.rows.get_mut(row)?, EMPTY);
        (lo < hi).then(|| (u32_to_usize(lo), u32_to_usize(hi)))
    }
}

/// One soft mask's values for the current band.
#[derive(Debug, Default)]
pub(crate) struct MaskCache {
    /// Values over `rect`, row by row.
    pub(crate) values: Vec<f64>,
    /// The pixels the values cover.
    pub(crate) rect: DeviceRect,
    /// The band they were computed for.
    pub(crate) band: Option<u32>,
}

/// Work and function evaluations of the last render, to check that
/// preparation reserved at least as much.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    /// Rasterization work and composited pixels.
    pub(crate) work: usize,
    /// Function evaluation units spent by the executor itself.
    pub(crate) functions: usize,
}

/// The storage of execution.
#[derive(Debug)]
pub(crate) struct Workspace {
    /// The page's band buffer.
    pub(crate) page: PageBuffer,
    /// The columns of the page's band that stores reached.
    pub(crate) extents: Extents,
    /// F64 group buffers alive at once.
    pub(crate) wide: Vec<f64>,
    /// F32 group buffers alive at once.
    pub(crate) levels: Vec<[f32; 4]>,
    /// Soft-mask values by mask.
    pub(crate) masks: Vec<MaskCache>,
    /// One mark's coverage.
    pub(crate) coverage: BandCoverage,
    /// The coverage of a shading pattern's `BBox` outline, and one span of
    /// it as bytes.
    pub(crate) boundary: (BandCoverage, Vec<u8>),
    /// One span of paint.
    pub(crate) colors: SpanColors,
    /// Per-paint execution buffers with capacities fitted before rendering.
    pub(crate) paint_scratch: Vec<PaintScratch>,
    /// Source alpha of a span.
    pub(crate) alpha: Vec<f64>,
    /// Source shape of a span.
    pub(crate) shape: Vec<f64>,
    /// The paint's own shape of a span, where it is not its alpha.
    pub(crate) texel_shape: Vec<f64>,
    /// Soft-mask values of a span.
    pub(crate) mask: Vec<f64>,
    /// Group-result colours of a span in the parent's channels.
    pub(crate) source: Vec<f64>,
    /// Backdrop pixels of a span.
    pub(crate) initial: Vec<f64>,
    /// Transfer selections of the band (11.7.5.2); empty without them.
    pub(crate) selection: Vec<u16>,
    /// The shapes of a glyph run's placements rasterized together.
    pub(crate) run_shapes: Vec<u32>,
    /// Active items per group.
    pub(crate) active: Vec<Active>,
    /// What the last render used.
    pub(crate) usage: Usage,
    /// The rows and cells `coverage` was sized for.
    capacity: (usize, usize),
}

/// The sizes of a workspace, from which preparation charges it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Sizes {
    /// Region width.
    pub(crate) width: usize,
    /// Band rows.
    pub(crate) rows: usize,
    /// The most channels of any buffer.
    pub(crate) channels: usize,
    /// The most f64 values of any pixel.
    pub(crate) stride: usize,
    /// Whether transfer selections are tracked.
    pub(crate) selections: bool,
}

impl Sizes {
    /// The bytes [`Workspace::new`] allocates for `plan` at most, were its
    /// group buffers `stack`.
    pub(crate) fn bytes(&self, plan: &Plan, stack: Tops) -> usize {
        let pixels = self.width.saturating_mul(self.rows);
        let page = plan.groups.first().map_or(0, |page| {
            PageBuffer::pixel_bytes(page.format, page.layout).saturating_mul(pixels)
        });
        let masks: usize = plan
            .masks
            .iter()
            .map(|mask| mask_values(mask.extent, self.rows))
            .fold(0, usize::saturating_add);
        let spans = self
            .width
            .saturating_mul(self.channels.saturating_mul(3).saturating_add(5))
            .saturating_add(self.width.saturating_mul(self.stride));
        // Growth by doubling may leave up to twice the spans and cells a band
        // of one-pixel spans needs; a mark and a pattern outline.
        let coverage = pixels
            .saturating_mul(size_of::<u8>() + 12)
            .saturating_add(self.rows * 12)
            .saturating_mul(4)
            .saturating_add(self.width)
            .saturating_add(self.rows * size_of::<[u32; 2]>());
        let active: usize = plan
            .groups
            .iter()
            .map(|group| group.items.len())
            .fold(0, usize::saturating_add);
        let gathered = run_shapes(plan).saturating_mul(size_of::<u32>());
        let f64s = stack.wide.saturating_add(masks).saturating_add(spans);
        f64s.saturating_mul(size_of::<f64>())
            .saturating_add(stack.levels.saturating_mul(size_of::<[f32; 4]>()))
            .saturating_add(page)
            .saturating_add(coverage)
            .saturating_add(gathered)
            .saturating_add(if self.selections { pixels * 2 } else { 0 })
            .saturating_add(active.saturating_mul(size_of::<u32>()))
            .saturating_add(plan.groups.len() * size_of::<Active>())
            .saturating_add(plan.masks.len() * size_of::<MaskCache>())
            .saturating_add(plan.paints.len().saturating_mul(size_of::<PaintScratch>()))
            .saturating_add(
                plan.paints
                    .iter()
                    .map(Source::scratch_bytes)
                    .fold(0, usize::saturating_add),
            )
    }
}

/// The most placements of one glyph run.
fn run_shapes(plan: &Plan) -> usize {
    plan.items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Run(run) => Some(run.glyphs.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// f64 values a soft mask's cache holds.
pub(crate) fn mask_values(extent: DeviceRect, rows: usize) -> usize {
    u32_to_usize(extent.width()).saturating_mul(u32_to_usize(extent.height()).min(rows))
}

/// Empties `values` for reuse when it was sized for `count`: exactly, so
/// that what a workspace holds stays what preparation reserved.
fn recycle<T>(values: &mut Vec<T>, count: usize) -> bool {
    values.clear();
    values.capacity() == count
}

/// Room for `count` values, charged to `bytes`.
fn reserve<T>(count: usize, bytes: &mut Meter) -> Result<Vec<T>> {
    let mut values = Vec::new();
    bytes.reserve(&mut values, count)?;
    Ok(values)
}

/// Room for `count` values, charged by the caller.
fn allocate<T>(count: usize) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("render buffer allocation failed"))?;
    Ok(values)
}

/// Fits paint storage after [`keep_paints`] dropped owners outside the
/// new reservation, charging matching reuse and new storage alike.
fn paint_storage(
    sources: &[Source],
    previous: Option<Vec<PaintScratch>>,
    bytes: &mut Meter,
) -> Result<Vec<PaintScratch>> {
    bytes.charge_items::<PaintScratch>(sources.len())?;
    let mut storage = match previous {
        Some(previous) => previous,
        None => allocate(sources.len())?,
    };
    storage.resize_with(sources.len(), PaintScratch::default);
    for (scratch, source) in storage.iter_mut().zip(sources) {
        *scratch = source.make_scratch(Some(std::mem::take(scratch)), bytes)?;
    }
    Ok(storage)
}

impl Workspace {
    /// Allocates the storage `plan` executes in, charging it to `bytes`
    /// (what preparation reserved, [`Sizes::bytes`]) and taking the
    /// already normalized owners of `previous`.
    pub(crate) fn new(
        plan: &Plan,
        sizes: Sizes,
        previous: Option<Kept>,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let pixels = sizes.width.saturating_mul(sizes.rows);
        let Kept {
            page,
            coverage,
            paints,
        } = previous.unwrap_or_default();
        let paint_scratch = paint_storage(&plan.paints, paints, bytes)?;
        let (format, layout) = page_layout(plan);
        let zeroed = |count: usize, bytes: &mut Meter| -> Result<Vec<f64>> {
            let mut values = reserve(count, bytes)?;
            values.resize(count, 0.0);
            Ok(values)
        };
        let wide = zeroed(plan.stack.wide, bytes)?;
        let mut levels = reserve(plan.stack.levels, bytes)?;
        levels.resize(plan.stack.levels, [0.0; 4]);
        let mut masks = reserve(plan.masks.len(), bytes)?;
        for mask in &plan.masks {
            masks.push(MaskCache {
                values: zeroed(mask_values(mask.extent, sizes.rows), bytes)?,
                rect: DeviceRect::default(),
                band: None,
            });
        }
        let mut active = reserve(plan.groups.len(), bytes)?;
        for group in &plan.groups {
            active.push(Active::new(reserve(group.items.len(), bytes)?));
        }
        let colors = SpanColors {
            colors: reserve(sizes.width.saturating_mul(sizes.channels), bytes)?,
            alpha: reserve(sizes.width, bytes)?,
            ..SpanColors::default()
        };
        let mut selection = Vec::new();
        if sizes.selections {
            selection = reserve(pixels, bytes)?;
            selection.resize(pixels, 0);
        }
        let run_shapes = reserve(run_shapes(plan), bytes)?;
        // Only a shading pattern's outline needs a second coverage.
        let outlines = plan.items.iter().any(|item| {
            matches!(&item.kind, ItemKind::Mark(mark) if matches!(mark.boundary, Boundary::Shape(_)))
        });
        let mut dense = reserve(sizes.width, bytes)?;
        dense.resize(sizes.width, 0);
        let outline = if outlines {
            warm_coverage(sizes, None, bytes)?
        } else {
            BandCoverage::default()
        };
        Ok(Self {
            page: PageBuffer::new((format, layout), pixels, page, bytes)?,
            extents: Extents::new(sizes.rows, bytes)?,
            wide,
            levels,
            masks,
            coverage: warm_coverage(sizes, coverage, bytes)?,
            boundary: (outline, dense),
            colors,
            paint_scratch,
            alpha: zeroed(sizes.width, bytes)?,
            shape: zeroed(sizes.width, bytes)?,
            texel_shape: reserve(sizes.width, bytes)?,
            mask: zeroed(sizes.width, bytes)?,
            source: zeroed(sizes.width.saturating_mul(sizes.channels), bytes)?,
            initial: zeroed(sizes.width.saturating_mul(sizes.stride), bytes)?,
            selection,
            run_shapes,
            active,
            usage: Usage::default(),
            capacity: (sizes.rows, pixels),
        })
    }

    /// Bytes retained.
    pub(crate) fn bytes(&self) -> usize {
        let f64s = self.wide.capacity()
            + self
                .masks
                .iter()
                .map(|mask| mask.values.capacity())
                .sum::<usize>()
            + self.colors.colors.capacity()
            + self.colors.alpha.capacity()
            + self.alpha.capacity()
            + self.shape.capacity()
            + self.texel_shape.capacity()
            + self.mask.capacity()
            + self.source.capacity()
            + self.initial.capacity();
        f64s * size_of::<f64>()
            + self.levels.capacity() * size_of::<[f32; 4]>()
            + self.page.bytes()
            + self.selection.capacity() * 2
            + self.extents.rows.capacity() * size_of::<[u32; 2]>()
            + self.boundary.1.capacity()
            + self.run_shapes.capacity() * size_of::<u32>()
            + self.active.iter().map(Active::bytes).sum::<usize>()
            + self.active.capacity() * size_of::<Active>()
            + self.masks.capacity() * size_of::<MaskCache>()
            + self.paint_scratch.capacity() * size_of::<PaintScratch>()
            + self
                .paint_scratch
                .iter()
                .map(PaintScratch::bytes)
                .sum::<usize>()
            + self.coverage.retained_bytes()
            + self.boundary.0.retained_bytes()
    }

    /// Clears the state of a previous render.
    pub(crate) fn restart(&mut self) {
        for active in &mut self.active {
            active.reset();
        }
        for mask in &mut self.masks {
            mask.band = None;
        }
        for scratch in &mut self.paint_scratch {
            scratch.reset();
        }
        self.usage = Usage::default();
    }
}

/// Exactly reusable owners of a previous workspace. Every other buffer is
/// dropped before any replacement allocation, including the rasterizer's.
#[derive(Debug, Default)]
pub(crate) struct Kept {
    page: Option<PageBuffer>,
    coverage: Option<BandCoverage>,
    paints: Option<Vec<PaintScratch>>,
}

impl Kept {
    fn new(plan: &Plan, previous: Workspace) -> Self {
        let pixels = plan.sizes.width.saturating_mul(plan.sizes.rows);
        let Workspace {
            page,
            mut coverage,
            paint_scratch,
            capacity,
            ..
        } = previous;
        coverage.clear();
        Self {
            page: page.keep(page_layout(plan), pixels),
            coverage: (capacity == (plan.sizes.rows, pixels)).then_some(coverage),
            paints: keep_paints(&plan.paints, paint_scratch),
        }
    }
}

/// Paint owners inside the new reservation, with incompatible layouts
/// dropped before any replacement is made.
fn keep_paints(sources: &[Source], mut previous: Vec<PaintScratch>) -> Option<Vec<PaintScratch>> {
    if previous.capacity() != sources.len() {
        return None;
    }
    previous.truncate(sources.len());
    for (scratch, source) in previous.iter_mut().zip(sources) {
        if !scratch.fits(source) {
            *scratch = PaintScratch::default();
        }
    }
    Some(previous)
}

/// The page's storage representation, including an empty plan's default.
fn page_layout(plan: &Plan) -> (Format, Layout) {
    plan.groups.first().map_or(
        (
            Format::F64,
            Layout {
                process: pagura_color::Process::Rgb,
                spots: 0,
            },
        ),
        |page| (page.format, page.layout),
    )
}

/// A coverage buffer grown once to the largest band of one mark (a span
/// per pixel), so that rasterization does not allocate, charged to
/// `bytes`: `kept` when there is one.
fn warm_coverage(
    sizes: Sizes,
    kept: Option<BandCoverage>,
    bytes: &mut Meter,
) -> Result<BandCoverage> {
    let capacity = coverage_capacity(sizes.width, sizes.rows);
    match kept {
        Some(coverage) => {
            bytes.charge(capacity.bytes())?;
            Ok(coverage)
        }
        None => BandCoverage::with_capacity(capacity, bytes),
    }
}

/// The capacity of coverage for every pixel of `rows` rows `width` wide
/// a partial cell and a span of its own.
pub(crate) fn coverage_capacity(width: usize, rows: usize) -> pagura_raster::Capacity {
    let cells = width.saturating_mul(rows);
    pagura_raster::Capacity {
        rows,
        spans: cells,
        cells,
    }
}

#[cfg(test)]
mod tests;
