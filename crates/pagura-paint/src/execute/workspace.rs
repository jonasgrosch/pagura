//! Every buffer execution writes, allocated once by preparation: the page's
//! band buffer, the stack of group buffers, soft-mask values, coverage and
//! span scratch.

use super::Active;
use crate::format::Format;
use crate::plan::{ItemKind, Layout, Plan};
use crate::source::SpanColors;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Result};
use pagura_raster::{BandCoverage, DeviceRect};

/// The page group's band buffer in its storage format.
#[derive(Debug)]
pub(crate) enum PageBuffer {
    /// f64 pixels of the page's layout.
    F64(Vec<f64>),
    /// Premultiplied RGBA words with the background embedded.
    N16(Vec<[u16; 4]>),
    /// Premultiplied RGBA levels with the background embedded.
    F32(Vec<[f32; 4]>),
    /// Premultiplied RGBA bytes with the background embedded.
    U8(Vec<[u8; 4]>),
}

impl PageBuffer {
    /// Bytes per pixel of `format` with `layout`.
    pub(crate) fn pixel_bytes(format: Format, layout: Layout) -> usize {
        match format {
            Format::F64 => layout.stride() * size_of::<f64>(),
            Format::N16 => size_of::<[u16; 4]>(),
            Format::F32 => size_of::<[f32; 4]>(),
            Format::U8 => size_of::<[u8; 4]>(),
        }
    }

    fn new(format: Format, pixels: usize, layout: Layout) -> Result<Self> {
        Ok(match format {
            Format::F64 => Self::F64(reserve(pixels.saturating_mul(layout.stride()))?),
            Format::N16 => Self::N16(reserve(pixels)?),
            Format::F32 => Self::F32(reserve(pixels)?),
            Format::U8 => Self::U8(reserve(pixels)?),
        })
    }

    /// Starts a band of `count` pixels in rows of `width`: transparent, or
    /// for narrow storage the premultiplied background. Only the columns
    /// the previous band painted are restored; the rest still hold it.
    pub(crate) fn reset(
        &mut self,
        extents: &mut Extents,
        (count, width): (usize, usize),
        background: [u8; 4],
        layout: Layout,
    ) {
        let words = background.map(|b| u16::from(b) * 257);
        let levels = background.map(f32::from);
        let stride = layout.stride();
        for (row, extent) in extents.rows.iter_mut().enumerate() {
            let [lo, hi] = *extent;
            *extent = EMPTY;
            if lo >= hi {
                continue;
            }
            let (lo, hi) = (
                row * width + u32_to_usize(lo),
                row * width + u32_to_usize(hi),
            );
            match self {
                Self::F64(values) => fill(values, lo * stride..hi * stride, 0.0),
                Self::N16(pixels) => fill(pixels, lo..hi, words),
                Self::F32(pixels) => fill(pixels, lo..hi, levels),
                Self::U8(pixels) => fill(pixels, lo..hi, background),
            }
        }
        match self {
            Self::F64(values) => grow(values, count * stride, 0.0),
            Self::N16(pixels) => grow(pixels, count, words),
            Self::F32(pixels) => grow(pixels, count, levels),
            Self::U8(pixels) => grow(pixels, count, background),
        }
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        match self {
            Self::F64(values) => values.capacity() * size_of::<f64>(),
            Self::N16(words) => words.capacity() * size_of::<[u16; 4]>(),
            Self::F32(levels) => levels.capacity() * size_of::<[f32; 4]>(),
            Self::U8(bytes) => bytes.capacity() * size_of::<[u8; 4]>(),
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
    fn new(rows: usize) -> Result<Self> {
        let mut values = reserve(rows)?;
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

    /// The painted columns of `row`, if any.
    pub(crate) fn get(&self, row: usize) -> Option<(usize, usize)> {
        let [lo, hi] = *self.rows.get(row)?;
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
    /// U8 group buffers alive at once.
    pub(crate) bytes: Vec<[u8; 4]>,
    /// Soft-mask values by mask.
    pub(crate) masks: Vec<MaskCache>,
    /// One mark's coverage.
    pub(crate) coverage: BandCoverage,
    /// The coverage of a shading pattern's `BBox` outline, and one span of
    /// it as bytes.
    pub(crate) boundary: (BandCoverage, Vec<u8>),
    /// One span of paint.
    pub(crate) colors: SpanColors,
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
    /// The gathered coverage rows of a glyph run, zero between runs, and
    /// each row's gathered columns.
    pub(crate) gathered: (Vec<u8>, Vec<[u32; 2]>),
    /// Active items per group.
    pub(crate) active: Vec<Active>,
    /// What the last render used.
    pub(crate) usage: Usage,
}

/// The sizes of a workspace, from which preparation charges it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// The bytes [`Workspace::new`] allocates for `plan`, at most.
    pub(crate) fn bytes(&self, plan: &Plan) -> usize {
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
        let gathered = gathered(plan, self.rows)
            .saturating_add(self.rows.saturating_mul(size_of::<[u32; 2]>()));
        let f64s = plan.stack.wide.saturating_add(masks).saturating_add(spans);
        f64s.saturating_mul(size_of::<f64>())
            .saturating_add(plan.stack.levels.saturating_mul(size_of::<[f32; 4]>()))
            .saturating_add(plan.stack.bytes.saturating_mul(size_of::<[u8; 4]>()))
            .saturating_add(page)
            .saturating_add(coverage)
            .saturating_add(gathered)
            .saturating_add(if self.selections { pixels * 2 } else { 0 })
            .saturating_add(active.saturating_mul(size_of::<u32>()))
            .saturating_add(plan.groups.len() * size_of::<Active>())
            .saturating_add(plan.masks.len() * size_of::<MaskCache>())
    }
}

/// Bytes of the gathered rows: the largest run that gathers, within the
/// band rows.
fn gathered(plan: &Plan, rows: usize) -> usize {
    plan.items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Run(run) if run.gather => Some(
                u32_to_usize(item.bounds.width())
                    .saturating_mul(u32_to_usize(item.bounds.height()).min(rows)),
            ),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// f64 values a soft mask's cache holds.
pub(crate) fn mask_values(extent: DeviceRect, rows: usize) -> usize {
    u32_to_usize(extent.width()).saturating_mul(u32_to_usize(extent.height()).min(rows))
}

fn reserve<T>(count: usize) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("render buffer allocation failed"))?;
    Ok(values)
}

impl Workspace {
    /// Allocates the storage `plan` executes in; preparation has charged
    /// [`Sizes::bytes`] before.
    pub(crate) fn new(plan: &Plan, sizes: Sizes) -> Result<Self> {
        let pixels = sizes.width.saturating_mul(sizes.rows);
        let (format, layout) = plan.groups.first().map_or(
            (
                Format::F64,
                Layout {
                    process: pagura_color::Process::Rgb,
                    spots: 0,
                },
            ),
            |page| (page.format, page.layout),
        );
        let mut wide = reserve(plan.stack.wide)?;
        wide.resize(plan.stack.wide, 0.0);
        let mut levels = reserve(plan.stack.levels)?;
        levels.resize(plan.stack.levels, [0.0; 4]);
        let mut bytes = reserve(plan.stack.bytes)?;
        bytes.resize(plan.stack.bytes, [0; 4]);
        let mut masks = Vec::new();
        masks
            .try_reserve_exact(plan.masks.len())
            .map_err(|_| Error::limit("render buffer allocation failed"))?;
        for mask in &plan.masks {
            let mut values = reserve(mask_values(mask.extent, sizes.rows))?;
            values.resize(mask_values(mask.extent, sizes.rows), 0.0);
            masks.push(MaskCache {
                values,
                rect: DeviceRect::default(),
                band: None,
            });
        }
        let mut active = Vec::new();
        active
            .try_reserve_exact(plan.groups.len())
            .map_err(|_| Error::limit("render buffer allocation failed"))?;
        for group in &plan.groups {
            active.push(Active::with_capacity(group.items.len())?);
        }
        let scratch = |count: usize| -> Result<Vec<f64>> {
            let mut values = reserve(count)?;
            values.resize(count, 0.0);
            Ok(values)
        };
        let colors = SpanColors {
            colors: reserve(sizes.width.saturating_mul(sizes.channels))?,
            alpha: reserve(sizes.width)?,
            ..SpanColors::default()
        };
        let mut selection = Vec::new();
        if sizes.selections {
            selection = reserve(pixels)?;
            selection.resize(pixels, 0);
        }
        let mut gathered_rows = reserve(gathered(plan, sizes.rows))?;
        gathered_rows.resize(gathered(plan, sizes.rows), 0);
        let mut gathered_extents = reserve(sizes.rows)?;
        gathered_extents.resize(sizes.rows, super::glyphs::EMPTY);
        Ok(Self {
            page: PageBuffer::new(format, pixels, layout)?,
            extents: Extents::new(sizes.rows)?,
            wide,
            levels,
            bytes,
            masks,
            coverage: warm_coverage(sizes),
            boundary: (warm_coverage(sizes), {
                let mut dense = reserve(sizes.width)?;
                dense.resize(sizes.width, 0);
                dense
            }),
            colors,
            alpha: scratch(sizes.width)?,
            shape: scratch(sizes.width)?,
            texel_shape: reserve(sizes.width)?,
            mask: scratch(sizes.width)?,
            source: scratch(sizes.width.saturating_mul(sizes.channels))?,
            initial: scratch(sizes.width.saturating_mul(sizes.stride))?,
            selection,
            gathered: (gathered_rows, gathered_extents),
            active,
            usage: Usage::default(),
        })
    }

    /// Bytes retained.
    #[cfg(test)]
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
            + self.bytes.capacity() * size_of::<[u8; 4]>()
            + self.page.bytes()
            + self.selection.capacity() * 2
            + self.extents.rows.capacity() * size_of::<[u32; 2]>()
            + self.boundary.1.capacity()
            + self.gathered.0.capacity()
            + self.gathered.1.capacity() * size_of::<[u32; 2]>()
            + self.active.iter().map(Active::bytes).sum::<usize>()
            + self.active.capacity() * size_of::<Active>()
            + self.masks.capacity() * size_of::<MaskCache>()
    }

    /// Clears the state of a previous render.
    pub(crate) fn restart(&mut self) {
        for active in &mut self.active {
            active.reset();
        }
        for mask in &mut self.masks {
            mask.band = None;
        }
        self.usage = Usage::default();
    }
}

/// A coverage buffer grown once to the largest band of one mark (a span
/// per pixel), so that rasterization does not allocate.
fn warm_coverage(sizes: Sizes) -> BandCoverage {
    let cells = sizes.width.saturating_mul(sizes.rows);
    let capacity = pagura_raster::Capacity {
        rows: sizes.rows,
        spans: cells,
        cells,
    };
    // Charged by preparation already.
    let mut free = pagura_core::Meter::new(usize::MAX, "coverage bytes");
    BandCoverage::with_capacity(capacity, &mut free).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_buffers_start_with_the_premultiplied_background() {
        let layout = Layout {
            process: pagura_color::Process::Rgb,
            spots: 0,
        };
        let mut page = PageBuffer::new(Format::N16, 2, layout).unwrap();
        let mut extents = Extents::new(1).unwrap();
        page.reset(&mut extents, (2, 2), [10, 20, 30, 40], layout);
        let PageBuffer::N16(words) = &page else {
            unreachable!()
        };
        assert_eq!(words[1], [2570, 5140, 7710, 10280]);
        let mut wide = PageBuffer::new(Format::F64, 2, layout).unwrap();
        wide.reset(&mut extents, (2, 2), [10, 20, 30, 40], layout);
        let PageBuffer::F64(values) = &wide else {
            unreachable!()
        };
        assert!(values.iter().all(|&v| v == 0.0));
        assert_eq!(crate::plan::area(DeviceRect::default()), 0);
    }
}
