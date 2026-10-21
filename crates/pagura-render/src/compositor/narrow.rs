//! Normal premultiplied stores on the 0..255 level scale (11.3.3).
//! N16's single integer rounding contracts error below 128/257 levels;
//! F32 admission bounds accumulated rounding before this buffer is allocated.
//! Legacy U8 storage rounds each store once; normal planning no longer selects it.
use super::{Source, plan::Format};
use pagura_core::{Error, Result};

pub(super) enum Buffer {
    U8(Bytes),
    N16(Storage<u16>),
    F32(Storage<f32>),
}

/// Pixels outside `painted` still hold the constructor's background.
/// Zero-coverage and same-colour stores may widen this conservative interval;
/// no store shrinks it or changes the dense output contract.
pub(super) struct Storage<T> {
    pixels: Vec<[T; 4]>,
    painted: std::ops::Range<usize>,
}

impl<T> Storage<T> {
    /// Validates a span before recording the stores it may receive.
    fn span(&mut self, offset: usize, count: usize) -> Result<&mut [[T; 4]]> {
        let end = offset + count;
        let row = self
            .pixels
            .get_mut(offset..end)
            .ok_or(Error::invalid_argument("mask outside colour buffer"))?;
        if offset < end {
            self.painted.start = self.painted.start.min(offset);
            self.painted.end = self.painted.end.max(end);
        }
        Ok(row)
    }
}

/// A solid source prepared once for a run of fragment masks. Borrowing the
/// existing buffer preserves its admission and avoids another allocation.
pub(super) enum MaskSink<'a> {
    U8 {
        bytes: &'a mut Bytes,
        color: [u8; 4],
        alpha: u8,
    },
    N16 {
        words: &'a mut Storage<u16>,
        size: (usize, usize),
        color: [u16; 4],
    },
    F32 {
        floats: &'a mut Storage<f32>,
        size: (usize, usize),
        color: [f32; 4],
        scale: f32,
    },
}

impl MaskSink<'_> {
    /// Stores a fragment mask using the unchanged clipped coverage bytes.
    pub(super) fn mask(
        &mut self,
        mask: (&[u8], usize, (i64, i64)),
        surface: ((i64, i64), [i64; 4]),
        clip: MaskClip<'_>,
        scratch: &mut Vec<u8>,
    ) -> Result<()> {
        match self {
            Self::U8 {
                bytes,
                color,
                alpha,
            } => bytes.mask(mask, surface, (*color, *alpha), clip, scratch),
            Self::N16 { words, size, color } => {
                let width = size.0;
                let Storage { pixels, painted } = &mut **words;
                mask_rows(
                    pixels,
                    *size,
                    mask,
                    surface,
                    clip,
                    scratch,
                    |(row, column), target, coverage| {
                        let offset = row * width + column;
                        painted.start = painted.start.min(offset);
                        painted.end = painted.end.max(offset + target.len());
                        if uniform(coverage) == Some(255) {
                            target.fill(*color);
                        } else {
                            words_row(target, coverage, *color);
                        }
                    },
                )
            }
            Self::F32 {
                floats,
                size,
                color,
                scale,
            } => {
                let width = size.0;
                let Storage { pixels, painted } = &mut **floats;
                mask_rows(
                    pixels,
                    *size,
                    mask,
                    surface,
                    clip,
                    scratch,
                    |(row, column), target, coverage| {
                        let offset = row * width + column;
                        painted.start = painted.start.min(offset);
                        painted.end = painted.end.max(offset + target.len());
                        if uniform(coverage) == Some(255) && 255.0 * *scale == 1.0 {
                            target.fill(*color);
                        } else {
                            float_row(target, coverage, *color, *scale);
                        }
                    },
                )
            }
        }
    }
}

/// Premultiplied RGBA bytes in rows of `width` pixels, rounding once per
/// store. Each row records the columns that stores have reached; every other
/// pixel of the row still holds the background.
pub(super) struct Bytes {
    pixels: Vec<u8>,
    width: usize,
    background: [u8; 4],
    /// Per row, the columns `[lo, hi)` that any store has reached; an empty
    /// extent is `[u32::MAX, 0]`, so extending it is a min and a max.
    painted: Vec<[u32; 2]>,
    /// The rows that offsets address: `[first, first + rows)`.
    first: usize,
    rows: usize,
}

impl Bytes {
    /// The bytes a surface of `count` pixels in `height` rows holds: its
    /// pixels and the painted extent of each row.
    pub(super) fn charge(count: usize, height: usize) -> Option<usize> {
        count
            .checked_mul(4)?
            .checked_add(height.checked_mul(size_of::<[u32; 2]>())?)
    }

    /// A `width` by `height` surface of `background`, in `storage` when its
    /// allocation suffices.
    pub(super) fn new(
        (width, height): (usize, usize),
        background: [u8; 4],
        storage: Vec<u8>,
    ) -> Result<Self> {
        let mut bytes = Self::banded((width, height), background, storage)?;
        bytes.fill_to(height);
        bytes.rows = height;
        Ok(bytes)
    }

    /// [`Self::new`] for a surface addressed in bands from the top: each
    /// row takes its background when a window first reaches it, so that
    /// the band's stores find it in cache. No row is addressed before the
    /// first window.
    pub(super) fn banded(
        (width, height): (usize, usize),
        background: [u8; 4],
        mut storage: Vec<u8>,
    ) -> Result<Self> {
        let count = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .ok_or(Error::limit("colour buffer size overflow"))?;
        storage.clear();
        storage
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("colour buffer allocation failed"))?;
        let mut painted = Vec::new();
        painted
            .try_reserve_exact(height)
            .map_err(|_| Error::limit("colour buffer allocation failed"))?;
        painted.resize(height, [u32::MAX, 0]);
        Ok(Self {
            pixels: storage,
            width,
            background,
            painted,
            first: 0,
            rows: 0,
        })
    }

    /// Extends the pixels with background rows up to row `end`. Capacity
    /// for every row was reserved at construction.
    fn fill_to(&mut self, end: usize) {
        let start = self.pixels.len();
        let end = end.min(self.painted.len()) * self.width * 4;
        if end <= start {
            return;
        }
        // A gray or transparent background is one repeated byte.
        self.pixels.resize(end, self.background[0]);
        if self.background.iter().any(|&b| b != self.background[0]) {
            self.pixels[start..]
                .as_chunks_mut::<4>()
                .0
                .fill(self.background);
        }
    }

    /// Whether no store has reached the rows of the current window, which
    /// then all hold the background.
    fn blank(&self) -> bool {
        self.painted[self.first..self.first + self.rows]
            .iter()
            .all(|&[lo, hi]| lo >= hi)
    }

    /// Addresses `rows` rows from `skip` rows below the current first row.
    pub(super) fn window(&mut self, skip: usize, rows: usize) -> Result<()> {
        let first = self.first + skip;
        if first + rows > self.painted.len() {
            return Err(Error::invalid_argument("band outside byte storage"));
        }
        (self.first, self.rows) = (first, rows);
        self.fill_to(first + rows);
        Ok(())
    }

    /// The surface row and column of a window offset.
    fn position(&self, offset: usize) -> Result<(usize, usize)> {
        let (row, column) = (offset / self.width, offset % self.width);
        if row >= self.rows {
            return Err(Error::invalid_argument("mask outside colour buffer"));
        }
        Ok((self.first + row, column))
    }

    /// The pixels from `offset` for `count` columns of one row, recorded as
    /// painted.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "columns are below the surface width, which fits u32"
    )]
    fn run(&mut self, offset: usize, count: usize) -> Result<&mut [[u8; 4]]> {
        let (row, column) = self.position(offset)?;
        if column + count > self.width {
            return Err(Error::invalid_argument("run crosses a colour buffer row"));
        }
        let painted = self
            .painted
            .get_mut(row)
            .ok_or(Error::invalid_argument("mask outside colour buffer"))?;
        *painted = [
            painted[0].min(column as u32),
            painted[1].max((column + count) as u32),
        ];
        let start = row * self.width + column;
        Ok(&mut self.pixels.as_chunks_mut::<4>().0[start..start + count])
    }

    /// Blends `color` at each coverage of `coverage` from `offset` within one
    /// row. An opaque colour equal to the background leaves background pixels
    /// as they are at every coverage (c·a + c·(255 − a) rounds back to c), so
    /// such a run is restricted to the columns already painted.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "columns are below the surface width, which fits u32"
    )]
    pub(super) fn blend(
        &mut self,
        offset: usize,
        coverage: &[u8],
        color: [u8; 4],
        alpha: u8,
    ) -> Result<()> {
        let (mut offset, mut coverage) = (offset, coverage);
        if alpha == 255 && color == self.background && color[3] == 255 {
            let (row, column) = self.position(offset)?;
            let [lo, hi] = *self
                .painted
                .get(row)
                .ok_or(Error::invalid_argument("mask outside colour buffer"))?;
            let start = column.max(lo as usize);
            let end = (column + coverage.len()).min(hi as usize);
            if start >= end {
                return Ok(());
            }
            offset += start - column;
            coverage = &coverage[start - column..end - column];
            debug_assert!(start as u32 >= lo && end as u32 <= hi);
        }
        let pixels = self.run(offset, coverage.len())?;
        let source = super::lowp::Source::new(color, alpha);
        // Rectangle and full runs hold one coverage throughout.
        match uniform(coverage) {
            Some(coverage) => super::lowp::run(pixels, coverage, &source),
            None => super::lowp::row(pixels, coverage, &source),
        }
        Ok(())
    }

    /// Blends `mask`, rows of `stride` levels whose first pixel is at
    /// `origin` in device space, into the part of `bounds` (a device
    /// rectangle within this surface, whose first pixel is `surface`) that
    /// it covers, scaled by `clip`'s levels when given: a product over the
    /// device rectangle it names.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "mask extents are budgeted; rectangles are intersected with the surface \
                  before indexing"
    )]
    pub(super) fn mask(
        &mut self,
        (mask, stride, origin): (&[u8], usize, (i64, i64)),
        (surface, bounds): ((i64, i64), [i64; 4]),
        (color, alpha): ([u8; 4], u8),
        clip: MaskClip<'_>,
        scratch: &mut Vec<u8>,
    ) -> Result<()> {
        let Some([x0, y0, x1, y1]) = mask_area((mask, stride, origin), bounds, clip) else {
            return Ok(());
        };
        let (count, height) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let outside = || Error::invalid_argument("mask outside colour buffer");
        let row = usize::try_from(y0 - surface.1).map_err(|_| outside())?;
        let column = usize::try_from(x0 - surface.0).map_err(|_| outside())?;
        if row + height > self.rows || column + count > self.width {
            return Err(outside());
        }
        let first = self.first + row;
        let extent = [column as u32, (column + count) as u32];
        let source = super::lowp::Source::new(color, alpha);
        let rows = self
            .pixels
            .as_chunks_mut::<4>()
            .0
            .chunks_exact_mut(self.width);
        let painted = self.painted[first..first + height].iter_mut();
        // A mask wholly inside an unscaled clip blends its rows as they are.
        if clip.product.is_none()
            && clip
                .rectangle
                .is_none_or(|r| inside(r, (x0, y0, count)) && inside(r, (x0, y1 - 1, count)))
        {
            for painted in painted {
                *painted = [painted[0].min(extent[0]), painted[1].max(extent[1])];
            }
            let start = (y0 - origin.1) as usize * stride + (x0 - origin.0) as usize;
            let masks = mask[start..].chunks(stride).map(|row| &row[..count]);
            let targets = rows.skip(first).map(|row| &mut row[column..column + count]);
            super::lowp::rows(targets.zip(masks).take(height), &source);
            return Ok(());
        }
        for ((y, pixels), painted) in (y0..y1).zip(rows.skip(first)).zip(painted) {
            let start = (y - origin.1) as usize * stride + (x0 - origin.0) as usize;
            let row = &mask[start..start + count];
            let coverage = clipped_row(row, (x0, y), clip, scratch)?;
            *painted = [painted[0].min(extent[0]), painted[1].max(extent[1])];
            super::lowp::row(&mut pixels[column..column + count], coverage, &source);
        }
        Ok(())
    }

    /// Stores `pixels` from `offset` within one row.
    pub(super) fn copy(&mut self, offset: usize, pixels: &[[u8; 4]]) -> Result<()> {
        self.run(offset, pixels.len())?.copy_from_slice(pixels);
        Ok(())
    }

    /// Composes opaque `colors` from `offset` within one row at the levels
    /// of `opacities`, whose first channel is each pixel's alpha.
    pub(super) fn over(
        &mut self,
        offset: usize,
        colors: &[[u8; 4]],
        opacities: &[[u8; 4]],
    ) -> Result<()> {
        for ((pixel, &color), opacity) in self
            .run(offset, colors.len())?
            .iter_mut()
            .zip(colors)
            .zip(opacities)
        {
            super::lowp::pixel(pixel, color, opacity[0]);
        }
        Ok(())
    }

    fn pixel(&mut self, index: usize, color: [u8; 4], alpha: u8) -> Result<()> {
        let pixel = self
            .run(index, 1)
            .map_err(|_| Error::invalid_argument("source outside colour buffer"))?;
        super::lowp::pixel(&mut pixel[0], color, alpha);
        Ok(())
    }

    /// The premultiplied RGBA bytes, row by row.
    pub(super) fn into_rgba(mut self) -> Vec<u8> {
        self.fill_to(self.painted.len());
        self.pixels
    }
}

/// The one level that every coverage of a nonempty run holds, compared
/// sixteen at a time.
fn uniform(coverage: &[u8]) -> Option<u8> {
    let &first = coverage.first()?;
    let (blocks, tail) = coverage.as_chunks::<16>();
    (blocks.iter().all(|block| *block == [first; 16]) && tail.iter().all(|&c| c == first))
        .then_some(first)
}

impl Buffer {
    /// Prepares one solid source for fragment masks. N16 admits only opaque
    /// sources; its planner has already excluded transfers and other feeds.
    pub(super) fn mask_sink(
        &mut self,
        size: (usize, usize),
        color: [f64; 4],
        alpha: f64,
    ) -> Result<MaskSink<'_>> {
        match self {
            Self::U8(bytes) => {
                let (color, alpha) = byte_source(color, alpha);
                Ok(MaskSink::U8 {
                    bytes,
                    color,
                    alpha,
                })
            }
            Self::N16(words) => {
                if alpha != 1.0 {
                    return Err(Error::invalid_argument(
                        "nonopaque fragment source in N16 buffer",
                    ));
                }
                if size.0 == 0
                    || size.1 == 0
                    || size.0.checked_mul(size.1) != Some(words.pixels.len())
                {
                    return Err(Error::invalid_argument(
                        "fragment surface differs from N16 buffer",
                    ));
                }
                Ok(MaskSink::N16 {
                    words,
                    size,
                    color: [word(color[0]), word(color[1]), word(color[2]), 65535],
                })
            }
            Self::F32(floats) => {
                if size.0 == 0
                    || size.1 == 0
                    || size.0.checked_mul(size.1) != Some(floats.pixels.len())
                {
                    return Err(Error::invalid_argument(
                        "fragment surface differs from F32 buffer",
                    ));
                }
                Ok(MaskSink::F32 {
                    floats,
                    size,
                    color: [level(color[0]), level(color[1]), level(color[2]), 255.0],
                    scale: scalar(alpha / 255.0),
                })
            }
        }
    }

    pub(super) fn new(format: Format, count: usize, background: [u8; 4]) -> Result<Self> {
        fn filled<T: Clone>(count: usize, value: T) -> Result<Vec<T>> {
            let mut pixels = Vec::new();
            pixels
                .try_reserve_exact(count)
                .map_err(|_| Error::limit("colour buffer allocation failed"))?;
            pixels.resize(count, value);
            Ok(pixels)
        }
        match format {
            Format::U8 => Ok(Self::U8(Bytes::new((count, 1), background, Vec::new())?)),
            Format::N16 => Ok(Self::N16(Storage {
                pixels: filled(count, background.map(|v| u16::from(v) * 257))?,
                painted: count..0,
            })),
            Format::F32 => Ok(Self::F32(Storage {
                pixels: filled(count, background.map(f32::from))?,
                painted: count..0,
            })),
            Format::F64 => Err(Error::invalid_argument("wide format in narrow buffer")),
        }
    }
    pub(super) fn bytes(&self) -> usize {
        match self {
            // As charged by `Bytes::charge`, from the lengths it allocated.
            Self::U8(v) => v.pixels.len() + size_of_val(v.painted.as_slice()),
            Self::N16(v) => v.pixels.len() * 8,
            Self::F32(v) => v.pixels.len() * 16,
        }
    }
    /// Whether painting an opaque `color` at `alpha` anywhere in the current
    /// window changes no pixel: it is the background of a window that no
    /// store has reached (see [`Bytes::blend`]).
    pub(super) fn unchanged_by(&self, color: [f64; 4], alpha: f64) -> bool {
        let Self::U8(bytes) = self else {
            return false;
        };
        let (color, alpha) = byte_source(color, alpha);
        alpha == 255 && color == bytes.background && bytes.blank()
    }
    /// Stores a dense mask of `width` columns whose first pixel is at
    /// `offset`, in rows `stride` apart, when this is byte storage.
    pub(super) fn small_mask(
        &mut self,
        (offset, stride, width): (usize, usize, usize),
        coverage: &[u8],
        color: [f64; 4],
        alpha: f64,
    ) -> Result<bool> {
        let Self::U8(bytes) = self else {
            return Ok(false);
        };
        if width == 0 || width > 64 || coverage.len() / width > 64 {
            return Ok(false);
        }
        let (color, alpha) = byte_source(color, alpha);
        for (row, mask) in coverage.chunks_exact(width).enumerate() {
            bytes.blend(offset + row * stride, mask, color, alpha)?;
        }
        Ok(true)
    }

    /// Stores one run of `Mask::spans`, which holds only full or only
    /// fractional coverage. A full run of an opaque source stores the source
    /// itself, so it is written as one slice fill.
    pub(super) fn path(
        &mut self,
        offset: usize,
        coverage: &[u8],
        color: [f64; 4],
        alpha: f64,
    ) -> Result<()> {
        let full = coverage.first() == Some(&255);
        debug_assert!(!full || coverage.iter().all(|&c| c == 255));
        match self {
            Self::U8(bytes) => {
                let (color, alpha) = byte_source(color, alpha);
                bytes.blend(offset, coverage, color, alpha)?;
            }
            Self::N16(words) => {
                let color = [word(color[0]), word(color[1]), word(color[2]), 65535];
                let row = words.span(offset, coverage.len())?;
                if full {
                    row.fill(color);
                    return Ok(());
                }
                words_row(row, coverage, color);
            }
            Self::F32(floats) => {
                let color = [level(color[0]), level(color[1]), level(color[2]), 255.0];
                let k = scalar(alpha / 255.0);
                let row = floats.span(offset, coverage.len())?;
                // `lerp` stores the target itself at a unit weight.
                if full && 255.0 * k == 1.0 {
                    row.fill(color);
                    return Ok(());
                }
                float_row(row, coverage, color, k);
            }
        }
        Ok(())
    }
    /// Dynamic sources may store anywhere. Mark once before their sampling
    /// loop, keeping interval bookkeeping out of each pixel store.
    pub(super) fn prepare_dynamic(&mut self) {
        if let Self::F32(floats) = self {
            floats.painted = 0..floats.pixels.len();
        }
    }

    /// A pixel of a dynamic operation prepared by [`Self::prepare_dynamic`].
    pub(super) fn paint_dynamic(&mut self, index: usize, source: Source) -> Result<()> {
        if let Self::U8(bytes) = self {
            let (color, alpha) = byte_source(source.color, source.alpha);
            return bytes.pixel(index, color, alpha);
        }
        let Self::F32(floats) = self else {
            return Err(Error::invalid_argument("dynamic source in N16 buffer"));
        };
        let Storage { pixels, painted } = floats;
        let count = pixels.len();
        let pixel = pixels
            .get_mut(index)
            .ok_or(Error::invalid_argument("source outside colour buffer"))?;
        debug_assert_eq!(*painted, 0..count);
        let color = [
            level(source.color[0]),
            level(source.color[1]),
            level(source.color[2]),
            255.0,
        ];
        lerp(pixel, color, scalar(source.alpha));
        Ok(())
    }
    /// Only page output onto the same opaque background may omit pixels
    /// outside this range. Dense output below makes no such assumption.
    pub(super) fn painted_range(&self) -> Option<std::ops::Range<usize>> {
        match self {
            Self::N16(words) => Some(words.painted.clone()),
            Self::F32(floats) => Some(floats.painted.clone()),
            Self::U8(_) => None,
        }
    }

    pub(super) fn output(&self, offset: usize, target: &mut [u8]) {
        match self {
            Self::U8(bytes) => {
                let start = (bytes.first * bytes.width + offset) * 4;
                let source = &bytes.pixels[start.min(bytes.pixels.len())..];
                let count = target.len().min(source.len());
                target[..count].copy_from_slice(&source[..count]);
            }
            Self::N16(words) => {
                let complete = target.len() / 4 * 4;
                for (out, &word) in target[..complete]
                    .iter_mut()
                    .zip(words.pixels[offset..].as_flattened())
                {
                    *out = word_byte(word);
                }
            }
            Self::F32(floats) => {
                for (out, pixel) in target
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(&floats.pixels[offset..])
                {
                    *out = pixel.map(byte);
                }
            }
        }
    }
}

/// Exactly `(word + 128) / 257`, using only 16-bit arithmetic.
/// Below saturation, `n = 256*q + r` gives `q` when `r >= q`, else `q - 1`.
/// Above it, both the original quotient and the saturated expression are 255.
fn word_byte(word: u16) -> u8 {
    let n = word.saturating_add(128);
    ((n - (n >> 8)) >> 8) as u8
}

/// The clip of a fragment mask: a product over a device rectangle,
/// and an axis-aligned rectangle whose coverage separates into a column and
/// a row factor.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct MaskClip<'a> {
    pub(super) product: Option<(&'a [u8], [i64; 4])>,
    pub(super) rectangle: Option<[f64; 4]>,
}

/// Intersects the same mask, viewport, product and fractional rectangle for
/// all storage kernels. Fragment extents have already passed their budgets.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "mask extents are budgeted and rectangle coordinates are clamped"
)]
fn mask_area(
    (mask, stride, origin): (&[u8], usize, (i64, i64)),
    bounds: [i64; 4],
    clip: MaskClip<'_>,
) -> Option<[i64; 4]> {
    if stride == 0 {
        return None;
    }
    let rows = (mask.len() / stride) as i64;
    let mut area = [
        origin.0.max(bounds[0]),
        origin.1.max(bounds[1]),
        (origin.0 + stride as i64).min(bounds[2]),
        (origin.1 + rows).min(bounds[3]),
    ];
    if let Some((_, rectangle)) = clip.product {
        area = [
            area[0].max(rectangle[0]),
            area[1].max(rectangle[1]),
            area[2].min(rectangle[2]),
            area[3].min(rectangle[3]),
        ];
    }
    if let Some([left, top, right, bottom]) = clip.rectangle {
        let pixel = |value: f64, round: fn(f64) -> f64| round(value.clamp(-1e9, 1e9)) as i64;
        area = [
            area[0].max(pixel(left, f64::floor)),
            area[1].max(pixel(top, f64::floor)),
            area[2].min(pixel(right, f64::ceil)),
            area[3].min(pixel(bottom, f64::ceil)),
        ];
    }
    (area[0] < area[2] && area[1] < area[3]).then_some(area)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the row lies within the budgeted product rectangle"
)]
fn clipped_row<'a>(
    row: &'a [u8],
    (x0, y): (i64, i64),
    clip: MaskClip<'_>,
    scratch: &'a mut Vec<u8>,
) -> Result<&'a [u8]> {
    let count = row.len();
    let mut scaled = false;
    if let Some((product, [cx0, cy0, cx1, _])) = clip.product {
        let at = (y - cy0) as usize * (cx1 - cx0) as usize + (x0 - cx0) as usize;
        let factors = product
            .get(at..at + count)
            .ok_or(Error::invalid_argument("mask exceeds clip"))?;
        scratch.clear();
        scratch.extend_from_slice(row);
        super::lowp::multiply(scratch, factors);
        scaled = true;
    }
    if let Some(rectangle) = clip.rectangle
        && !inside(rectangle, (x0, y, count))
    {
        if !scaled {
            scratch.clear();
            scratch.extend_from_slice(row);
            scaled = true;
        }
        scale_edges(rectangle, (x0, y), scratch);
    }
    Ok(if scaled { scratch.as_slice() } else { row })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "mask extents are budgeted and checked against the admitted surface"
)]
fn mask_rows<T>(
    pixels: &mut [[T; 4]],
    (width, rows): (usize, usize),
    (mask, stride, origin): (&[u8], usize, (i64, i64)),
    (surface, bounds): ((i64, i64), [i64; 4]),
    clip: MaskClip<'_>,
    scratch: &mut Vec<u8>,
    mut paint: impl FnMut((usize, usize), &mut [[T; 4]], &[u8]),
) -> Result<()> {
    let Some([x0, y0, x1, y1]) = mask_area((mask, stride, origin), bounds, clip) else {
        return Ok(());
    };
    let (count, height) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let outside = || Error::invalid_argument("mask outside colour buffer");
    let row = usize::try_from(y0 - surface.1).map_err(|_| outside())?;
    let column = usize::try_from(x0 - surface.0).map_err(|_| outside())?;
    if row.checked_add(height).is_none_or(|end| end > rows)
        || column.checked_add(count).is_none_or(|end| end > width)
    {
        return Err(outside());
    }
    for (row, (y, target)) in (row..).zip((y0..y1).zip(pixels.chunks_exact_mut(width).skip(row))) {
        let start = (y - origin.1) as usize * stride + (x0 - origin.0) as usize;
        let coverage = mask.get(start..start + count).ok_or_else(outside)?;
        let coverage = clipped_row(coverage, (x0, y), clip, scratch)?;
        paint((row, column), &mut target[column..column + count], coverage);
    }
    Ok(())
}

/// Arbitrary fragment rows may begin with full coverage and later contain
/// fractional or zero coverage; unlike Mask::spans, they are not full runs.
fn words_row(row: &mut [[u16; 4]], coverage: &[u8], color: [u16; 4]) {
    for (pixel, &c) in row.iter_mut().zip(coverage) {
        match c {
            0 => {}
            255 => *pixel = color,
            c => {
                for (out, source) in pixel.iter_mut().zip(color) {
                    *out = mix(source, *out, c);
                }
            }
        }
    }
}

/// The same admitted F32 store for dense spans and arbitrary fragment rows.
/// Coverage zero leaves the pixel untouched; alpha one keeps `lerp`'s exact
/// source assignment. The source and opacity scale are prepared once.
fn float_row(row: &mut [[f32; 4]], coverage: &[u8], color: [f32; 4], scale: f32) {
    for (pixel, &c) in row.iter_mut().zip(coverage) {
        let alpha = f32::from(c) * scale;
        if c != 0 {
            lerp(pixel, color, alpha);
        }
    }
}

#[cfg(test)]
#[path = "narrow_tests.rs"]
mod mask_tests;

/// The share of the pixel from `start` that lies in `[low, high)`.
#[expect(clippy::cast_precision_loss, reason = "device coordinates are bounded")]
fn overlap(start: i64, low: f64, high: f64) -> f64 {
    let start = start as f64;
    (high.min(start + 1.0) - low.max(start)).clamp(0.0, 1.0)
}

/// Whether the `count` pixels of row `y` from column `x0` lie wholly inside
/// `rectangle`.
#[expect(clippy::cast_possible_wrap, reason = "mask widths are bounded")]
fn inside([left, top, right, bottom]: [f64; 4], (x0, y, count): (i64, i64, usize)) -> bool {
    overlap(y, top, bottom) == 1.0
        && overlap(x0, left, right) == 1.0
        && overlap(x0 + count as i64 - 1, left, right) == 1.0
}

/// Scales a coverage row from device `(x0, y)` by `rectangle`'s coverage
/// there, a column factor times a row factor, rounding each product once.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "factors in [0, 1] scale to bytes; mask widths are bounded"
)]
fn scale_edges([left, top, right, bottom]: [f64; 4], (x0, y): (i64, i64), row: &mut [u8]) {
    let vertical = overlap(y, top, bottom);
    for (column, value) in row.iter_mut().enumerate() {
        let horizontal = overlap(x0 + column as i64, left, right);
        if vertical == 1.0 && horizontal == 1.0 {
            continue;
        }
        let level = (horizontal * vertical * 255.0).round() as u16;
        *value = ((u16::from(*value) * level + 127) / 255) as u8;
    }
}

/// An opaque byte colour and byte alpha for a normalized source.
pub(super) fn byte_source(color: [f64; 4], alpha: f64) -> ([u8; 4], u8) {
    (
        [
            byte(scalar(color[0] * 255.0)),
            byte(scalar(color[1] * 255.0)),
            byte(scalar(color[2] * 255.0)),
            255,
        ],
        byte(scalar(alpha * 255.0)),
    )
}

fn lerp(pixel: &mut [f32; 4], target: [f32; 4], alpha: f32) {
    if alpha == 1.0 {
        *pixel = target;
        return;
    }
    for (out, target) in pixel.iter_mut().zip(target) {
        *out += alpha * (target - *out);
    }
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "normalized source or alpha converted to the admitted storage precision"
)]
fn scalar(value: f64) -> f32 {
    value as f32
}
fn level(value: f64) -> f32 {
    let value = value.clamp(0.0, 1.0) * 255.0;
    let nearest = scalar(value);
    // Round inexact targets to an odd significand so a following nearest
    // byte store retains their side of an exactly representable half level.
    let truncated = nearest.to_bits() - u32::from(f64::from(nearest) > value);
    f32::from_bits(truncated | u32::from(f64::from(nearest) != value))
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite level rounded and clamped to a byte"
)]
fn byte(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "normalized colour rounded to 0..65535"
)]
fn word(value: f64) -> u16 {
    let level = value.clamp(0.0, 1.0) * 255.0;
    let word = (level * 257.0).round() as u16;
    let byte = (u32::from(word) + 128) / 257;
    let nearest = level.round() as u32;
    // Avoid double rounding at half levels. The adjacent word remains
    // within one word of the source, keeping the 128/257-level bound.
    match byte.cmp(&nearest) {
        std::cmp::Ordering::Greater => word - 1,
        std::cmp::Ordering::Less => word + 1,
        std::cmp::Ordering::Equal => word,
    }
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "convex combination of words stays within 0..65535"
)]
fn mix(source: u16, backdrop: u16, coverage: u8) -> u16 {
    let c = u32::from(coverage);
    ((u32::from(source) * c + u32::from(backdrop) * (255 - c) + 127) / 255) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rectangle_edges_scale_by_column_and_row_overlap() {
        let rectangle = [1.5, 0.25, 3.25, 2.0];
        assert!(inside(rectangle, (2, 1, 1)));
        assert!(!inside(rectangle, (1, 1, 2)));
        assert!(!inside(rectangle, (2, 0, 1)));
        let mut row = vec![255, 255, 255, 200];
        scale_edges(rectangle, (0, 0), &mut row);
        // Columns overlap 0, 1/2, 1 and 1/4; the row overlaps 3/4, so the
        // rounded levels are 0, 96, 191 and 48.
        let expected: Vec<u8> = [0u32, 96, 191, 48]
            .iter()
            .zip([255u32, 255, 255, 200])
            .map(|(level, v)| u8::try_from((v * level + 127) / 255).unwrap())
            .collect();
        assert_eq!(row, expected);
    }

    #[test]
    fn byte_bands_address_their_rows_and_skip_unpainted_background() {
        let mut bytes = Bytes::new((3, 4), [255; 4], Vec::new()).unwrap();
        bytes.window(1, 2).unwrap();
        bytes.blend(3, &[255, 128], [0, 0, 0, 255], 255).unwrap();
        assert!(bytes.window(2, 3).is_err());
        assert!(bytes.blend(6, &[255], [0, 0, 0, 255], 255).is_err());
        // A background-coloured run leaves unpainted rows untouched.
        bytes.blend(0, &[10, 10, 10], [255; 4], 255).unwrap();
        // Window row 1 is surface row 2, whose pixels start at byte 24.
        let pixels = bytes.into_rgba();
        assert_eq!(&pixels[24..32], &[0, 0, 0, 255, 127, 127, 127, 255]);
        assert!(pixels[..24].iter().all(|&v| v == 255));
    }

    #[test]
    fn banded_surfaces_refuse_stores_before_a_window() {
        let mut bytes = Bytes::banded((2, 2), [255; 4], Vec::new()).unwrap();
        assert!(bytes.blend(0, &[255], [0, 0, 0, 255], 255).is_err());
        bytes.window(0, 1).unwrap();
        assert!(bytes.blend(0, &[255], [0, 0, 0, 255], 255).is_ok());
    }

    #[test]
    fn banded_rows_take_their_background_when_first_addressed() {
        let background = [250, 240, 230, 255];
        let mut buffer = Buffer::U8(Bytes::banded((2, 4), background, vec![7; 64]).unwrap());
        let Buffer::U8(bytes) = &mut buffer else {
            unreachable!()
        };
        bytes.window(0, 2).unwrap();
        // The background over a blank window changes nothing; other colours
        // and translucent background do.
        let opaque = [250.0 / 255.0, 240.0 / 255.0, 230.0 / 255.0, 1.0];
        assert!(buffer.unchanged_by(opaque, 1.0));
        assert!(!buffer.unchanged_by(opaque, 0.5));
        assert!(!buffer.unchanged_by([0.0, 0.0, 0.0, 1.0], 1.0));
        let Buffer::U8(bytes) = &mut buffer else {
            unreachable!()
        };
        bytes.blend(2, &[255], [0, 0, 0, 255], 255).unwrap();
        assert!(!buffer.unchanged_by(opaque, 1.0));
        let Buffer::U8(bytes) = buffer else {
            unreachable!()
        };
        // Rows that no window reached hold the background too.
        let pixels = bytes.into_rgba();
        assert_eq!(pixels.len(), 32);
        assert_eq!(&pixels[8..12], &[0, 0, 0, 255]);
        for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            if index != 2 {
                assert_eq!(pixel, &background, "{index}");
            }
        }
    }

    fn value(buffer: &Buffer) -> u8 {
        let mut out = [0; 4];
        buffer.output(0, &mut out);
        out[0]
    }
    #[test]
    fn rational_normal_stacks() {
        // Independent rational recurrences of 11.3.3: 218.7107843137,
        // 219.5003822059, 93.9936190601, 93.6250166324, 5.0127537023.
        for format in [Format::N16, Format::F32] {
            let mut buffer = Buffer::new(format, 1, [255; 4]).unwrap();
            buffer.path(0, &[255], [0.85; 4], 1.0).unwrap();
            for _ in 0..2 {
                buffer.path(0, &[10], [0.95; 4], 1.0).unwrap();
            }
            assert_eq!(value(&buffer), 219);
            let mut buffer = Buffer::new(format, 1, [255; 4]).unwrap();
            for (s, c) in [(159, 95), (240, 10), (80, 1)] {
                buffer
                    .path(0, &[c], [f64::from(s) / 255.0; 4], 1.0)
                    .unwrap();
            }
            assert_eq!(
                value(&buffer),
                if format == Format::N16 { 219 } else { 220 }
            );
            for (n, expected) in [(254, 94), (255, 94), (1000, 5), (270_000, 0)] {
                let mut buffer = Buffer::new(format, 1, [255; 4]).unwrap();
                for _ in 0..n {
                    buffer.path(0, &[1], [0.0; 4], 1.0).unwrap();
                }
                assert_eq!(value(&buffer), expected, "{format:?} {n}");
            }
        }
    }
    #[test]
    fn translucent_stagnation_requires_the_format_bound() {
        let mut buffer = Buffer::new(Format::F32, 1, [255; 4]).unwrap();
        for _ in 0..20_000 {
            buffer.path(0, &[1], [10.6 / 255.0; 4], 0.5).unwrap();
        }
        assert_eq!(value(&buffer), 11);
        let mut buffer = Buffer::new(Format::F32, 1, [255; 4]).unwrap();
        for _ in 0..270_000 {
            buffer
                .path(0, &[255], [0.0; 4], 3.0 / 134_217_728.0)
                .unwrap();
        }
        // Exact closed form is 253.4657168936922. Forced F32 violates
        // the bound; the planner must keep this stack on F64.
        assert_eq!(value(&buffer), 255);
    }
    #[test]
    fn word_embedding_and_background_rounding_are_exact() {
        for w in 0..=65_535_u32 {
            let byte = (w + 128) / 257;
            let residual = i64::from(w) - i64::from(byte) * 257;
            assert!((-128..=128).contains(&residual));
            assert_eq!(i64::from(byte) * 257 + residual, i64::from(w));
        }
        for c in 0..=255_u32 {
            for a in 0..=255_u32 {
                let w = (c * a * 257 + 127) / 255;
                assert_eq!((w + 128) / 257, (c * a + 127) / 255);
            }
        }
    }
    #[test]
    fn source_encoding_preserves_byte_rounding_at_half_levels() {
        for format in [Format::N16, Format::F32] {
            for (source, expected) in [
                (0.5_f64.next_down(), 127),
                (0.5, 128),
                (0.5_f64.next_up(), 128),
            ] {
                let mut buffer = Buffer::new(format, 1, [255; 4]).unwrap();
                buffer.path(0, &[255], [source; 4], 1.0).unwrap();
                assert_eq!(value(&buffer), expected);
            }
            for byte in 0..=255 {
                let mut buffer = Buffer::new(format, 1, [byte; 4]).unwrap();
                assert_eq!(value(&buffer), byte);
                buffer
                    .path(0, &[255], [f64::from(byte) / 255.0; 4], 1.0)
                    .unwrap();
                assert_eq!(value(&buffer), byte);
            }
        }
    }
    #[test]
    fn zero_coverage_is_a_noop_and_full_coverage_overwrites() {
        for format in [Format::N16, Format::F32] {
            let mut buffer = Buffer::new(format, 1, [42; 4]).unwrap();
            buffer.path(0, &[0], [0.0; 4], 1.0).unwrap();
            assert_eq!(value(&buffer), 42);
            buffer.path(0, &[255], [1.0; 4], 1.0).unwrap();
            assert_eq!(value(&buffer), 255);
        }
    }
    #[test]
    fn full_runs_store_what_single_samples_store() {
        let backdrop = [200, 100, 50, 255];
        for format in [Format::N16, Format::F32] {
            for alpha in [1.0, 0.5, 1.0 - 1.0 / 1024.0] {
                let mut run = Buffer::new(format, 5, backdrop).unwrap();
                let mut single = Buffer::new(format, 5, backdrop).unwrap();
                let color = [0.3, 0.6, 0.9, 1.0];
                run.path(1, &[255; 3], color, alpha).unwrap();
                for offset in 1..4 {
                    single.path(offset, &[255], color, alpha).unwrap();
                }
                let (mut a, mut b) = ([0; 20], [0; 20]);
                run.output(0, &mut a);
                single.output(0, &mut b);
                assert_eq!(a, b, "{format:?} {alpha}");
                assert_eq!(a[..4], backdrop);
                assert_eq!(a[16..], backdrop);
                if alpha == 1.0 {
                    // An opaque source replaces the backdrop: 76.5, 153, 229.5.
                    assert_eq!(a[4..8], [77, 153, 230, 255]);
                } else if format == Format::F32 && alpha == 0.5 {
                    // 200 + (76.5 − 200) / 2 = 138.25, then 126.5 and 139.75.
                    assert_eq!(a[4..8], [138, 127, 140, 255]);
                }
            }
        }
    }
    #[test]
    fn dynamic_store_and_memory_use_the_selected_format() {
        let n = Buffer::new(Format::N16, 3, [255; 4]).unwrap();
        assert_eq!(n.bytes(), 24);
        let mut f = Buffer::new(Format::F32, 3, [255; 4]).unwrap();
        assert_eq!(f.bytes(), 48);
        f.prepare_dynamic();
        f.paint_dynamic(0, Source::new([0.0; 4], 0.5, 1.0)).unwrap();
        let mut out = [0; 4];
        f.output(0, &mut out);
        assert_eq!(out, [128, 128, 128, 255]);
        assert!(Buffer::new(Format::F64, 1, [0; 4]).is_err());
    }
    #[test]
    fn sampled_and_binade_edge_stores_respect_the_error_envelope() {
        let mut seed = 7_u64;
        let mut random = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            u32::try_from(seed >> 32).unwrap()
        };
        for i in 0..100_000 {
            let x = if i % 3 == 0 {
                f32::from_bits(255.0_f32.to_bits() - (random() % 64))
            } else {
                level(f64::from(random()) / f64::from(u32::MAX))
            };
            let source = f64::from(random()) / f64::from(u32::MAX);
            let alpha = f64::from(random()) / f64::from(u32::MAX);
            let c = u8::try_from(random() % 256).unwrap();
            let a = f64::from(c) * alpha / 255.0;
            let mut pixel = [x; 4];
            lerp(
                &mut pixel,
                [level(source); 4],
                f32::from(c) * scalar(alpha / 255.0),
            );
            let ideal = f64::from(x) + a * (255.0 * source - f64::from(x));
            let bound = 1.0 / 131_072.0 + a * (2.0 / 131_072.0 + 255.0 * 6.0 / 16_777_216.0);
            assert!((f64::from(pixel[0]) - ideal).abs() <= bound + 1e-12);
        }
    }
}
