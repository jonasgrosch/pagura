//! The exact area average of turned placements (8.9.5.3, ADR 0004). A
//! device pixel's preimage is a parallelogram; the integral of the
//! piecewise-constant texels over its part inside the image is, by Green's
//! theorem, the integral of `F dy` around its boundary, where `F(x, y)` is
//! the partial sum of row `⌊y⌋` up to `x`. Along each boundary piece
//! between two grid lines `F` is linear, so the piece adds `F` at its
//! midpoint times its height, exactly; rows outside the image add nothing
//! and `F` holds still left and right of it, so no polygon is clipped.
//!
//! A device row is sampled as one strip: the partial sums of the source
//! rows it meets are built once, relative to a base column fixed by the
//! device row and the source row alone, and each vertical device grid edge
//! is integrated once for the two pixels beside it. Every value depends
//! only on its pixel and the placement, so a pixel samples alike in every
//! span, band and region.
//!
//! A footprint that meets the image in less than 2^-12 of itself covers
//! less than half a coverage level of its pixel, whose coverage byte is
//! then 0; such a pixel is left uncovered, since its few samples' average
//! would carry the contour's rounding.

use super::geometry::{Geometry, Texels};
use super::scratch::allocate;
use crate::source::Pixels;
use pagura_core::Result;
use pagura_raster::{Point, Transform};

/// The turned placement of one image: its inverse, size and scratch.
#[derive(Debug)]
pub(super) struct Contour {
    inverse: Transform,
    width: u32,
    height: u32,
    /// The smallest part of a footprint averaged.
    sliver: f64,
    /// An upper bound of the bytes a strip of the region holds.
    layout: Layout,
}

/// One source row of a strip: its partial sums from `base` for columns
/// `first..=end`, then its values for `first..end`, at `offset` in the
/// strip's sums.
#[derive(Debug, Clone, Copy, Default)]
struct Row {
    base: f64,
    first: usize,
    end: usize,
    offset: usize,
}

/// Storage reused between spans.
#[derive(Debug, Default)]
pub(super) struct Scratch {
    /// Source rows `top..top + rows.len()` of the strip.
    top: usize,
    rows: Vec<Row>,
    sums: Vec<f64>,
    /// Device grid points of the span's upper and lower lines.
    upper: Vec<Point>,
    lower: Vec<Point>,
    /// Integrals along the vertical device edges, `n + 1` values each.
    vertical: Vec<f64>,
    /// One read row of texels.
    texels: Vec<f64>,
    /// One channel at a time for less common channel counts.
    single: Vec<f64>,
}

/// Capacities fixed by the region, transform and maximum channel count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Layout {
    rows: usize,
    columns: usize,
    pixels: usize,
    values: usize,
}

impl Scratch {
    /// Actual capacities of the strip's heap buffers.
    pub(super) fn bytes(&self) -> usize {
        self.rows.capacity() * size_of::<Row>()
            + (self.upper.capacity() + self.lower.capacity()) * size_of::<Point>()
            + (self.sums.capacity()
                + self.vertical.capacity()
                + self.texels.capacity()
                + self.single.capacity())
                * size_of::<f64>()
    }
}

impl Layout {
    /// Bytes of the strip's allocations.
    pub(super) fn bytes(self) -> usize {
        self.rows
            .saturating_mul(size_of::<Row>())
            .saturating_add(
                self.pixels
                    .saturating_add(1)
                    .saturating_mul(2 * size_of::<Point>()),
            )
            .saturating_add(self.float_values().saturating_mul(size_of::<f64>()))
    }

    fn sum_values(self) -> usize {
        self.rows
            .saturating_mul(self.columns.saturating_mul(2).saturating_add(1))
            .saturating_mul(self.values)
    }

    fn float_values(self) -> usize {
        self.sum_values()
            .saturating_add(
                self.pixels
                    .saturating_add(1)
                    .saturating_mul(self.values + 1),
            )
            .saturating_add(self.columns.saturating_mul(self.values))
            .saturating_add(self.pixels)
    }

    /// Allocates capacities charged by the source layout before execution.
    pub(super) fn make(self) -> Result<Scratch> {
        Ok(Scratch {
            top: 0,
            rows: allocate(self.rows)?,
            sums: allocate(self.sum_values())?,
            upper: allocate(self.pixels.saturating_add(1))?,
            lower: allocate(self.pixels.saturating_add(1))?,
            vertical: allocate(
                self.pixels
                    .saturating_add(1)
                    .saturating_mul(self.values + 1),
            )?,
            texels: allocate(self.columns.saturating_mul(self.values))?,
            single: allocate(self.pixels)?,
        })
    }
}

impl Contour {
    /// The contour sampling of `geometry`'s turned placement over device
    /// columns `columns`, of texels of at most `lanes` values.
    pub(super) fn new(geometry: &Geometry, (x0, x1): (u32, u32)) -> Self {
        let t = geometry.inverse();
        let (width, height) = geometry.size();
        let span = f64::from(x1.saturating_sub(x0)) + 1.0;
        // A strip meets the source rows a device row's preimage spans, and
        // in each at most the columns from its base to its far end.
        let rows = (t.b.abs() * span + t.d.abs() + 3.0).min(f64::from(height));
        let crossing = if t.b == 0.0 {
            f64::INFINITY
        } else {
            (t.d.abs() + 2.0) * (t.a / t.b).abs()
        };
        let columns = (t.a.abs() * span + t.c.abs() + crossing + 3.0).min(f64::from(width) + 1.0);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "finite non-negative counts capped by decoded dimensions"
        )]
        let layout = Layout {
            rows: rows.ceil() as usize,
            columns: columns.ceil() as usize,
            pixels: usize::try_from(x1.saturating_sub(x0)).unwrap_or(usize::MAX),
            values: 0,
        };
        Self {
            inverse: t,
            width,
            height,
            sliver: (t.a * t.d - t.b * t.c).abs() / 4096.0,
            layout,
        }
    }

    /// The mutable storage of the plan.
    pub(super) fn layout(&self, values: usize) -> Layout {
        Layout {
            values: values.min(6),
            ..self.layout
        }
    }

    /// Writes the averages of device pixels `x .. x + len` of row `y`,
    /// `texels.lanes()` values each, into `out`; the alpha and shape of
    /// opaque texels are 1 where a pixel's footprint meets the image and 0
    /// elsewhere.
    pub(super) fn fill(
        &self,
        pixels: Pixels<'_>,
        texels: &impl Texels,
        out: &mut [f64],
        scratch: &mut Scratch,
    ) {
        let lanes = texels.lanes();
        let n = lanes - if texels.opaque() { 2 } else { 0 };
        match n {
            1 => self.span::<1>(pixels, texels, (lanes, out), scratch),
            3 => self.span::<3>(pixels, texels, (lanes, out), scratch),
            4 => self.span::<4>(pixels, texels, (lanes, out), scratch),
            5 => self.span::<5>(pixels, texels, (lanes, out), scratch),
            6 => self.span::<6>(pixels, texels, (lanes, out), scratch),
            _ => self.wide(pixels, texels, (lanes, out), scratch),
        }
    }

    /// [`Self::fill`] for texels of more values than the common ones, one
    /// value of the texels at a time.
    fn wide(
        &self,
        pixels: Pixels<'_>,
        texels: &impl Texels,
        (lanes, out): (usize, &mut [f64]),
        scratch: &mut Scratch,
    ) {
        let one = Lane {
            texels,
            index: 0,
            count: lanes - if texels.opaque() { 2 } else { 0 },
        };
        let len = pixels.len;
        let mut single = std::mem::take(&mut scratch.single);
        single.resize(len, 0.0);
        for lane in 0..one.count {
            let texels = Lane { index: lane, ..one };
            self.span::<1>(pixels, &texels, (1, &mut single), scratch);
            for (pixel, &value) in out.chunks_exact_mut(lanes).zip(&single) {
                if let Some(slot) = pixel.get_mut(lane) {
                    *slot = value;
                }
            }
        }
        // The alpha and shape of opaque texels, as one lane gives them.
        if texels.opaque() {
            let covered = Ones;
            self.span::<1>(pixels, &covered, (1, &mut single), scratch);
            for (pixel, &value) in out.chunks_exact_mut(lanes).zip(&single) {
                if let Some(rest) = pixel.get_mut(lanes - 2..) {
                    rest.fill(value);
                }
            }
        }
        scratch.single = single;
    }

    /// The averages of a span, `N` summed values per pixel written into the
    /// first `N` of every `lanes` values of `out`, the rest (alpha and shape
    /// of opaque texels) 1 or 0.
    fn span<const N: usize>(
        &self,
        pixels: Pixels<'_>,
        texels: &impl Texels,
        (lanes, out): (usize, &mut [f64]),
        scratch: &mut Scratch,
    ) {
        let Pixels { x, y, len, .. } = pixels;
        let t = self.inverse;
        let point = |u: u32, v: u32| Point::new(f64::from(u), f64::from(v)) * t;
        let next = y.saturating_add(1);
        scratch.upper.clear();
        scratch.lower.clear();
        for u in (x..=u32::MAX).take(len + 1) {
            scratch.upper.push(point(u, y));
            scratch.lower.push(point(u, next));
        }
        self.strip::<N>(y, texels, scratch);
        let mut vertical = std::mem::take(&mut scratch.vertical);
        vertical.clear();
        for (&p, &q) in scratch.upper.iter().zip(&scratch.lower) {
            let edge = self.walk::<N>((p, q), scratch);
            vertical.extend_from_slice(&edge[..=N]);
        }
        let stride = N + 1;
        for (j, pixel) in out.chunks_exact_mut(lanes).take(len).enumerate() {
            let (Some(upper), Some(lower)) =
                (scratch.upper.get(j..j + 2), scratch.lower.get(j..j + 2))
            else {
                continue;
            };
            let top = self.walk::<N>((upper[0], upper[1]), scratch);
            let bottom = self.walk::<N>((lower[0], lower[1]), scratch);
            let (Some(left), Some(right)) = (
                vertical.get(j * stride..(j + 1) * stride),
                vertical.get((j + 1) * stride..(j + 2) * stride),
            ) else {
                continue;
            };
            let mut sum = [0.0; 7];
            for (k, value) in sum.iter_mut().enumerate().take(stride) {
                *value = (top[k] + right[k]) - (bottom[k] + left[k]);
            }
            let mut area = sum[N];
            if area < 0.0 {
                area = -area;
                for value in &mut sum[..N] {
                    *value = -*value;
                }
            }
            if area > self.sliver {
                for (out, &value) in pixel.iter_mut().zip(&sum[..N]) {
                    *out = value / area;
                }
                if let Some(rest) = pixel.get_mut(N..) {
                    rest.fill(1.0);
                }
            } else {
                pixel.fill(0.0);
            }
        }
        scratch.vertical = vertical;
    }

    /// Builds the partial sums of the source rows the strip of device row
    /// `y` meets within the span whose grid points the scratch holds.
    fn strip<const N: usize>(&self, y: u32, texels: &impl Texels, scratch: &mut Scratch) {
        let Scratch {
            top,
            rows,
            sums,
            upper,
            lower,
            texels: read,
            ..
        } = scratch;
        rows.clear();
        sums.clear();
        let (Some(&a), Some(&b), Some(&c), Some(&d)) =
            (upper.first(), upper.last(), lower.last(), lower.first())
        else {
            return;
        };
        let corners = [a, b, c, d];
        let (low, high) = corners
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), p| {
                (l.min(p.y), h.max(p.y))
            });
        let height = f64::from(self.height);
        let first = low.floor().clamp(0.0, height);
        let end = high.ceil().clamp(0.0, height);
        *top = coordinate(first);
        let width = f64::from(self.width);
        let mut r = first;
        while r < end {
            let (left, right) = extent(&corners, r);
            let base = self.base(y, r).clamp(0.0, width);
            let start = left.floor().clamp(base, width);
            let stop = right.ceil().clamp(start, width);
            let offset = sums.len();
            let (base_k, first_k, end_k) = (coordinate(base), coordinate(start), coordinate(stop));
            rows.push(Row {
                base,
                first: first_k,
                end: end_k,
                offset,
            });
            // Partial sums from the base, kept from the first column on.
            read.clear();
            read.resize((end_k - base_k) * N, 0.0);
            if end_k > base_k {
                let index = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
                let row = (index(coordinate(r)), self.width);
                texels.row(row, (index(base_k), index(end_k)), read);
            }
            let mut sum = [0.0; N];
            let samples = read.as_chunks::<N>().0;
            for sample in samples.iter().take(first_k - base_k) {
                for (s, v) in sum.iter_mut().zip(sample) {
                    *s += v;
                }
            }
            sums.extend_from_slice(&sum);
            for sample in samples.iter().skip(first_k - base_k) {
                for (s, v) in sum.iter_mut().zip(sample) {
                    *s += v;
                }
                sums.extend_from_slice(&sum);
            }
            sums.extend_from_slice(read.get((first_k - base_k) * N..).unwrap_or(&[]));
            r += 1.0;
        }
    }

    /// The base column of source row `r` in the strip of device row `y`:
    /// the leftmost point of the strip's band in that row, so that partial
    /// sums stay short; 0 where device rows run along source rows.
    fn base(&self, y: u32, r: f64) -> f64 {
        let t = self.inverse;
        if t.b == 0.0 {
            return 0.0;
        }
        let mut left = f64::INFINITY;
        for v in [f64::from(y), f64::from(y) + 1.0] {
            for row in [r, r + 1.0] {
                let u = (row - (t.d * v + t.f)) / t.b;
                left = left.min(t.a * u + t.c * v + t.e);
            }
        }
        left.floor()
    }

    /// The integral of `F dy` along the edge from `p` to `q`, `N` values
    /// and then the area's.
    fn walk<const N: usize>(&self, (p, q): (Point, Point), scratch: &Scratch) -> [f64; 7] {
        let mut out = [0.0; 7];
        let (dx, dy) = (q.x - p.x, q.y - p.y);
        if dy == 0.0 {
            return out;
        }
        let (step_x, step_y) = (1.0 / dx.abs(), 1.0 / dy.abs());
        // The parameters of the grid lines the edge crosses, and the cell
        // it is in, counted as it crosses them.
        let (mut tx, mut k, move_x) = if dx > 0.0 {
            ((p.x.floor() + 1.0 - p.x) * step_x, p.x.floor(), 1.0)
        } else if dx < 0.0 {
            ((p.x - (p.x.ceil() - 1.0)) * step_x, p.x.ceil() - 1.0, -1.0)
        } else {
            (f64::INFINITY, p.x.floor(), 0.0)
        };
        let (mut ty, mut r, move_y) = if dy > 0.0 {
            ((p.y.floor() + 1.0 - p.y) * step_y, p.y.floor(), 1.0)
        } else {
            ((p.y - (p.y.ceil() - 1.0)) * step_y, p.y.ceil() - 1.0, -1.0)
        };
        let top = scratch.top as f64;
        let width = f64::from(self.width);
        let mut t0 = 0.0;
        loop {
            let t1 = tx.min(ty).min(1.0);
            if let Some(row) = row_at(scratch, r - top) {
                let h = (t1 - t0) * dy;
                let x = p.x + f64::midpoint(t0, t1) * dx;
                let (sum, ones) = value::<N>(row, (k, x, width), &scratch.sums);
                for (out, value) in out.iter_mut().zip(sum) {
                    *out += value * h;
                }
                out[N] += ones * h;
            }
            if t1 >= 1.0 {
                break;
            }
            if tx <= ty {
                tx += step_x;
                k += move_x;
            } else {
                ty += step_y;
                r += move_y;
            }
            t0 = t1;
        }
        out
    }
}

/// The strip row `index` rows below its first, if the strip has it.
#[inline]
fn row_at(scratch: &Scratch, index: f64) -> Option<&Row> {
    if index < 0.0 {
        return None;
    }
    scratch.rows.get(coordinate(index))
}

/// `F` at `x` in cell `k` of `row` (`N` values, from its sums), and the
/// area's `F`: the distance from the base, within the image.
#[inline]
fn value<const N: usize>(
    row: &Row,
    (k, x, width): (f64, f64, f64),
    sums: &[f64],
) -> ([f64; N], f64) {
    let mut out = [0.0; N];
    if k < 0.0 || row.end <= row.first && x < row.base {
        return (out, 0.0);
    }
    let columns = row.end - row.first;
    if k >= width || columns == 0 {
        // Right of the image, or a row the strip meets only at its edge.
        let at = row.offset + columns * N;
        if let Some(sum) = sums.get(at..at + N) {
            out.copy_from_slice(sum);
        }
        let x = x.clamp(row.base, width);
        return (
            out,
            if k >= width {
                width - row.base
            } else {
                x - row.base
            },
        );
    }
    let cell = coordinate(k).clamp(row.first, row.end - 1) - row.first;
    let fraction = x - (cell + row.first) as f64;
    let sum = row.offset + cell * N;
    let value = row.offset + (columns + 1) * N + cell * N;
    if let (Some(sum), Some(value)) = (sums.get(sum..sum + N), sums.get(value..value + N)) {
        for ((out, &s), &v) in out.iter_mut().zip(sum).zip(value) {
            *out = s + fraction * v;
        }
    }
    (out, x - row.base)
}

/// The leftmost and rightmost `x` of the convex polygon `corners` within
/// rows `r..r + 1`; an empty range where it does not reach them.
fn extent(corners: &[Point; 4], r: f64) -> (f64, f64) {
    let (mut left, mut right) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut take = |x: f64| {
        left = left.min(x);
        right = right.max(x);
    };
    for (i, &p) in corners.iter().enumerate() {
        let q = corners[(i + 1) % 4];
        if (r..=r + 1.0).contains(&p.y) {
            take(p.x);
        }
        for bound in [r, r + 1.0] {
            if (p.y - bound) * (q.y - bound) < 0.0 {
                take(p.x + (bound - p.y) / (q.y - p.y) * (q.x - p.x));
            }
        }
    }
    (left, right)
}

/// A non-negative sample coordinate as an index.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "coordinates clamped to the image's u32 extent"
)]
fn coordinate(value: f64) -> usize {
    value.max(0.0) as usize
}

/// One value of each texel of `texels`, as a source of one value.
#[derive(Clone, Copy)]
struct Lane<'a, T> {
    texels: &'a T,
    index: usize,
    count: usize,
}

impl<T: Texels> Texels for Lane<'_, T> {
    fn lanes(&self) -> usize {
        1
    }

    fn get(&self, index: usize, out: &mut [f64]) {
        let mut texel = [0.0; super::geometry::MAX_LANES];
        let texel = &mut texel[..self.count];
        self.texels.get(index, texel);
        if let (Some(out), Some(&value)) = (out.first_mut(), texel.get(self.index)) {
            *out = value;
        }
    }

    fn add(&self, index: usize, weight: f64, sum: &mut [f64]) {
        let mut value = [0.0];
        self.get(index, &mut value);
        if let Some(sum) = sum.first_mut() {
            *sum += value[0] * weight;
        }
    }
}

/// Texels of one value 1, whose average is 1 where a footprint meets the
/// image.
struct Ones;

impl Texels for Ones {
    fn lanes(&self) -> usize {
        1
    }

    fn get(&self, _: usize, out: &mut [f64]) {
        out.fill(1.0);
    }

    fn add(&self, _: usize, weight: f64, sum: &mut [f64]) {
        if let Some(sum) = sum.first_mut() {
            *sum += weight;
        }
    }
}
