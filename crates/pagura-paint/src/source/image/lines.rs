//! Separable area averages (8.9.5.3, ADR 0004) over source rows converted
//! once: a span converts each source row its footprints reach, over the
//! columns they reach, and keeps the rows for the other spans of its device
//! row and the device rows below, where [`Geometry::fill`] converts a sample
//! for every footprint that covers it.
//!
//! Each pixel then adds the same terms in the same order as
//! [`Geometry::fill`] does: per source line its cells, the first and the
//! last weighted, scaled by the line's column and row weights, the lines in
//! the order the placement reaches them. The values are the same.

use super::geometry::{Footprint, Geometry, Mode, Texels};
use std::sync::{Mutex, TryLockError};

/// Converted source rows kept for the next spans at least: those one
/// device row reads and the next row's.
const KEPT_ROWS: usize = 4;

/// The bytes of converted rows kept beyond [`KEPT_ROWS`].
const KEPT_BYTES: usize = 1 << 20;

/// The most values per texel that sum row by row; texels with more sum as
/// [`Geometry::fill`] sums them.
const MAX_VALUES: usize = 6;

/// Row-by-row separable sampling of one image's texels, with the rows kept
/// between spans.
#[derive(Debug, Default)]
pub(super) struct Lines {
    scratch: Mutex<Scratch>,
}

/// Storage reused between spans.
#[derive(Debug, Default)]
struct Scratch {
    /// Each pixel's footprint along the source columns.
    feet: Vec<Footprint>,
    /// Each pixel's alpha so far, for opaque texels.
    alphas: Vec<f64>,
    /// Each pixel's sum of cell weights along one line, for opaque texels.
    line_alphas: Vec<f64>,
    rows: Vec<Converted>,
    /// Counts row lookups, so that the least recently used row is replaced.
    clock: u64,
}

/// The texels of samples `first..end` of source row `row`, as many as one
/// span needs or more.
#[derive(Debug, Default)]
struct Converted {
    row: u32,
    first: u32,
    end: u32,
    used: u64,
    values: Vec<f64>,
}

impl Lines {
    /// Writes the values of device pixels `x .. x + len` of row `y`, placed
    /// by `geometry`, `texels.lanes()` each, into `out`, as
    /// [`Geometry::fill`] does.
    pub(super) fn fill(
        &self,
        geometry: &Geometry,
        (x, y, len): (u32, u32, usize),
        texels: &impl Texels,
        out: &mut [f64],
    ) {
        let values = texels
            .lanes()
            .saturating_sub(if texels.opaque() { 2 } else { 0 });
        if geometry.mode() != (Mode::Separable { swapped: false })
            || !(1..=MAX_VALUES).contains(&values)
        {
            geometry.fill((x, y, len), texels, out);
            return;
        }
        // A concurrent caller works in scratch of its own.
        let mut guard = match self.scratch.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        };
        let mut local = None;
        let scratch = match guard.as_deref_mut() {
            Some(scratch) => scratch,
            None => local.insert(Scratch::default()),
        };
        let span = (x, y, len);
        match values {
            1 => reduce::<1>(geometry, span, texels, scratch, out),
            2 => reduce::<2>(geometry, span, texels, scratch, out),
            3 => reduce::<3>(geometry, span, texels, scratch, out),
            4 => reduce::<4>(geometry, span, texels, scratch, out),
            5 => reduce::<5>(geometry, span, texels, scratch, out),
            _ => reduce::<6>(geometry, span, texels, scratch, out),
        }
    }
}

/// The separable area average of an upright placement whose texels have
/// `N` summed values, source row by source row.
fn reduce<const N: usize>(
    geometry: &Geometry,
    (x, y, len): (u32, u32, usize),
    texels: &impl Texels,
    scratch: &mut Scratch,
    out: &mut [f64],
) {
    let lanes = texels.lanes();
    let opaque = texels.opaque();
    let t = geometry.inverse();
    let (width, height) = geometry.size();
    let rows = Footprint::new(y, t.d, t.f, height);
    let descending = t.d < 0.0;
    let Scratch {
        feet,
        alphas,
        line_alphas,
        rows: kept,
        clock,
    } = scratch;
    feet.clear();
    feet.extend(
        (x..=u32::MAX)
            .take(len)
            .map(|px| Footprint::new(px, t.a, t.e, width)),
    );
    let (first, end) = feet
        .iter()
        .filter(|foot| foot.first < foot.end)
        .fold((u32::MAX, 0), |(first, end), foot| {
            (first.min(foot.first), end.max(foot.end))
        });
    let pixels = feet.len().min(out.len() / lanes.max(1));
    let out = &mut out[..pixels * lanes];
    out.fill(0.0);
    if opaque {
        alphas.clear();
        alphas.resize(pixels, 0.0);
        // An opaque line's alpha: its cells' weights, as its texels sum
        // them.
        line_alphas.clear();
        line_alphas.extend(feet.iter().map(|foot| {
            let mut sum = 0.0;
            for cell in foot.first..foot.end {
                sum += foot.weight(cell);
            }
            sum
        }));
    }
    // Every row of the footprint and the next device row's first stay.
    let count = usize::try_from(rows.end.saturating_sub(rows.first)).unwrap_or(usize::MAX);
    let row_bytes = usize::try_from(end.saturating_sub(first))
        .unwrap_or(usize::MAX)
        .saturating_mul(N * size_of::<f64>())
        .max(1);
    let capacity = count
        .saturating_add(2)
        .min(KEPT_BYTES / row_bytes)
        .max(KEPT_ROWS);
    for offset in rows.first..rows.end {
        let row = if descending {
            rows.end - 1 - (offset - rows.first)
        } else {
            offset
        };
        let weight = rows.weight(row);
        let values = if first < end {
            let kept = Kept {
                rows: &mut *kept,
                clock: &mut *clock,
                capacity,
            };
            converted(kept, texels, (row, width), (first, end), N)
        } else {
            &[]
        };
        for (i, (foot, pixel)) in feet.iter().zip(out.chunks_exact_mut(lanes)).enumerate() {
            let cells = usize::try_from(foot.end.saturating_sub(foot.first)).unwrap_or(0);
            let start = usize::try_from(foot.first.saturating_sub(first))
                .unwrap_or(0)
                .saturating_mul(N);
            let line = line::<N>(
                values
                    .get(start..start.saturating_add(cells.saturating_mul(N)))
                    .unwrap_or(&[]),
                foot.first_weight,
                foot.last_weight,
            );
            for (sum, value) in pixel.iter_mut().zip(line) {
                *sum += value * foot.normalization * weight;
            }
            if let (Some(alpha), Some(&line_alpha)) = (alphas.get_mut(i), line_alphas.get(i))
                && opaque
            {
                *alpha += line_alpha * foot.normalization * weight;
            }
        }
    }
    for (i, pixel) in out.chunks_exact_mut(lanes).enumerate() {
        if opaque && let (Some(rest), Some(&alpha)) = (pixel.get_mut(N..), alphas.get(i)) {
            rest.fill(alpha);
        }
        for value in pixel.iter_mut() {
            *value *= rows.normalization;
        }
    }
}

/// The sum of one line's texels, `N` values per cell: the first cell
/// weighted by `first`, the last by `last`, those between added in order,
/// starting from zero, as [`Geometry::fill`] adds them.
#[inline]
fn line<const N: usize>(values: &[f64], first: f64, last: f64) -> [f64; N] {
    let mut sum = [0.0; N];
    let Some((a, rest)) = values.as_chunks::<N>().0.split_first() else {
        return sum;
    };
    for (sum, &a) in sum.iter_mut().zip(a) {
        *sum += a * first;
    }
    if let Some((z, between)) = rest.split_last() {
        for cell in between {
            for (sum, &value) in sum.iter_mut().zip(cell) {
                *sum += value;
            }
        }
        for (sum, &z) in sum.iter_mut().zip(z) {
            *sum += z * last;
        }
    }
    sum
}

/// The converted rows of a span: kept rows, a clock to replace the least
/// recently used, and how many to keep.
struct Kept<'s> {
    rows: &'s mut Vec<Converted>,
    clock: &'s mut u64,
    capacity: usize,
}

/// The texels of samples `first..end` of source row `row`, `n` values
/// each: the row's kept texels, extended over the samples it lacks, or the
/// least recently used row's storage converted anew.
fn converted<'s>(
    kept: Kept<'s>,
    texels: &impl Texels,
    (row, width): (u32, u32),
    (first, end): (u32, u32),
    n: usize,
) -> &'s [f64] {
    let Kept {
        rows,
        clock,
        capacity,
    } = kept;
    *clock = clock.wrapping_add(1);
    let found = rows
        .iter()
        .position(|kept| kept.row == row && kept.first < kept.end);
    let index = if let Some(index) = found {
        if let Some(entry) = rows.get_mut(index) {
            extend(entry, texels, width, (first, end), n);
        }
        index
    } else {
        let index = if rows.len() < capacity {
            rows.push(Converted::default());
            rows.len() - 1
        } else {
            rows.iter()
                .enumerate()
                .min_by_key(|(_, kept)| kept.used)
                .map_or(0, |(index, _)| index)
        };
        if let Some(entry) = rows.get_mut(index) {
            let count = usize::try_from(end - first).unwrap_or(0);
            entry.values.clear();
            entry.values.resize(count.saturating_mul(n), 0.0);
            texels.row((row, width), (first, end), &mut entry.values);
            (entry.row, entry.first, entry.end) = (row, first, end);
        }
        index
    };
    let Some(entry) = rows.get_mut(index) else {
        return &[];
    };
    entry.used = *clock;
    let start = usize::try_from(first.saturating_sub(entry.first))
        .unwrap_or(0)
        .saturating_mul(n);
    entry.values.get(start..).unwrap_or(&[])
}

/// Extends `entry` over samples `first..end` of its row, converting the
/// samples it lacks.
fn extend(
    entry: &mut Converted,
    texels: &impl Texels,
    width: u32,
    (first, end): (u32, u32),
    n: usize,
) {
    if entry.first <= first && end <= entry.end {
        return;
    }
    let (low, high) = (entry.first.min(first), entry.end.max(end));
    let held = entry.values.len();
    let left = usize::try_from(entry.first - low)
        .unwrap_or(0)
        .saturating_mul(n);
    let total = usize::try_from(high - low).unwrap_or(0).saturating_mul(n);
    entry.values.resize(total, 0.0);
    if left > 0 {
        entry.values.copy_within(0..held, left);
        if let Some(out) = entry.values.get_mut(..left) {
            texels.row((entry.row, width), (low, entry.first), out);
        }
    }
    if entry.end < high
        && let Some(out) = entry.values.get_mut(left.saturating_add(held)..)
    {
        texels.row((entry.row, width), (entry.end, high), out);
    }
    (entry.first, entry.end) = (low, high);
}
