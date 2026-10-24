//! The sampling engine of upright placements (8.9.5.3, ADR 0004); quarter
//! turns are sampled upright from transposed samples. A device pixel's
//! footprint is a column footprint times a row footprint: the exact area
//! of its preimage when minifying, the two bilinear neighbours of its
//! centre when interpolating, the sample under its centre otherwise.
//!
//! A device row first reduces the source rows of its row footprint for
//! every source column its pixels reach: the first row weighted, plus the
//! sum of the rows between, plus the last row weighted. Each pixel then
//! adds its columns' lines left to right, the first and last weighted, and
//! scales the sum by both footprints' normalizations. Eight-bit codes sum
//! as exact integers, and where every weight is whole they stay integers
//! until that scaling. Source rows come from a [`Rows`] source, as codes or
//! as converted texels, and are kept for the spans and device rows that
//! read them again.

use super::geometry::{Footprint, Geometry, Mode};
use super::scratch::{allocate, grown};
use crate::source::Pixels;
use pagura_core::{Meter, Result};

/// Rows kept at least: those of a few device rows' footprints.
const KEPT_ROWS: usize = 8;

/// The bytes of rows kept beyond [`KEPT_ROWS`].
const KEPT_BYTES: usize = 1 << 20;

/// How a footprint weights its samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Taps {
    /// The sample under the pixel centre.
    Nearest,
    /// The two samples around the pixel centre.
    Bilinear,
    /// The exact area of the pixel's preimage.
    Area,
}

/// One value of a source row: an eight-bit code or a texel value.
pub(super) trait Value: Copy + Default + std::fmt::Debug {
    /// A sum of values: an exact integer for codes.
    type Sum: Copy + Default + std::fmt::Debug;
    /// Whether sums are exact, so that whole footprints may add every row
    /// and cell before scaling.
    const EXACT: bool;
    /// The value as f64.
    fn widen(self) -> f64;
    /// The sum of this value alone.
    fn alone(self) -> Self::Sum;
    /// `sum` plus this value.
    fn add(self, sum: Self::Sum) -> Self::Sum;
    /// The sum of two sums.
    fn merge(a: Self::Sum, b: Self::Sum) -> Self::Sum;
    /// A sum as f64.
    fn total(sum: Self::Sum) -> f64;
    /// This value type's storage of the two.
    fn pick<'s>(codes: &'s mut Buffers<u8>, texels: &'s mut Buffers<f64>) -> &'s mut Buffers<Self>;
}

impl Value for u8 {
    type Sum = u32;
    const EXACT: bool = true;

    #[inline]
    fn widen(self) -> f64 {
        f64::from(self)
    }

    #[inline]
    fn alone(self) -> u32 {
        u32::from(self)
    }

    /// Wrapping: sums of fewer than 2^24 codes stay below 2^32.
    #[inline]
    fn add(self, sum: u32) -> u32 {
        sum.wrapping_add(u32::from(self))
    }

    #[inline]
    fn merge(a: u32, b: u32) -> u32 {
        a.wrapping_add(b)
    }

    #[inline]
    fn total(sum: u32) -> f64 {
        f64::from(sum)
    }

    fn pick<'s>(codes: &'s mut Buffers<u8>, _: &'s mut Buffers<f64>) -> &'s mut Buffers<Self> {
        codes
    }
}

impl Value for f64 {
    type Sum = f64;
    const EXACT: bool = false;

    #[inline]
    fn widen(self) -> f64 {
        self
    }

    #[inline]
    fn alone(self) -> f64 {
        self
    }

    #[inline]
    fn add(self, sum: f64) -> f64 {
        sum + self
    }

    #[inline]
    fn merge(a: f64, b: f64) -> f64 {
        a + b
    }

    #[inline]
    fn total(sum: f64) -> f64 {
        sum
    }

    fn pick<'s>(_: &'s mut Buffers<u8>, texels: &'s mut Buffers<f64>) -> &'s mut Buffers<Self> {
        texels
    }
}

/// A source of rows of values, [`Rows::width`] per sample.
pub(super) trait Rows {
    /// The values.
    type Value: Value;

    /// Values per sample.
    fn width(&self) -> usize;

    /// The values of samples `first..end` of source row `y`, through
    /// `cache`; empty when the row cannot be read, which reads as zeros.
    fn row<'c>(
        &'c self,
        y: usize,
        columns: (usize, usize),
        cache: &'c mut RowCache<Self::Value>,
    ) -> &'c [Self::Value];

    /// Where the source knows its background: the background's values,
    /// with `runs` of source columns `first..end` whose samples hold it in
    /// every row `top..=bottom` (`false`) or may not (`true`).
    fn background(
        &self,
        rows: (usize, usize),
        columns: (usize, usize),
        runs: &mut Runs,
    ) -> Option<&[Self::Value]> {
        let _ = (rows, columns, runs);
        None
    }
}

/// Runs of background (`false`) and other samples of a row.
pub(super) type Runs = Vec<(usize, usize, bool)>;

/// Source rows kept between spans, by row, over the samples they hold.
#[derive(Debug)]
pub(super) struct RowCache<V> {
    entries: Vec<Kept<V>>,
    /// The entry replaced next.
    next: usize,
    /// Scratch of the rows being filled.
    runs: Runs,
}

impl<V> Default for RowCache<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            next: 0,
            runs: Vec::new(),
        }
    }
}

/// One kept row: values of samples `first..end` of row `row`.
#[derive(Debug)]
struct Kept<V> {
    row: usize,
    first: usize,
    end: usize,
    values: Vec<V>,
}

impl<V: Copy + Default> RowCache<V> {
    /// The `n` values per sample of samples `first..end` of row `y`: from a
    /// kept row that holds them, else filled by `fill` (which returns
    /// whether it could) into a new or the next replaced entry.
    pub(super) fn get(
        &mut self,
        (y, (first, end)): (usize, (usize, usize)),
        n: usize,
        fill: impl FnOnce(&mut [V], &mut Runs) -> bool,
    ) -> &[V] {
        let found = self
            .entries
            .iter()
            .position(|kept| kept.row == y && kept.first <= first && end <= kept.end);
        let index = found.unwrap_or_else(|| {
            let index = self.next % self.entries.len().max(1);
            self.next = index + 1;
            if let Some(kept) = self.entries.get_mut(index) {
                // Filling writes every value.
                if fill(grown(&mut kept.values, (end - first) * n), &mut self.runs) {
                    (kept.row, kept.first, kept.end) = (y, first, end);
                } else {
                    // A row that cannot be read is not kept.
                    kept.values.clear();
                    (kept.row, kept.first, kept.end) = (usize::MAX, 0, 0);
                }
            }
            index
        });
        self.entries.get(index).map_or(&[], |kept| {
            let start = (first - kept.first) * n;
            kept.values
                .get(start..start + (end - first) * n)
                .unwrap_or(&[])
        })
    }
}

/// Storage for one value type.
#[derive(Debug)]
pub(super) struct Buffers<V: Value> {
    cache: RowCache<V>,
    /// Per source column, the sum of the rows between a row footprint's
    /// ends, or of all its rows where every weight is whole.
    sums: Vec<V::Sum>,
    /// The values of nearest samples.
    gathered: Vec<V>,
}

impl<V: Value> Default for Buffers<V> {
    fn default() -> Self {
        Self {
            cache: RowCache::default(),
            sums: Vec::new(),
            gathered: Vec::new(),
        }
    }
}

/// Storage reused between spans.
#[derive(Debug, Default)]
pub(super) struct Scratch {
    /// Per source column, its row footprint's weighted sum.
    lines: Vec<f64>,
    /// A span's averages and which pixels its footprints cover.
    averages: Vec<f64>,
    covered: Vec<bool>,
    /// Runs of background and other source columns.
    runs: Runs,
    codes: Buffers<u8>,
    texels: Buffers<f64>,
}

/// The representation sampled by a separable plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Values {
    Codes,
    Texels,
}

/// Capacities of the one value type a separable plan samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Layout {
    columns: usize,
    pixels: usize,
    values: usize,
    rows: usize,
    kind: Values,
}

impl Layout {
    /// Bytes of all allocations, including the boxed scratch value.
    pub(super) fn bytes(self) -> usize {
        let source = self.columns.saturating_mul(self.values);
        let output = self.pixels.saturating_mul(self.values);
        let sample = if self.kind == Values::Codes {
            size_of::<u8>()
        } else {
            size_of::<f64>()
        };
        let sum = if self.kind == Values::Codes {
            size_of::<u32>()
        } else {
            size_of::<f64>()
        };
        size_of::<Scratch>()
            .saturating_add(self.rows.saturating_mul(size_of::<Kept<u8>>()))
            .saturating_add(self.rows.saturating_mul(source).saturating_mul(sample))
            .saturating_add(source.saturating_mul(sum + size_of::<f64>()))
            .saturating_add(output.saturating_mul(sample + size_of::<f64>()))
            .saturating_add(self.pixels)
            .saturating_add(
                self.columns
                    .saturating_add(1)
                    .saturating_mul(2 * size_of::<(usize, usize, bool)>()),
            )
    }

    /// Allocates the capacities already charged by the source layout.
    pub(super) fn make(self) -> Result<Scratch> {
        let source = self.columns.saturating_mul(self.values);
        let output = self.pixels.saturating_mul(self.values);
        let mut scratch = Scratch {
            lines: allocate(source)?,
            averages: allocate(output)?,
            covered: allocate(self.pixels)?,
            runs: allocate(self.columns.saturating_add(1))?,
            codes: Buffers::default(),
            texels: Buffers::default(),
        };
        if self.kind == Values::Codes {
            scratch.codes = self.buffers(source, output)?;
        } else {
            scratch.texels = self.buffers(source, output)?;
        }
        Ok(scratch)
    }

    fn buffers<V: Value>(self, source: usize, output: usize) -> Result<Buffers<V>> {
        let mut entries = allocate(self.rows)?;
        for _ in 0..self.rows {
            entries.push(Kept {
                row: usize::MAX,
                first: 0,
                end: 0,
                values: allocate(source)?,
            });
        }
        Ok(Buffers {
            cache: RowCache {
                entries,
                next: 0,
                runs: allocate(self.columns.saturating_add(1))?,
            },
            sums: allocate(source)?,
            gathered: allocate(output)?,
        })
    }
}

impl Scratch {
    /// Actual capacities retained, checked against the prepared layout.
    pub(super) fn bytes(&self) -> usize {
        fn buffers<V: Value>(values: &Buffers<V>) -> usize {
            values.cache.entries.capacity() * size_of::<Kept<V>>()
                + values
                    .cache
                    .entries
                    .iter()
                    .map(|row| row.values.capacity() * size_of::<V>())
                    .sum::<usize>()
                + values.cache.runs.capacity() * size_of::<(usize, usize, bool)>()
                + values.sums.capacity() * size_of::<V::Sum>()
                + values.gathered.capacity() * size_of::<V>()
        }
        size_of::<Self>()
            + (self.lines.capacity() + self.averages.capacity()) * size_of::<f64>()
            + self.covered.capacity()
            + self.runs.capacity() * size_of::<(usize, usize, bool)>()
            + buffers(&self.codes)
            + buffers(&self.texels)
    }

    /// Invalidates source-dependent row values between renders.
    pub(super) fn reset(&mut self) {
        fn clear<V>(entries: &mut [Kept<V>]) {
            for kept in entries {
                kept.row = usize::MAX;
            }
        }
        clear(&mut self.codes.cache.entries);
        clear(&mut self.texels.cache.entries);
    }
}

/// The separable placement of one image: each planned device column's
/// footprint, the row footprints on demand, and scratch.
#[derive(Debug)]
pub(super) struct Separable {
    geometry: Geometry,
    taps: Taps,
    /// The first device column of the plan.
    x0: u32,
    /// Per planned device column, its footprint, or for nearest samples
    /// its sample's column in `cells`.
    feet: Vec<Footprint>,
    cells: Vec<u32>,
    /// Whether every planned footprint weights its samples whole.
    whole: bool,
    /// Whether the planned nearest samples step by one sample.
    consecutive: bool,
    /// Whether the planned footprints that meet the image step one way.
    monotone: bool,
}

impl Separable {
    /// The storage needed by this plan, for `values` per sample, as codes
    /// or converted texels. Every span stays inside the planned columns.
    pub(super) fn layout(&self, values: usize, kind: Values) -> Layout {
        let (first, end) = if self.nearest() {
            self.cells
                .iter()
                .fold((u32::MAX, 0), |(first, end), &cell| {
                    (first.min(cell), end.max(cell.saturating_add(1)))
                })
        } else {
            let (first, end) = extent(&self.feet).unwrap_or((0, 0));
            (
                u32::try_from(first).unwrap_or(0),
                u32::try_from(end).unwrap_or(0),
            )
        };
        let columns = index(end.saturating_sub(first));
        let value_bytes = if kind == Values::Codes {
            size_of::<u8>()
        } else {
            size_of::<f64>()
        };
        let row_bytes = columns.saturating_mul(values).saturating_mul(value_bytes);
        let height = index(self.geometry.size().1).max(1);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "finite non-negative sample extent, capped by the decoded image height"
        )]
        let needed = self
            .geometry
            .inverse()
            .d
            .abs()
            .ceil()
            .min(f64::from(self.geometry.size().1)) as usize;
        let rows = needed
            .saturating_add(4)
            .min(KEPT_BYTES / row_bytes.max(1))
            .max(KEPT_ROWS)
            .min(height);
        Layout {
            columns,
            pixels: self.feet.len().max(self.cells.len()),
            values,
            rows,
            kind,
        }
    }

    /// The separable placement of `geometry` for device columns `x0..x1`,
    /// or `None` when it is not upright.
    ///
    /// # Errors
    ///
    /// A `Limit` error when the plan cannot be allocated.
    pub(super) fn new(
        geometry: &Geometry,
        (x0, x1): (u32, u32),
        bytes: &mut Meter,
    ) -> Result<Option<Self>> {
        let t = geometry.inverse();
        if t.b != 0.0 || t.c != 0.0 {
            return Ok(None);
        }
        let taps = match geometry.mode() {
            Mode::Nearest => Taps::Nearest,
            Mode::Bilinear => Taps::Bilinear,
            Mode::Area => Taps::Area,
        };
        let count = index(x1.saturating_sub(x0));
        let (mut feet, mut cells) = (Vec::new(), Vec::new());
        if taps == Taps::Nearest {
            bytes.reserve(&mut cells, count)?;
            cells.extend((x0..x1).map(|x| column(geometry, taps, x).first));
        } else {
            bytes.reserve(&mut feet, count)?;
            feet.extend((x0..x1).map(|x| column(geometry, taps, x)));
        }
        Ok(Some(Self {
            geometry: geometry.clone(),
            taps,
            x0,
            whole: whole(&feet),
            consecutive: consecutive(&cells),
            monotone: monotone(&feet),
            feet,
            cells,
        }))
    }

    /// Bytes the plan holds.
    pub(super) fn bytes(&self) -> usize {
        self.feet.capacity() * size_of::<Footprint>() + self.cells.capacity() * size_of::<u32>()
    }

    /// Whether pixels take the sample under their centre.
    pub(super) fn nearest(&self) -> bool {
        self.taps == Taps::Nearest
    }

    /// Hands `finish` each pixel's sample of device pixels `x .. x + len`
    /// of row `y`, [`Rows::width`] values each (zeros where the row cannot
    /// be read), for a placement whose pixels take their nearest sample.
    pub(super) fn gather<R: Rows>(
        &self,
        rows: &R,
        pixels: Pixels<'_>,
        scratch: &mut Scratch,
        finish: impl FnOnce(&[R::Value]),
    ) {
        let Pixels { y, len, .. } = pixels;
        {
            let Scratch { codes, texels, .. } = scratch;
            let (cells, consecutive) = self.cells(pixels);
            let Buffers {
                cache, gathered, ..
            } = R::Value::pick(codes, texels);
            let n = rows.width();
            let across = row(&self.geometry, self.taps, y);
            gathered.clear();
            let (low, high) = cells.iter().fold((u32::MAX, 0), |(low, high), &cell| {
                (low.min(cell), high.max(cell))
            });
            if low <= high && across.first < across.end {
                let (first, end) = (index(low), index(high) + 1);
                let values = rows.row(index(across.first), (first, end), cache);
                if consecutive && let Some(values) = values.get(..len * n) {
                    // One sample per pixel in order: the row itself.
                    finish(values);
                    return;
                }
                gathered.resize(len * n, R::Value::default());
                let cells = (values, cells, first);
                match n {
                    1 => pick::<1, R::Value>(cells, gathered),
                    2 => pick::<2, R::Value>(cells, gathered),
                    3 => pick::<3, R::Value>(cells, gathered),
                    4 => pick::<4, R::Value>(cells, gathered),
                    _ => {
                        let (values, cells, first) = cells;
                        for (out, &cell) in gathered.chunks_exact_mut(n.max(1)).zip(cells) {
                            let start = (index(cell) - first) * n;
                            if let Some(values) = values.get(start..start + n) {
                                out.copy_from_slice(values);
                            }
                        }
                    }
                }
            }
            gathered.resize(len * n, R::Value::default());
            finish(gathered);
        }
    }

    /// Writes `map` of each average of device pixels `x .. x + len` of row
    /// `y`, [`Rows::width`] values each, into the first values of each of
    /// `len` pixels of `stride` values of `out`, or with `None` into
    /// scratch, and hands `finish` the values written and for each pixel
    /// whether its footprint meets the image; the other pixels' values are
    /// zeros.
    pub(super) fn average<R: Rows>(
        &self,
        rows: &R,
        pixels: Pixels<'_>,
        scratch: &mut Scratch,
        out: Option<(&mut [f64], usize)>,
        (map, finish): (impl Fn(f64) -> f64 + Copy, impl FnOnce(&mut [f64], &[bool])),
    ) {
        let Pixels { y, len, .. } = pixels;
        {
            let Scratch {
                lines,
                averages,
                covered,
                runs,
                codes,
                texels,
                ..
            } = scratch;
            let n = rows.width();
            let (values, stride) = if let Some((values, stride)) = out {
                (values, stride.max(n))
            } else {
                (grown(averages, len * n), n)
            };
            covered.clear();
            covered.resize(len, false);
            let feet = self.feet(pixels);
            let whole = self.whole;
            let across = row(&self.geometry, self.taps, y);
            if let Some((first, end)) = self.extent(feet)
                && across.first < across.end
                && n > 0
            {
                let Buffers { cache, sums, .. } = R::Value::pick(codes, texels);
                let sums = grown(sums, (end - first) * n);
                let span = Span {
                    feet,
                    first,
                    scale: across.normalization,
                    n,
                    stride,
                };
                let out = (&mut *values, covered.as_mut_slice());
                if R::Value::EXACT && whole && across.whole() {
                    // Whole samples sum as integers throughout.
                    let work = Work {
                        cache,
                        sums: &mut *sums,
                        runs,
                    };
                    sum_rows(rows, &across, (first, end), work);
                    span.columns::<R::Value>(Reduced::Sums(sums), out, map);
                } else {
                    let lines = grown(lines, (end - first) * n);
                    let work = Work { cache, sums, runs };
                    reduce_rows(rows, &across, (first, end), work, &mut *lines);
                    span.columns::<R::Value>(Reduced::Lines(lines), out, map);
                }
            }
            // Only covered pixels were written.
            for (pixel, _) in values
                .chunks_exact_mut(stride.max(1))
                .zip(covered.iter())
                .filter(|(_, covered)| !**covered)
            {
                if let Some(values) = pixel.get_mut(..n) {
                    values.fill(0.0);
                }
            }
            finish(values, covered);
        }
    }

    /// The samples `first..end` that `feet`, planned footprints, span: on
    /// a monotone plan those of the outermost footprints that meet the
    /// image, without visiting the others.
    fn extent(&self, feet: &[Footprint]) -> Option<(usize, usize)> {
        if !self.monotone {
            return extent(feet);
        }
        let mut met = feet.iter().filter(|foot| foot.first < foot.end);
        let a = met.next()?;
        let b = met.next_back().unwrap_or(a);
        Some((index(a.first.min(b.first)), index(a.end.max(b.end))))
    }

    /// The planned footprints of `pixels`.
    fn feet(&self, pixels: Pixels<'_>) -> &[Footprint] {
        self.planned(pixels.x, pixels.len)
            .and_then(|range| self.feet.get(range))
            .unwrap_or(&[])
    }

    /// The planned nearest samples of `pixels`.
    fn cells(&self, pixels: Pixels<'_>) -> (&[u32], bool) {
        let cells = self
            .planned(pixels.x, pixels.len)
            .and_then(|range| self.cells.get(range))
            .unwrap_or(&[]);
        (cells, self.consecutive)
    }

    /// The plan's entries for device columns `x .. x + len`, or `None` when
    /// they reach outside it.
    fn planned(&self, x: u32, len: usize) -> Option<std::ops::Range<usize>> {
        let start = index(x.checked_sub(self.x0)?);
        let end = start.checked_add(len)?;
        (end <= self.feet.len().max(self.cells.len())).then_some(start..end)
    }
}

/// The pixels of one span over the source columns from `first`, `n` values
/// per column and pixel (`stride` apart in the output), and its row
/// footprint's normalization.
struct Span<'a> {
    feet: &'a [Footprint],
    first: usize,
    scale: f64,
    n: usize,
    stride: usize,
}

impl Span<'_> {
    /// Writes each covered pixel's values: `map` of its footprint's sum of
    /// the columns' `reduced` values scaled by both normalizations; values
    /// in groups of up to six, each group's width a constant.
    fn columns<V: Value>(
        &self,
        reduced: Reduced<'_, V::Sum>,
        out: (&mut [f64], &mut [bool]),
        map: impl Fn(f64) -> f64 + Copy,
    ) {
        let (averages, covered) = out;
        let mut offset = 0;
        while offset < self.n {
            let width = (self.n - offset).min(6);
            let out = (&mut *averages, &mut *covered);
            match width {
                1 => self.group::<1, V>(offset, reduced, out, map),
                2 => self.group::<2, V>(offset, reduced, out, map),
                3 => self.group::<3, V>(offset, reduced, out, map),
                4 => self.group::<4, V>(offset, reduced, out, map),
                5 => self.group::<5, V>(offset, reduced, out, map),
                _ => self.group::<6, V>(offset, reduced, out, map),
            }
            offset += width;
        }
    }

    /// [`Self::columns`] for values `offset .. offset + N`: whole cells
    /// where a pixel's values are one group.
    #[inline]
    fn group<const N: usize, V: Value>(
        &self,
        offset: usize,
        reduced: Reduced<'_, V::Sum>,
        out: (&mut [f64], &mut [bool]),
        map: impl Fn(f64) -> f64 + Copy,
    ) {
        let n = self.n;
        let at = (offset, out, map);
        match (reduced, n == N) {
            (Reduced::Sums(sums), true) => self.pixels::<N, _>(sums, at, |cells, _| {
                integers::<N, V>(cells.as_chunks::<N>().0.iter().copied())
            }),
            (Reduced::Sums(sums), false) => self.pixels::<N, _>(sums, at, |cells, _| {
                integers::<N, V>(strided(cells, n, offset))
            }),
            (Reduced::Lines(lines), true) => self.pixels::<N, _>(lines, at, |cells, foot| {
                line::<N>(cells.as_chunks::<N>().0, foot)
            }),
            (Reduced::Lines(lines), false) => self.pixels::<N, _>(lines, at, |cells, foot| {
                weighted::<N>(strided(cells, n, offset), foot)
            }),
        }
    }

    /// Writes values `offset .. offset + N` of each covered pixel: `map` of
    /// `sum` of its footprint's cells of `reduced`, scaled by both
    /// normalizations.
    #[expect(
        clippy::inline_always,
        reason = "called per pixel; a call per pixel costs a quarter of the span"
    )]
    #[inline(always)]
    fn pixels<const N: usize, T>(
        &self,
        reduced: &[T],
        (offset, (averages, covered), map): (usize, (&mut [f64], &mut [bool]), impl Fn(f64) -> f64),
        sum: impl Fn(&[T], &Footprint) -> [f64; N],
    ) {
        let n = self.n;
        let pixels = averages.chunks_exact_mut(self.stride.max(1));
        for ((foot, pixel), covered) in self.feet.iter().zip(pixels).zip(covered.iter_mut()) {
            if foot.first >= foot.end {
                continue;
            }
            let cells = (index(foot.first) - self.first) * n..(index(foot.end) - self.first) * n;
            let (Some(cells), Some(values)) = (
                reduced.get(cells),
                pixel
                    .get_mut(offset..)
                    .and_then(<[f64]>::first_chunk_mut::<N>),
            ) else {
                continue;
            };
            *covered = true;
            let factor = foot.normalization * self.scale;
            for (value, sum) in values.iter_mut().zip(sum(cells, foot)) {
                *value = map(sum * factor);
            }
        }
    }
}

/// The source columns' reductions of a row footprint: sums of every row
/// where all weights are whole, else weighted lines.
#[derive(Clone, Copy)]
enum Reduced<'a, S> {
    Sums(&'a [S]),
    Lines(&'a [f64]),
}

/// Values `offset .. offset + N` of each cell of `cells`, `n` values a
/// cell.
#[inline]
fn strided<const N: usize, T: Copy + Default>(
    cells: &[T],
    n: usize,
    offset: usize,
) -> impl DoubleEndedIterator<Item = [T; N]> + '_ {
    cells.chunks_exact(n.max(1)).map(move |cell| {
        cell.get(offset..)
            .and_then(<[T]>::first_chunk::<N>)
            .copied()
            .unwrap_or([T::default(); N])
    })
}

/// The sum of `cells`: the first weighted by the footprint's first weight,
/// those between added, the last weighted by its last weight, left to
/// right.
#[expect(
    clippy::inline_always,
    reason = "called per pixel; a call per pixel costs a quarter of the span"
)]
#[inline(always)]
fn weighted<const N: usize>(
    mut cells: impl DoubleEndedIterator<Item = [f64; N]>,
    foot: &Footprint,
) -> [f64; N] {
    let Some(a) = cells.next() else {
        return [0.0; N];
    };
    let mut sum = a.map(|a| foot.first_weight * a);
    let z = cells.next_back();
    for cell in cells {
        for (sum, value) in sum.iter_mut().zip(cell) {
            *sum += value;
        }
    }
    if let Some(z) = z {
        for (sum, z) in sum.iter_mut().zip(z) {
            *sum += foot.last_weight * z;
        }
    }
    sum
}

/// [`weighted`] of whole cells, by their count.
#[expect(
    clippy::inline_always,
    reason = "called per pixel; a call per pixel costs a quarter of the span"
)]
#[inline(always)]
fn line<const N: usize>(cells: &[[f64; N]], foot: &Footprint) -> [f64; N] {
    let (first, last) = (foot.first_weight, foot.last_weight);
    match cells {
        [] => [0.0; N],
        [a] => a.map(|a| first * a),
        [a, b] => {
            let mut sum = [0.0; N];
            for ((sum, &a), &b) in sum.iter_mut().zip(a).zip(b) {
                *sum = first * a + last * b;
            }
            sum
        }
        [a, b, c] => {
            let mut sum = [0.0; N];
            for (((sum, &a), &b), &c) in sum.iter_mut().zip(a).zip(b).zip(c) {
                *sum = (first * a + b) + last * c;
            }
            sum
        }
        _ => weighted::<N>(cells.iter().copied(), foot),
    }
}

/// The sum of whole cells of exact sums.
#[expect(
    clippy::inline_always,
    reason = "called per pixel; a call per pixel costs a quarter of the span"
)]
#[inline(always)]
fn integers<const N: usize, V: Value>(cells: impl Iterator<Item = [V::Sum; N]>) -> [f64; N] {
    let mut total = [V::Sum::default(); N];
    for cell in cells {
        for (total, value) in total.iter_mut().zip(cell) {
            *total = V::merge(*total, value);
        }
    }
    total.map(V::total)
}

/// The storage reducing rows works in: kept rows, per source column sums,
/// and runs of background and other columns.
struct Work<'a, V: Value> {
    cache: &'a mut RowCache<V>,
    sums: &'a mut [V::Sum],
    runs: &'a mut Runs,
}

/// The sums of every row of the row footprint `across` (all weights whole)
/// for the source columns `first..end` into `sums`; the columns of
/// background runs take the sum of their constant samples.
fn sum_rows<R: Rows>(
    rows: &R,
    across: &Footprint,
    (first, end): (usize, usize),
    Work { cache, sums, runs }: Work<'_, R::Value>,
) {
    let n = rows.width();
    let length = (end - first) * n;
    // The first row, or a background run, assigns every sum.
    let (top, bottom) = (index(across.first), index(across.end) - 1);
    if let Some(background) = background(rows, (top, bottom), (first, end), runs) {
        let mut constant = [<R::Value as Value>::Sum::default(); super::geometry::MAX_LANES];
        for (sum, &value) in constant.iter_mut().zip(background) {
            *sum = (top..bottom).fold(value.alone(), |sum, _| value.add(sum));
        }
        fill(sums, (first, n), runs, &constant[..n]);
    }
    for y in top..=bottom {
        let values = rows.row(y, (first, end), cache);
        if values.len() != length {
            sums.fill(Default::default());
            return;
        }
        for range in dirty(runs, (first, n)) {
            let (Some(sums), Some(values)) = (sums.get_mut(range.clone()), values.get(range))
            else {
                continue;
            };
            if y == top {
                for (sum, &value) in sums.iter_mut().zip(values) {
                    *sum = value.alone();
                }
            } else {
                for (sum, &value) in sums.iter_mut().zip(values) {
                    *sum = value.add(*sum);
                }
            }
        }
    }
}

/// The weighted sums of the rows of the row footprint `across` for the
/// source columns `first..end` into `lines`: the first row weighted, plus
/// the sum of the rows between (in `between`), plus the last row weighted;
/// the columns of background runs take that of their constant samples.
fn reduce_rows<R: Rows>(
    rows: &R,
    across: &Footprint,
    (first, end): (usize, usize),
    Work {
        cache,
        sums: between,
        runs,
    }: Work<'_, R::Value>,
    lines: &mut [f64],
) {
    let n = rows.width();
    let length = (end - first) * n;
    // The first row, or a background run, assigns every line.
    let (top, bottom) = (index(across.first), index(across.end) - 1);
    let weights = (across.first_weight, across.last_weight);
    if let Some(background) = background(rows, (top, bottom), (first, end), runs) {
        // A column of background takes the steps every column takes.
        let mut constant = [0.0; super::geometry::MAX_LANES];
        let mut sum = [<R::Value as Value>::Sum::default(); super::geometry::MAX_LANES];
        for y in top..=bottom {
            step(
                (y, top, bottom),
                weights,
                (&mut constant[..n], &mut sum[..n]),
                background,
            );
        }
        fill(lines, (first, n), runs, &constant[..n]);
    }
    for y in top..=bottom {
        let values = rows.row(y, (first, end), cache);
        if values.len() != length {
            lines.fill(0.0);
            return;
        }
        for range in dirty(runs, (first, n)) {
            let (Some(lines), Some(between), Some(values)) = (
                lines.get_mut(range.clone()),
                between.get_mut(range.clone()),
                values.get(range),
            ) else {
                continue;
            };
            step((y, top, bottom), weights, (lines, between), values);
        }
    }
}

/// Row `y`'s step of reducing rows `top..=bottom` into `lines`: the first
/// row weighted by `first`, the rows between added into `between`, then
/// those sums and the last row weighted by `last`.
#[inline]
fn step<V: Value>(
    (y, top, bottom): (usize, usize, usize),
    (first, last): (f64, f64),
    (lines, between): (&mut [f64], &mut [V::Sum]),
    values: &[V],
) {
    if y == top {
        for (line, &value) in lines.iter_mut().zip(values) {
            *line = first * value.widen();
        }
    } else if y == bottom && bottom - top > 1 {
        for ((line, &sum), &value) in lines.iter_mut().zip(between.iter()).zip(values) {
            *line = (*line + V::total(sum)) + last * value.widen();
        }
    } else if y == bottom {
        for (line, &value) in lines.iter_mut().zip(values) {
            *line += last * value.widen();
        }
    } else if y == top + 1 {
        for (sum, &value) in between.iter_mut().zip(values) {
            *sum = value.alone();
        }
    } else {
        for (sum, &value) in between.iter_mut().zip(values) {
            *sum = value.add(*sum);
        }
    }
}

/// The samples of `rows` that hold their background in every row `top..=bottom`:
/// `runs` of source columns `first..end` that do (`false`) or may not
/// (`true`), and the background's values; `None`, with one run that may
/// not, where the source knows none.
fn background<'r, R: Rows>(
    rows: &'r R,
    (top, bottom): (usize, usize),
    (first, end): (usize, usize),
    runs: &mut Runs,
) -> Option<&'r [R::Value]> {
    runs.clear();
    let background = rows.background((top, bottom), (first, end), runs);
    if background.is_none() {
        runs.clear();
        runs.push((first, end, true));
    }
    background.filter(|values| values.len() == rows.width())
}

/// The value ranges of the runs of source columns that may not be
/// background, `n` values a column from source column `first`.
fn dirty(
    runs: &Runs,
    (first, n): (usize, usize),
) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    runs.iter()
        .filter(|run| run.2)
        .map(move |&(start, stop, _)| (start - first) * n..(stop - first) * n)
}

/// Sets every column of the background runs of `runs` to `constant`, `n`
/// values a column from source column `first`.
fn fill<T: Copy>(out: &mut [T], (first, n): (usize, usize), runs: &Runs, constant: &[T]) {
    for &(start, stop, _) in runs.iter().filter(|run| !run.2) {
        let range = (start - first) * n..(stop - first) * n;
        let Some(columns) = out.get_mut(range) else {
            continue;
        };
        match *constant {
            [a] => columns.fill(a),
            [a, b, c] => columns.as_chunks_mut::<3>().0.fill([a, b, c]),
            _ => {
                for column in columns.chunks_exact_mut(n.max(1)) {
                    column.copy_from_slice(constant);
                }
            }
        }
    }
}

/// Copies the sample of each column of `cells` of `values`, `N` values
/// each from source column `first`, into `out`.
#[inline]
fn pick<const N: usize, V: Copy>((values, cells, first): (&[V], &[u32], usize), out: &mut [V]) {
    let samples = values.as_chunks::<N>().0;
    for (out, &cell) in out.as_chunks_mut::<N>().0.iter_mut().zip(cells) {
        if let Some(sample) = samples.get(index(cell) - first) {
            *out = *sample;
        }
    }
}

/// The footprint of device column `x` along the source columns.
fn column(geometry: &Geometry, taps: Taps, x: u32) -> Footprint {
    let t = geometry.inverse();
    footprint(taps, x, (t.a, t.e, geometry.size().0))
}

/// The footprint of device row `y` across the source rows.
fn row(geometry: &Geometry, taps: Taps, y: u32) -> Footprint {
    let t = geometry.inverse();
    footprint(taps, y, (t.d, t.f, geometry.size().1))
}

fn footprint(taps: Taps, pixel: u32, (scale, offset, maximum): (f64, f64, u32)) -> Footprint {
    match taps {
        Taps::Nearest => Footprint::nearest(pixel, scale, offset, maximum),
        Taps::Bilinear => Footprint::bilinear(pixel, scale, offset, maximum),
        Taps::Area => Footprint::new(pixel, scale, offset, maximum).trimmed(),
    }
}

/// Whether every footprint weights its samples whole.
fn whole(feet: &[Footprint]) -> bool {
    feet.iter()
        .all(|foot| foot.first >= foot.end || foot.whole())
}

/// Whether the footprints that meet the image step one way along the row:
/// their first and end samples both never decrease, or both never
/// increase.
fn monotone(feet: &[Footprint]) -> bool {
    let mut met = feet.iter().filter(|foot| foot.first < foot.end);
    let Some(mut previous) = met.next() else {
        return true;
    };
    let (mut up, mut down) = (true, true);
    for foot in met {
        up &= previous.first <= foot.first && previous.end <= foot.end;
        down &= previous.first >= foot.first && previous.end >= foot.end;
        previous = foot;
    }
    up || down
}

/// Whether sample columns step by one.
fn consecutive(cells: &[u32]) -> bool {
    cells
        .windows(2)
        .all(|pair| pair[0].checked_add(1) == Some(pair[1]))
}

/// The samples `first..end` that footprints span.
fn extent(feet: &[Footprint]) -> Option<(usize, usize)> {
    let (first, end) = feet
        .iter()
        .filter(|foot| foot.first < foot.end)
        .fold((u32::MAX, 0), |(first, end), foot| {
            (first.min(foot.first), end.max(foot.end))
        });
    (first < end).then(|| (index(first), index(end)))
}

/// A sample coordinate as an index.
pub(super) fn index(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}
