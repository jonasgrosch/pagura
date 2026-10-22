//! A study of one image sampler with fewer passes, measured against
//! [`super::ImageSampler`] and not used by the renderer. It computes the
//! same definition (ADR 0004 §2):
//!
//! - upright placements (quarter turns from transposed samples): the
//!   separable footprints, each device row reducing the source rows of its
//!   row footprint into lines and each pixel adding its columns' lines, in
//!   the order and with the weights the separable engine uses; nearest
//!   samples looked up per code;
//! - other placements: the exact area average as the contour integral of
//!   row partial sums along the pixel's preimage (Green's theorem), which
//!   reads each sample under the footprint once and splits only the edges
//!   at the grid lines they cross, instead of clipping the preimage against
//!   every sample cell;
//! - texels of samples of several components converted once per distinct
//!   sample and kept in a memo, whole rows at a time;
//! - opaque pixels optionally written as N16 words directly.
//!
//! It covers what the measured pages need: colour images of samples of at
//! most eight bits (interleaved, one-bit packed or JPEG planes) with an
//! optional eight-bit soft mask, without colour keys, embedded opacity or
//! Matte.

#![expect(
    dead_code,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::chunks_exact_to_as_chunks,
    clippy::manual_clamp,
    clippy::manual_midpoint,
    clippy::many_single_char_names,
    clippy::single_match_else,
    clippy::struct_field_names,
    clippy::too_many_lines,
    reason = "a measured study, not product code"
)]

use super::geometry::{Footprint, Geometry, Mode};
use super::texels::{Colors, Opacity};
use crate::SpanColors;
use pagura_image::deferred::{Packed, Planar};
use pagura_image::{Decoded, Samples};
use pagura_raster::Point;
use std::sync::Arc;

/// One value of a source row: an eight-bit code or a texel lane.
pub(super) trait Value: Copy + Default {
    fn widen(self) -> f64;
}

impl Value for u8 {
    #[inline]
    fn widen(self) -> f64 {
        f64::from(self)
    }
}

impl Value for f64 {
    #[inline]
    fn widen(self) -> f64 {
        self
    }
}

/// Rows of values, [`Rows::lanes`] per sample: [`Rows::prepare`] makes a
/// range of rows readable, then [`Rows::row`] reads them together.
pub(super) trait Rows {
    type Value: Value;
    fn lanes(&self) -> usize;
    fn prepare(&mut self, r0: usize, r1: usize);
    /// The values of samples `k0..k1` of a prepared row `r`.
    fn row(&self, r: usize, k0: usize, k1: usize) -> &[Self::Value];
}

/// Interleaved eight-bit samples, read in place.
pub(super) struct Bytes<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) width: usize,
    pub(super) lanes: usize,
}

impl Rows for Bytes<'_> {
    type Value = u8;

    fn lanes(&self) -> usize {
        self.lanes
    }

    #[inline]
    fn prepare(&mut self, _: usize, _: usize) {}

    #[inline]
    fn row(&self, r: usize, k0: usize, k1: usize) -> &[u8] {
        let n = self.lanes;
        let start = (r * self.width + k0) * n;
        self.bytes.get(start..start + (k1 - k0) * n).unwrap_or(&[])
    }
}

/// Whole rows kept by row index, direct mapped: `capacity` consecutive
/// rows are readable together.
#[derive(Default)]
pub(super) struct Ring<V> {
    rows: Vec<(usize, Vec<V>)>,
}

impl<V: Value> Ring<V> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            rows: (0..capacity.max(1).next_power_of_two())
                .map(|_| (usize::MAX, Vec::new()))
                .collect(),
        }
    }

    /// The row `r`'s storage and whether it holds the row already.
    #[inline]
    fn slot(&mut self, r: usize) -> (bool, &mut Vec<V>) {
        let mask = self.rows.len() - 1;
        let entry = &mut self.rows[r & mask];
        let hit = entry.0 == r;
        entry.0 = r;
        (hit, &mut entry.1)
    }

    #[inline]
    fn get(&self, r: usize) -> &[V] {
        &self.rows[r & (self.rows.len() - 1)].1
    }
}

/// One-bit samples unpacked to codes 0 and 1.
pub(super) struct Bits<'a, 'k> {
    pub(super) packed: &'a Packed,
    pub(super) ring: &'k mut Ring<u8>,
}

impl Rows for Bits<'_, '_> {
    type Value = u8;

    fn lanes(&self) -> usize {
        1
    }

    fn prepare(&mut self, r0: usize, r1: usize) {
        let width = self.packed.width();
        for r in r0..r1 {
            let (hit, row) = self.ring.slot(r);
            if !hit {
                row.resize(width, 0);
                let bits = self.packed.row(r).unwrap_or(&[]);
                for (x, out) in row.iter_mut().enumerate() {
                    *out = bits.get(x / 8).map_or(0, |b| (b >> (7 - x % 8)) & 1);
                }
            }
        }
    }

    #[inline]
    fn row(&self, r: usize, k0: usize, k1: usize) -> &[u8] {
        &self.ring.get(r)[k0..k1]
    }
}

/// A JPEG's component planes converted to RGB codes whole rows at a time.
pub(super) struct Planes<'a, 'k> {
    pub(super) planar: &'a Planar,
    pub(super) ring: &'k mut Ring<u8>,
}

impl Rows for Planes<'_, '_> {
    type Value = u8;

    fn lanes(&self) -> usize {
        3
    }

    fn prepare(&mut self, r0: usize, r1: usize) {
        let width = self.planar.width();
        for r in r0..r1 {
            let (hit, row) = self.ring.slot(r);
            if !hit {
                row.resize(width * 3, 0);
                if !self.planar.rgb(r, row) {
                    row.fill(0);
                }
            }
        }
    }

    #[inline]
    fn row(&self, r: usize, k0: usize, k1: usize) -> &[u8] {
        &self.ring.get(r)[k0 * 3..k1 * 3]
    }
}

/// Texels of eight-bit samples converted once per distinct sample: a table
/// per code for one component, a memo by sample otherwise.
pub(super) struct Texeler<'c> {
    colors: &'c Colors,
    components: usize,
    lanes: usize,
    table: Vec<f64>,
    memo: Memo,
    /// Conversions made, for the report.
    pub(super) conversions: usize,
}

/// Open addressing by sample key, behind a small direct-mapped cache of
/// recent samples that neighbouring samples hit.
struct Memo {
    keys: Vec<u64>,
    values: Vec<f64>,
    lanes: usize,
    len: usize,
    recent_keys: Vec<u64>,
    recent: Vec<f64>,
}

/// Entries of the recent-sample cache.
const RECENT: usize = 1 << 12;

const EMPTY: u64 = u64::MAX;

impl Memo {
    fn new(lanes: usize) -> Self {
        Self {
            keys: vec![EMPTY; 1 << 12],
            values: vec![0.0; (1 << 12) * lanes],
            lanes,
            len: 0,
            recent_keys: vec![EMPTY; RECENT],
            recent: vec![0.0; RECENT * lanes],
        }
    }

    #[inline]
    fn slot(&self, key: u64) -> usize {
        let mask = self.keys.len() - 1;
        let mut slot = (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40) as usize & mask;
        while self.keys[slot] != key && self.keys[slot] != EMPTY {
            slot = (slot + 1) & mask;
        }
        slot
    }

    fn grow(&mut self) {
        let size = self.keys.len() * 2;
        let keys = std::mem::replace(&mut self.keys, vec![EMPTY; size]);
        let values = std::mem::replace(&mut self.values, vec![0.0; size * self.lanes]);
        let n = self.lanes;
        for (i, &key) in keys.iter().enumerate() {
            if key != EMPTY {
                let slot = self.slot(key);
                self.keys[slot] = key;
                self.values[slot * n..slot * n + n].copy_from_slice(&values[i * n..i * n + n]);
            }
        }
    }
}

impl<'c> Texeler<'c> {
    pub(super) fn new(colors: &'c Colors, components: usize) -> Self {
        let lanes = colors.lanes() - 2;
        let mut table = Vec::new();
        if components == 1 {
            let mut texel = vec![0.0; lanes + 2];
            for code in 0..=255u16 {
                colors.plain_texel(&[code], &mut texel);
                table.extend_from_slice(&texel[..lanes]);
            }
        }
        Self {
            colors,
            components,
            lanes,
            table,
            memo: Memo::new(lanes),
            conversions: if components == 1 { 256 } else { 0 },
        }
    }

    /// The colour lanes of samples `codes`, into `out`.
    pub(super) fn convert(&mut self, codes: &[u8], out: &mut [f64]) {
        match (self.components, self.lanes) {
            (3, 3) => self.convert_n::<3, 3>(codes, out),
            (3, 4) => self.convert_n::<3, 4>(codes, out),
            (4, 3) => self.convert_n::<4, 3>(codes, out),
            (4, 4) => self.convert_n::<4, 4>(codes, out),
            _ => self.convert_any(codes, out),
        }
    }

    /// [`Self::convert`] for `C` components and `N` lanes.
    fn convert_n<const C: usize, const N: usize>(&mut self, codes: &[u8], out: &mut [f64]) {
        let mut previous = EMPTY;
        let mut value = [0.0; N];
        for (sample, out) in codes
            .as_chunks::<C>()
            .0
            .iter()
            .zip(out.as_chunks_mut::<N>().0)
        {
            let key = sample.iter().fold(0u64, |k, &c| (k << 8) | u64::from(c));
            if key != previous {
                previous = key;
                let near = ((key ^ (key >> 7) ^ (key >> 13)) as usize) & (RECENT - 1);
                if self.memo.recent_keys[near] == key
                    && let Some(v) = self.memo.recent[near * N..].first_chunk::<N>()
                {
                    value = *v;
                    *out = value;
                    continue;
                }
                let mut slot = self.memo.slot(key);
                if self.memo.keys[slot] == EMPTY {
                    let mut wide = [0u16; C];
                    for (w, &c) in wide.iter_mut().zip(sample) {
                        *w = u16::from(c);
                    }
                    let mut texel = [0.0; 70];
                    self.colors.plain_texel(&wide, &mut texel[..N + 2]);
                    self.conversions += 1;
                    self.memo.keys[slot] = key;
                    self.memo.values[slot * N..slot * N + N].copy_from_slice(&texel[..N]);
                    self.memo.len += 1;
                    if self.memo.len * 2 > self.memo.keys.len() {
                        self.memo.grow();
                        slot = self.memo.slot(key);
                    }
                }
                if let Some(v) = self.memo.values[slot * N..].first_chunk::<N>() {
                    value = *v;
                }
                self.memo.recent_keys[near] = key;
                self.memo.recent[near * N..near * N + N].copy_from_slice(&value);
            }
            *out = value;
        }
    }

    fn convert_any(&mut self, codes: &[u8], out: &mut [f64]) {
        let n = self.lanes;
        if self.components == 1 {
            for (&code, out) in codes.iter().zip(out.chunks_exact_mut(n)) {
                let at = usize::from(code) * n;
                out.copy_from_slice(&self.table[at..at + n]);
            }
            return;
        }
        let mut previous = EMPTY;
        let mut texel = [0.0; 70];
        let mut last = 0;
        for (sample, out) in codes
            .chunks_exact(self.components)
            .zip(out.chunks_exact_mut(n))
        {
            let key = sample.iter().fold(0u64, |k, &c| (k << 8) | u64::from(c));
            if key != previous {
                previous = key;
                let mut slot = self.memo.slot(key);
                if self.memo.keys[slot] == EMPTY {
                    let mut wide = [0u16; 4];
                    for (w, &c) in wide.iter_mut().zip(sample) {
                        *w = u16::from(c);
                    }
                    self.colors
                        .plain_texel(&wide[..self.components], &mut texel[..n + 2]);
                    self.conversions += 1;
                    self.memo.keys[slot] = key;
                    self.memo.values[slot * n..slot * n + n].copy_from_slice(&texel[..n]);
                    self.memo.len += 1;
                    if self.memo.len * 2 > self.memo.keys.len() {
                        self.memo.grow();
                        slot = self.memo.slot(key);
                    }
                }
                last = slot;
            }
            out.copy_from_slice(&self.memo.values[last * n..last * n + n]);
        }
    }
}

/// Codes rows converted to texel rows, whole rows at a time.
pub(super) struct Converted<'t, 'c, R> {
    pub(super) codes: R,
    pub(super) texeler: &'t mut Texeler<'c>,
    pub(super) ring: &'t mut Ring<f64>,
    pub(super) width: usize,
}

impl<R: Rows<Value = u8>> Rows for Converted<'_, '_, R> {
    type Value = f64;

    fn lanes(&self) -> usize {
        self.texeler.lanes
    }

    fn prepare(&mut self, r0: usize, r1: usize) {
        let n = self.texeler.lanes;
        for r in r0..r1 {
            let (hit, row) = self.ring.slot(r);
            if !hit {
                row.resize(self.width * n, 0.0);
                self.codes.prepare(r, r + 1);
                let codes = self.codes.row(r, 0, self.width);
                self.texeler.convert(codes, row);
            }
        }
    }

    #[inline]
    fn row(&self, r: usize, k0: usize, k1: usize) -> &[f64] {
        let n = self.texeler.lanes;
        &self.ring.get(r)[k0 * n..k1 * n]
    }
}

/// A soft mask's opacities by code, whole rows at a time.
pub(super) struct Opacities<'a, 'k> {
    pub(super) codes: &'a [u8],
    pub(super) width: usize,
    pub(super) table: &'k [f64; 256],
    pub(super) ring: &'k mut Ring<f64>,
}

impl Rows for Opacities<'_, '_> {
    type Value = f64;

    fn lanes(&self) -> usize {
        1
    }

    fn prepare(&mut self, r0: usize, r1: usize) {
        for r in r0..r1 {
            let (hit, row) = self.ring.slot(r);
            if !hit {
                row.clear();
                let start = r * self.width;
                let table = self.table;
                row.extend(
                    self.codes
                        .get(start..start + self.width)
                        .unwrap_or(&[])
                        .iter()
                        .map(|&c| table[usize::from(c)]),
                );
                row.resize(self.width, 0.0);
            }
        }
    }

    #[inline]
    fn row(&self, r: usize, k0: usize, k1: usize) -> &[f64] {
        &self.ring.get(r)[k0..k1]
    }
}

/// Texel rows with the soft mask's opacity of the same sample appended as
/// one more lane, for a mask on the image's grid: one footprint walk
/// averages both.
pub(super) struct Fused<'k, R> {
    pub(super) texels: R,
    pub(super) codes: &'k [u8],
    pub(super) table: &'k [f64; 256],
    pub(super) ring: &'k mut Ring<f64>,
    pub(super) width: usize,
}

impl<R: Rows<Value = f64>> Rows for Fused<'_, R> {
    type Value = f64;

    fn lanes(&self) -> usize {
        self.texels.lanes() + 1
    }

    fn prepare(&mut self, r0: usize, r1: usize) {
        let n = self.texels.lanes();
        for r in r0..r1 {
            let (hit, row) = self.ring.slot(r);
            if !hit {
                row.resize(self.width * (n + 1), 0.0);
                self.texels.prepare(r, r + 1);
                let texels = self.texels.row(r, 0, self.width);
                let start = r * self.width;
                let codes = self.codes.get(start..start + self.width).unwrap_or(&[]);
                match n {
                    3 => fuse::<3, 4>(row, texels, codes, self.table),
                    4 => fuse::<4, 5>(row, texels, codes, self.table),
                    _ => {
                        for ((out, texel), &code) in row
                            .chunks_exact_mut(n + 1)
                            .zip(texels.chunks_exact(n))
                            .zip(codes)
                        {
                            out[..n].copy_from_slice(texel);
                            out[n] = self.table[usize::from(code)];
                        }
                    }
                }
            }
        }
    }

    #[inline]
    fn row(&self, r: usize, k0: usize, k1: usize) -> &[f64] {
        let n = self.texels.lanes() + 1;
        &self.ring.get(r)[k0 * n..k1 * n]
    }
}

/// Texels of `N` lanes and their opacity codes as rows of `M = N + 1`.
fn fuse<const N: usize, const M: usize>(
    row: &mut [f64],
    texels: &[f64],
    codes: &[u8],
    table: &[f64; 256],
) {
    for ((out, texel), &code) in row
        .as_chunks_mut::<M>()
        .0
        .iter_mut()
        .zip(texels.as_chunks::<N>().0)
        .zip(codes)
    {
        out[..N].copy_from_slice(texel);
        out[N] = table[usize::from(code)];
    }
}

/// Scratch of the kernels.
#[derive(Default)]
pub(super) struct Scratch {
    lines: Vec<f64>,
    between: Vec<f64>,
    sums: Vec<f64>,
    values: Vec<f64>,
}

/// Writes the averages of device pixels of one row of an upright placement
/// with column footprints `feet`, `n` values each, into `out`, as the
/// separable engine forms them: per source column the row footprint's
/// first row weighted, plus the rows between, plus the last row weighted;
/// per pixel its first column weighted, plus those between, plus its last
/// weighted; times both normalizations. `covered` tells the pixels whose
/// footprint meets the image.
pub(super) fn upright<R: Rows>(
    rows: &mut R,
    (across, feet): (Footprint, &[Footprint]),
    scratch: &mut Scratch,
    (out, covered): (&mut [f64], &mut [bool]),
) {
    let n = rows.lanes();
    covered.fill(false);
    if across.first >= across.end {
        out.fill(0.0);
        return;
    }
    let (first, end) = feet
        .iter()
        .filter(|f| f.first < f.end)
        .fold((u32::MAX, 0), |(a, b), f| (a.min(f.first), b.max(f.end)));
    if first >= end {
        out.fill(0.0);
        return;
    }
    let (first, end) = (first as usize, end as usize);
    let length = (end - first) * n;
    let Scratch { lines, between, .. } = scratch;
    lines.clear();
    lines.resize(length, 0.0);
    between.clear();
    between.resize(length, 0.0);
    let (top, bottom) = (across.first as usize, across.end as usize - 1);
    rows.prepare(top, bottom + 1);
    for y in top..=bottom {
        let values = rows.row(y, first, end);
        if values.len() != length {
            lines.fill(0.0);
            break;
        }
        if y == top {
            for (line, v) in lines.iter_mut().zip(values) {
                *line = across.first_weight * v.widen();
            }
        } else if y == bottom && bottom - top > 1 {
            for ((line, &sum), v) in lines.iter_mut().zip(between.iter()).zip(values) {
                *line = (*line + sum) + across.last_weight * v.widen();
            }
        } else if y == bottom {
            for (line, v) in lines.iter_mut().zip(values) {
                *line += across.last_weight * v.widen();
            }
        } else if y == top + 1 {
            for (sum, v) in between.iter_mut().zip(values) {
                *sum = v.widen();
            }
        } else {
            for (sum, v) in between.iter_mut().zip(values) {
                *sum += v.widen();
            }
        }
    }
    match n {
        1 => columns::<1>(lines, (across, feet, first), (out, covered)),
        2 => columns::<2>(lines, (across, feet, first), (out, covered)),
        3 => columns::<3>(lines, (across, feet, first), (out, covered)),
        4 => columns::<4>(lines, (across, feet, first), (out, covered)),
        5 => columns::<5>(lines, (across, feet, first), (out, covered)),
        _ => {}
    }
}

/// Each pixel's sum of its columns' lines, `N` values a column.
#[inline]
fn columns<const N: usize>(
    lines: &[f64],
    (across, feet, first): (Footprint, &[Footprint], usize),
    (out, covered): (&mut [f64], &mut [bool]),
) {
    let lines = lines.as_chunks::<N>().0;
    for ((foot, pixel), covered) in feet.iter().zip(out.as_chunks_mut::<N>().0).zip(covered) {
        if foot.first >= foot.end {
            *pixel = [0.0; N];
            continue;
        }
        *covered = true;
        let factor = foot.normalization * across.normalization;
        let cells = &lines[foot.first as usize - first..foot.end as usize - first];
        let mut sum = cells[0].map(|v| foot.first_weight * v);
        if let [_, middle @ .., last] = cells {
            for cell in middle {
                for (s, v) in sum.iter_mut().zip(cell) {
                    *s += v;
                }
            }
            for (s, v) in sum.iter_mut().zip(last) {
                *s += foot.last_weight * v;
            }
        }
        *pixel = sum.map(|s| s * factor);
    }
}

/// One edge of every pixel's preimage: its displacement and the parameter
/// steps between the vertical and the horizontal grid lines it crosses.
#[derive(Debug, Clone, Copy)]
pub(super) struct Edge {
    pub(super) dx: f64,
    pub(super) dy: f64,
    pub(super) step_x: f64,
    pub(super) step_y: f64,
}

impl Edge {
    fn new(dx: f64, dy: f64) -> Self {
        Self {
            dx,
            dy,
            step_x: 1.0 / dx.abs(),
            step_y: 1.0 / dy.abs(),
        }
    }

    /// The edges of the preimage of device pixels under `t`, from corner
    /// `(x, y)` around: `+x`, `+y`, `−x`, `−y` of the device.
    pub(super) fn of(t: pagura_raster::Transform) -> [Self; 4] {
        [
            Self::new(t.a, t.b),
            Self::new(t.c, t.d),
            Self::new(-t.a, -t.b),
            Self::new(-t.c, -t.d),
        ]
    }
}

/// The integral of `rows` over the parallelogram `corners` (a pixel's
/// preimage, in order around it) within the image of `width` × `height`
/// samples, `N` values, and the area of that part: Green's theorem with
/// `F(x, y)`, the partial sum of row `floor(y)` from the footprint's first
/// column to `x`. Each edge is split where it crosses a grid line; on each
/// piece `F` is linear, so the piece adds `F` at its midpoint times its
/// height. Rows outside the image add nothing, and `F` holds still left and
/// right of it, so the integral is over the image's part of the footprint.
#[inline]
pub(super) fn contour<const N: usize, R: Rows>(
    rows: &mut R,
    (width, height): (u32, u32),
    corners: &[Point; 4],
    edges: &[Edge; 4],
    sums: &mut Vec<f64>,
) -> ([f64; N], f64) {
    let mut out = [0.0; N];
    let (mut low, mut high) = (corners[0], corners[0]);
    for p in &corners[1..] {
        low = Point::new(low.x.min(p.x), low.y.min(p.y));
        high = Point::new(high.x.max(p.x), high.y.max(p.y));
    }
    let clamp = |v: f64, max: u32| v.clamp(0.0, f64::from(max)) as usize;
    let (x0, x1) = (clamp(low.x.floor(), width), clamp(high.x.ceil(), width));
    let (y0, y1) = (clamp(low.y.floor(), height), clamp(high.y.ceil(), height));
    if x0 >= x1 || y0 >= y1 {
        return (out, 0.0);
    }
    let columns = x1 - x0;
    // Per row: the partial sums, `columns + 1`, then the values, `columns`,
    // `N` lanes each.
    let values = (columns + 1) * N;
    let stride = values + columns * N;
    sums.clear();
    rows.prepare(y0, y1);
    for r in y0..y1 {
        let row = rows.row(r, x0, x1);
        let mut sum = [0.0; N];
        sums.extend_from_slice(&sum);
        for sample in row.chunks_exact(N) {
            for c in 0..N {
                sum[c] += sample[c].widen();
            }
            sums.extend_from_slice(&sum);
        }
        sums.extend(row.iter().map(|v| v.widen()));
    }
    if sums.len() != (y1 - y0) * stride {
        return (out, 0.0);
    }
    let left = x0 as f64;
    let total = (x1 - x0) as f64;
    let (top, count) = (y0 as isize, (y1 - y0) as isize);
    let first = x0 as isize;
    let mut area = 0.0;
    for (p, edge) in corners.iter().zip(edges) {
        let Edge {
            dx,
            dy,
            step_x,
            step_y,
        } = *edge;
        if dy == 0.0 {
            continue;
        }
        // The parameters of the grid lines the edge crosses, in order, and
        // the cell the edge is in, counted as it crosses them; `step_x` and
        // `step_y` are the parameter steps between lines, `1 / |dx|` and
        // `1 / |dy|`.
        let (mut tx, k, move_x) = if dx > 0.0 {
            ((p.x.floor() + 1.0 - p.x) * step_x, p.x.floor(), 1)
        } else if dx < 0.0 {
            ((p.x - (p.x.ceil() - 1.0)) * step_x, p.x.ceil() - 1.0, -1)
        } else {
            (f64::INFINITY, p.x.floor(), 0)
        };
        let (mut ty, r, move_y) = if dy > 0.0 {
            ((p.y.floor() + 1.0 - p.y) * step_y, p.y.floor(), 1)
        } else {
            ((p.y - (p.y.ceil() - 1.0)) * step_y, p.y.ceil() - 1.0, -1)
        };
        // Cell column and row relative to the footprint's first.
        let (mut cell, mut row) = (k as isize - first, r as isize - top);
        let mut t0 = 0.0;
        loop {
            let t1 = tx.min(ty).min(1.0);
            if row >= 0 && row < count && cell >= 0 {
                let h = (t1 - t0) * dy;
                let base = row as usize * stride;
                if (cell as usize) < columns {
                    let x = p.x + 0.5 * (t0 + t1) * dx;
                    let fraction = x - (cell + first) as f64;
                    area += (x - left) * h;
                    let at = base + cell as usize * N;
                    if let (Some(s), Some(v)) = (
                        sums[at..].first_chunk::<N>(),
                        sums[base + values + cell as usize * N..].first_chunk::<N>(),
                    ) {
                        for c in 0..N {
                            out[c] += (s[c] + fraction * v[c]) * h;
                        }
                    }
                } else {
                    area += total * h;
                    if let Some(s) = sums[base + columns * N..].first_chunk::<N>() {
                        for c in 0..N {
                            out[c] += s[c] * h;
                        }
                    }
                }
            }
            if t1 >= 1.0 {
                break;
            }
            if tx <= ty {
                tx += step_x;
                cell += move_x;
            } else {
                ty += step_y;
                row += move_y;
            }
            t0 = t1;
        }
    }
    if area < 0.0 {
        (out.map(|v| -v), -area)
    } else {
        (out, area)
    }
}

/// The value of an average code on a unit map, as the codes sampler takes
/// it: the product unless it does not multiply back to the code, then the
/// quotient.
#[inline]
fn unit(code: f64, maximum: f64) -> f64 {
    let value = code * (1.0 / maximum);
    if value * maximum == code {
        value
    } else {
        code / maximum
    }
}

/// The geometry of one placement: upright column footprints of the region,
/// or the general inverse.
pub(super) struct Placement {
    pub(super) geometry: Geometry,
    pub(super) x0: u32,
    pub(super) feet: Vec<Footprint>,
    pub(super) upright: bool,
    /// The smallest part of a footprint averaged: 2^-12 of its area.
    pub(super) sliver: f64,
    /// Rows kept: those one device row reads, and a margin.
    pub(super) rows: usize,
    pub(super) edges: [Edge; 4],
}

impl Placement {
    pub(super) fn new(geometry: Geometry, (x0, x1): (u32, u32)) -> Self {
        let t = geometry.inverse();
        let upright = t.b == 0.0 && t.c == 0.0;
        let feet = if upright {
            (x0..x1).map(|x| column(&geometry, x)).collect()
        } else {
            Vec::new()
        };
        let sliver = (t.a * t.d - t.b * t.c).abs() / 4096.0;
        // Source rows one device row of the region meets.
        let span = (t.b.abs() * f64::from(x1 - x0) + t.d.abs()).ceil() + 3.0;
        let rows = (span.min(f64::from(geometry.size().1)) as usize + 2).next_power_of_two();
        Self {
            edges: Edge::of(t),
            geometry,
            x0,
            feet,
            upright,
            sliver,
            rows,
        }
    }

    fn across(&self, y: u32) -> Footprint {
        let t = self.geometry.inverse();
        footprint(self.geometry.mode(), y, (t.d, t.f, self.geometry.size().1))
    }

    /// Whether pixels take the nearest sample of an upright placement.
    fn nearest(&self) -> bool {
        self.upright && self.geometry.mode() == Mode::Nearest
    }

    /// Averages of device pixels `x .. x + len` of row `y`, `n` each, and
    /// whether each meets the image.
    pub(super) fn sample<R: Rows>(
        &self,
        rows: &mut R,
        (x, y, len): (u32, u32, usize),
        scratch: &mut Scratch,
        (out, covered): (&mut [f64], &mut [bool]),
    ) {
        if self.upright {
            let start = (x - self.x0) as usize;
            let feet = &self.feet[start..start + len];
            upright(rows, (self.across(y), feet), scratch, (out, covered));
            return;
        }
        match rows.lanes() {
            1 => self.general::<1, R>(rows, (x, y), scratch, (out, covered)),
            2 => self.general::<2, R>(rows, (x, y), scratch, (out, covered)),
            3 => self.general::<3, R>(rows, (x, y), scratch, (out, covered)),
            4 => self.general::<4, R>(rows, (x, y), scratch, (out, covered)),
            5 => self.general::<5, R>(rows, (x, y), scratch, (out, covered)),
            _ => {}
        }
    }

    fn general<const N: usize, R: Rows>(
        &self,
        rows: &mut R,
        (x, y): (u32, u32),
        _: &mut Scratch,
        (out, covered): (&mut [f64], &mut [bool]),
    ) {
        let t = self.geometry.inverse();
        let size = self.geometry.size();
        let (width, height) = size;
        let mode = self.geometry.mode();
        let mut sums: Vec<f64> = Vec::new();
        for ((px, pixel), covered) in (x..)
            .zip(out.as_chunks_mut::<N>().0)
            .zip(covered.iter_mut())
        {
            if matches!(mode, Mode::Area | Mode::Separable) {
                let (fx, fy) = (f64::from(px), f64::from(y));
                let corners = [
                    Point::new(fx, fy),
                    Point::new(fx + 1.0, fy),
                    Point::new(fx + 1.0, fy + 1.0),
                    Point::new(fx, fy + 1.0),
                ]
                .map(|p| p * t);
                let (sum, area) = contour::<N, R>(rows, size, &corners, &self.edges, &mut sums);
                // A footprint that meets the image in less than 2^-12 of
                // itself covers less than half a coverage level of the
                // pixel, whose coverage byte is then 0; the contour's
                // rounding would dominate its average.
                *covered = area > self.sliver;
                *pixel = if *covered {
                    let scale = 1.0 / area;
                    sum.map(|v| v * scale)
                } else {
                    [0.0; N]
                };
            } else {
                // Not measured here: the pages' turned images minify.
                let c = Point::new(f64::from(px) + 0.5, f64::from(y) + 0.5) * t;
                let cx = c.x.floor().clamp(0.0, f64::from(width - 1)) as usize;
                let cy = c.y.floor().clamp(0.0, f64::from(height - 1)) as usize;
                rows.prepare(cy, cy + 1);
                for (o, v) in pixel.iter_mut().zip(rows.row(cy, cx, cx + 1)) {
                    *o = v.widen();
                }
                *covered = true;
            }
        }
    }
}

fn column(geometry: &Geometry, x: u32) -> Footprint {
    let t = geometry.inverse();
    footprint(geometry.mode(), x, (t.a, t.e, geometry.size().0))
}

fn footprint(mode: Mode, pixel: u32, (scale, offset, maximum): (f64, f64, u32)) -> Footprint {
    match mode {
        Mode::Nearest => Footprint::nearest(pixel, scale, offset, maximum),
        Mode::Bilinear => Footprint::bilinear(pixel, scale, offset, maximum),
        Mode::Area | Mode::Separable => Footprint::new(pixel, scale, offset, maximum).trimmed(),
    }
}

/// How the averaged values become colour channels.
pub(super) enum Values<'c> {
    /// Codes: channel `c` is the unit map of the average code of component
    /// `component[c]`.
    Codes { component: Vec<usize>, maximum: f64 },
    /// Texels: the channels are the values.
    Texels(Texeler<'c>),
}

/// A soft mask of the prototype: its samples, placement and opacity table.
pub(super) struct Mask {
    pub(super) image: Arc<Decoded>,
    pub(super) placement: Placement,
    pub(super) table: [f64; 256],
}

/// The prototype sampler of one opaque colour image with an optional soft
/// mask.
pub(super) struct Proto<'c> {
    pub(super) image: Arc<Decoded>,
    pub(super) placement: Placement,
    pub(super) values: Values<'c>,
    pub(super) channels: usize,
    pub(super) mask: Option<Mask>,
    /// Per code, its channel value and its N16 word, for nearest codes.
    levels: [f64; 256],
    words: [u16; 256],
    scratch: Scratch,
    averages: Vec<f64>,
    opacities: Vec<f64>,
    covered: Vec<bool>,
    mask_covered: Vec<bool>,
    codes: Ring<u8>,
    texels: Ring<f64>,
    mask_rows: Ring<f64>,
    fused: Ring<f64>,
    /// Whether the mask is on the image's grid and averages with it.
    pub(super) fuse: bool,
}

impl<'c> Proto<'c> {
    pub(super) fn new(
        image: Arc<Decoded>,
        placement: Placement,
        values: Values<'c>,
        channels: usize,
        mask: Option<(Arc<Decoded>, Placement, &Opacity)>,
    ) -> Self {
        let mask = mask.map(|(image, placement, opacity)| {
            let mut table = [0.0; 256];
            for (code, value) in (0u16..).zip(table.iter_mut()) {
                *value = opacity.of_code(code);
            }
            Mask {
                image,
                placement,
                table,
            }
        });
        let maximum = match &values {
            Values::Codes { maximum, .. } => *maximum,
            Values::Texels(_) => 255.0,
        };
        let mut levels = [0.0; 256];
        let mut words = [0u16; 256];
        for code in 0..=255u8 {
            let level = unit(f64::from(code), maximum);
            levels[usize::from(code)] = level;
            words[usize::from(code)] = (level.max(0.0).min(1.0) * 65535.0).round() as u16;
        }
        let rows = placement.rows;
        let mask_rows = mask.as_ref().map_or(1, |m| m.placement.rows);
        Self {
            image,
            placement,
            values,
            channels,
            mask,
            levels,
            words,
            scratch: Scratch::default(),
            averages: Vec::new(),
            opacities: Vec::new(),
            covered: Vec::new(),
            mask_covered: Vec::new(),
            codes: Ring::with_capacity(rows),
            texels: Ring::with_capacity(rows),
            mask_rows: Ring::with_capacity(mask_rows),
            fused: Ring::with_capacity(rows),
            fuse: false,
        }
    }

    /// The conversions made so far.
    pub(super) fn conversions(&self) -> usize {
        match &self.values {
            Values::Texels(t) => t.conversions,
            Values::Codes { .. } => 0,
        }
    }

    /// Averages of the image's values for the span, returning the values
    /// per pixel.
    fn average(&mut self, span: (u32, u32, usize)) -> usize {
        let len = span.2;
        let image = &self.image.image;
        let width = image.width as usize;
        let components = usize::from(image.components);
        let lanes = match &self.values {
            Values::Codes { .. } => components,
            Values::Texels(texeler) => texeler.lanes + usize::from(self.fuse),
        };
        self.covered.clear();
        self.covered.resize(len, false);
        self.averages.clear();
        self.averages.resize(len * lanes, 0.0);
        let out = (self.averages.as_mut_slice(), self.covered.as_mut_slice());
        let scratch = &mut self.scratch;
        let placement = &self.placement;
        match (&mut self.values, &image.samples) {
            (Values::Codes { .. }, Samples::Bytes(bytes)) => {
                let mut rows = Bytes {
                    bytes,
                    width,
                    lanes: components,
                };
                placement.sample(&mut rows, span, scratch, out);
            }
            (Values::Codes { .. }, Samples::Packed(packed)) => {
                let mut rows = Bits {
                    packed,
                    ring: &mut self.codes,
                };
                placement.sample(&mut rows, span, scratch, out);
            }
            (Values::Codes { .. }, Samples::Planar(planar)) => {
                let mut rows = Planes {
                    planar,
                    ring: &mut self.codes,
                };
                placement.sample(&mut rows, span, scratch, out);
            }
            (Values::Texels(texeler), Samples::Bytes(bytes)) => {
                let rows = Converted {
                    codes: Bytes {
                        bytes,
                        width,
                        lanes: components,
                    },
                    texeler,
                    ring: &mut self.texels,
                    width,
                };
                match (&self.mask, self.fuse) {
                    (Some(mask), true) => {
                        let Samples::Bytes(codes) = &mask.image.image.samples else {
                            return lanes;
                        };
                        let mut rows = Fused {
                            texels: rows,
                            codes,
                            table: &mask.table,
                            ring: &mut self.fused,
                            width,
                        };
                        placement.sample(&mut rows, span, scratch, out);
                    }
                    _ => {
                        let mut rows = rows;
                        placement.sample(&mut rows, span, scratch, out);
                    }
                }
            }
            (Values::Texels(texeler), Samples::Planar(planar)) => {
                let mut rows = Converted {
                    codes: Planes {
                        planar,
                        ring: &mut self.codes,
                    },
                    texeler,
                    ring: &mut self.texels,
                    width,
                };
                placement.sample(&mut rows, span, scratch, out);
            }
            _ => {}
        }
        lanes
    }

    /// Mask opacities of the span.
    fn mask(&mut self, span: (u32, u32, usize)) {
        let Some(mask) = &self.mask else {
            return;
        };
        let len = span.2;
        let Samples::Bytes(codes) = &mask.image.image.samples else {
            return;
        };
        let mut rows = Opacities {
            codes,
            width: mask.image.image.width as usize,
            table: &mask.table,
            ring: &mut self.mask_rows,
        };
        self.opacities.clear();
        self.opacities.resize(len, 0.0);
        self.mask_covered.clear();
        self.mask_covered.resize(len, false);
        mask.placement.sample(
            &mut rows,
            span,
            &mut self.scratch,
            (&mut self.opacities, &mut self.mask_covered),
        );
    }

    /// The nearest codes of an upright placement for the span, `n` per
    /// pixel, handed to `finish`.
    fn nearest<T>(
        &mut self,
        (x, y, len): (u32, u32, usize),
        finish: impl FnOnce(&[u8], &[Footprint], usize, &Self) -> T,
    ) -> Option<T> {
        if !self.placement.nearest() || !matches!(self.values, Values::Codes { .. }) {
            return None;
        }
        let across = self.placement.across(y);
        let start = (x - self.placement.x0) as usize;
        let low = self.placement.feet[start]
            .first
            .min(self.placement.feet[start + len - 1].first);
        let high = self.placement.feet[start]
            .first
            .max(self.placement.feet[start + len - 1].first);
        let (low, high) = (low as usize, high as usize + 1);
        let row = across.first as usize;
        let mut codes = std::mem::take(&mut self.codes);
        let image = &self.image.image;
        let width = image.width as usize;
        let n = usize::from(image.components);
        let feet = &self.placement.feet[start..start + len];
        let result = match &image.samples {
            Samples::Bytes(bytes) => {
                let rows = Bytes {
                    bytes,
                    width,
                    lanes: n,
                };
                Some(finish(rows.row(row, low, high), feet, low, self))
            }
            Samples::Packed(packed) => {
                let mut rows = Bits {
                    packed,
                    ring: &mut codes,
                };
                rows.prepare(row, row + 1);
                Some(finish(rows.row(row, low, high), feet, low, self))
            }
            Samples::Planar(planar) => {
                let mut rows = Planes {
                    planar,
                    ring: &mut codes,
                };
                rows.prepare(row, row + 1);
                Some(finish(rows.row(row, low, high), feet, low, self))
            }
            _ => None,
        };
        self.codes = codes;
        result
    }

    /// Fills `out` as [`super::ImageSampler::span`] does: straight colours
    /// and one alpha per pixel.
    pub(super) fn span(&mut self, x: u32, y: u32, len: usize, out: &mut SpanColors) {
        let channels = self.channels;
        out.channels = channels;
        out.colors.clear();
        out.alpha.clear();
        if self.mask.is_none() {
            let done = self.nearest((x, y, len), |codes, feet, low, this| {
                let Values::Codes { component, .. } = &this.values else {
                    return;
                };
                let n = codes.len() / (feet.len().max(1)).max(1);
                let n = if n == 0 { 1 } else { n };
                let _ = n;
                let lanes = usize::from(this.image.image.components);
                for foot in feet {
                    let sample = &codes[(foot.first as usize - low) * lanes..];
                    for &k in component {
                        out.colors.push(this.levels[usize::from(sample[k])]);
                    }
                }
            });
            if done.is_some() {
                return;
            }
        }
        let n = self.average((x, y, len));
        if self.fuse {
            self.opacities.clear();
            self.opacities
                .extend(self.averages.chunks_exact(n).map(|v| v[n - 1]));
        } else {
            self.mask((x, y, len));
        }
        for i in 0..len {
            let values = &self.averages[i * n..i * n + n];
            let mut color = [0.0; 8];
            if self.covered[i] {
                match &self.values {
                    Values::Codes { component, maximum } => {
                        for (c, &k) in component.iter().enumerate() {
                            color[c] = unit(values[k], *maximum);
                        }
                    }
                    Values::Texels(_) => color[..channels].copy_from_slice(&values[..channels]),
                }
            }
            let mut alpha = f64::from(u8::from(self.covered[i]));
            if self.mask.is_some() {
                let factor = self.opacities[i];
                for v in &mut color[..channels] {
                    *v *= factor;
                }
                alpha *= factor;
            }
            if alpha != 1.0 {
                for v in &mut color[..channels] {
                    *v = if alpha > 0.0 { *v / alpha } else { 0.0 };
                }
            }
            out.colors.extend_from_slice(&color[..channels]);
            out.alpha.push(alpha);
        }
    }

    /// Writes the N16 words of an opaque, covered span of an image without a
    /// mask into `out`, as a store at full opacity would; false otherwise.
    pub(super) fn words(&mut self, x: u32, y: u32, out: &mut [[u16; 4]]) -> bool {
        if self.mask.is_some() || self.channels != 3 {
            return false;
        }
        let len = out.len();
        let done = self.nearest((x, y, len), |codes, feet, low, this| {
            let Values::Codes { component, .. } = &this.values else {
                return;
            };
            let lanes = usize::from(this.image.image.components);
            let [a, b, c] = [component[0], component[1], component[2]];
            let words = &this.words;
            for (pixel, foot) in out.iter_mut().zip(feet) {
                let at = (foot.first as usize - low) * lanes;
                let sample = &codes[at..at + lanes];
                *pixel = [
                    words[usize::from(sample[a])],
                    words[usize::from(sample[b])],
                    words[usize::from(sample[c])],
                    u16::MAX,
                ];
            }
        });
        if done.is_some() {
            return true;
        }
        let n = self.average((x, y, len));
        if !self.covered.iter().all(|&c| c) {
            return false;
        }
        let word = |v: f64| (v.max(0.0).min(1.0) * 65535.0).round() as u16;
        match &self.values {
            Values::Codes { component, maximum } => {
                let [a, b, c] = [component[0], component[1], component[2]];
                for (pixel, values) in out.iter_mut().zip(self.averages.chunks_exact(n)) {
                    *pixel = [
                        word(unit(values[a], *maximum)),
                        word(unit(values[b], *maximum)),
                        word(unit(values[c], *maximum)),
                        u16::MAX,
                    ];
                }
            }
            Values::Texels(_) => {
                for (pixel, values) in out.iter_mut().zip(self.averages.chunks_exact(n)) {
                    *pixel = [word(values[0]), word(values[1]), word(values[2]), u16::MAX];
                }
            }
        }
        true
    }
}
