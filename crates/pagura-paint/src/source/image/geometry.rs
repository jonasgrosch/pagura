//! Where a device pixel samples an image (8.9.5.3, ADR 0004): the nearest
//! sample of its centre, or with `Interpolate` the bilinear weights of the
//! four samples around it, when the image is not minified; otherwise the
//! exact area average of the source samples under the pixel's footprint.
//! Area averages use the prepared separable or contour plan in
//! [`super::sampling`]; nearest and bilinear pixels are evaluated here.
//!
//! Every weighted sum adds texels in a fixed order with the same f64
//! operations for every partition of the device grid, so a pixel's value
//! does not depend on which span asks for it.

use crate::source::Pixels;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Result};
use pagura_raster::{Point, Transform};

/// The most values one texel holds: colour channels, alpha and shape.
pub(super) const MAX_LANES: usize = pagura_color::MAX_CHANNELS + 3;

/// The most the cross terms of a near-upright placement may move a sample
/// position, as a part of a sample and of a pixel's footprint, for it to be
/// sampled upright ([`Geometry::near_upright`]).
const NEAR_UPRIGHT: f64 = 1.0 / 4096.0;

/// A source of texels, each `lanes` values that sum with weights: colour
/// channels, alpha and shape.
pub(super) trait Texels {
    /// Values per texel.
    fn lanes(&self) -> usize;

    /// Whether every texel's alpha and shape are 1. Then [`Self::get`] and
    /// [`Self::add`] handle the colour lanes alone, and the geometry sums
    /// alpha and shape from the weights, which gives the same values.
    fn opaque(&self) -> bool {
        false
    }

    /// Writes the texel of sample `index` (row-major, clamped by the
    /// geometry to the image) into `out`.
    fn get(&self, index: usize, out: &mut [f64]);

    /// Adds `weight` times the texel of sample `index` to `sum`.
    fn add(&self, index: usize, weight: f64, sum: &mut [f64]);

    /// Writes the texels of samples `first..end` of row `row` of an image
    /// `width` samples wide into `out`, as [`Self::get`] gives them, an
    /// equal share of `out` each.
    fn row(&self, row: (u32, u32), cells: (u32, u32), out: &mut [f64]) {
        sample_row(self, row, cells, out);
    }
}

/// Writes the texels of samples `first..end` of row `row` of an image
/// `width` samples wide into `out`, one [`Texels::get`] each.
pub(super) fn sample_row<T: Texels + ?Sized>(
    texels: &T,
    (row, width): (u32, u32),
    (first, end): (u32, u32),
    out: &mut [f64],
) {
    let count = usize::try_from(end.saturating_sub(first)).unwrap_or(0);
    let Some(stride) = out.len().checked_div(count).filter(|&n| n > 0) else {
        return;
    };
    let base = u64::from(row) * u64::from(width);
    for (cell, values) in (first..end).zip(out.chunks_exact_mut(stride)) {
        texels.get(sample_index(base + u64::from(cell)), values);
    }
}

/// How device pixels map to source samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// The sample under the pixel centre.
    Nearest,
    /// Bilinear weights of the four samples around the pixel centre.
    Bilinear,
    /// The exact area average over the pixel's quadrilateral footprint.
    Area,
}

/// The placement of one image's samples on the device grid.
#[derive(Debug, Clone)]
pub(super) struct Geometry {
    width: u32,
    height: u32,
    /// Device pixels to sample space, where the image's unit square spans
    /// its extent in stored samples.
    inverse: Transform,
    mode: Mode,
    interpolate: bool,
}

/// The source cells under one device pixel along one axis: `first..end`,
/// with fractional end weights, unit interior weights and the reciprocal
/// extent.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Footprint {
    pub(super) first: u32,
    pub(super) end: u32,
    pub(super) first_weight: f64,
    pub(super) last_weight: f64,
    pub(super) normalization: f64,
}

impl Footprint {
    /// The cell under the centre of device pixel `pixel` along an axis that
    /// maps pixels by `scale` and `offset` onto `0..maximum` cells, whole.
    pub(super) fn nearest(pixel: u32, scale: f64, offset: f64, maximum: u32) -> Self {
        let center = (f64::from(pixel) + 0.5) * scale + offset;
        let first = coordinate(center.floor(), maximum.saturating_sub(1));
        Self {
            first,
            end: first + 1,
            first_weight: 1.0,
            last_weight: 1.0,
            normalization: 1.0,
        }
    }

    /// The cells under device pixel `pixel` along an axis that maps pixels
    /// by `scale` and `offset` onto `0..maximum` cells.
    pub(super) fn new(pixel: u32, scale: f64, offset: f64, maximum: u32) -> Self {
        let a = f64::from(pixel) * scale + offset;
        let b = (f64::from(pixel) + 1.0) * scale + offset;
        let low = a.min(b).clamp(0.0, f64::from(maximum));
        let high = a.max(b).clamp(0.0, f64::from(maximum));
        let first = coordinate(low.floor(), maximum);
        let end = coordinate(if high > low { high.ceil() } else { low.floor() }, maximum);
        let weight =
            |cell: u32| (high.min(f64::from(cell) + 1.0) - low.max(f64::from(cell))).max(0.0);
        Self {
            first,
            end,
            first_weight: weight(first),
            last_weight: weight(end.saturating_sub(1)),
            normalization: if high > low { 1.0 / (high - low) } else { 0.0 },
        }
    }

    /// The two cells around the centre of device pixel `pixel` along an
    /// axis that maps pixel centres by `scale` and `offset` onto `0..maximum`
    /// cells, with their bilinear weights, as [`Geometry::fill`] weights
    /// them; the cells merge where the image edge clamps them.
    pub(super) fn bilinear(pixel: u32, scale: f64, offset: f64, maximum: u32) -> Self {
        let position = (f64::from(pixel) + 0.5) * scale + offset - 0.5;
        let left = position.floor();
        let fraction = position - left;
        let last = maximum.saturating_sub(1);
        let (first, second) = (coordinate(left, last), coordinate(left + 1.0, last));
        let (first_weight, last_weight) = (1.0 - fraction, fraction);
        if first == second {
            let weight = first_weight + last_weight;
            return Self {
                first,
                end: first + 1,
                first_weight: weight,
                last_weight: weight,
                normalization: 1.0 / weight,
            };
        }
        Self {
            first,
            end: first + 2,
            first_weight,
            last_weight,
            normalization: 1.0 / (first_weight + last_weight),
        }
    }

    /// The footprint without end samples that carry less than 2^-40 of its
    /// weight, and with end weights that fall short of a whole sample by
    /// less than that taken whole: each moves the average by less than
    /// 2^-40 of the colour range, far below the quantizer's tie guard, where
    /// floating-point error in the transform otherwise makes a footprint of
    /// two whole samples touch four with fractional weights.
    #[must_use]
    pub(super) fn trimmed(mut self) -> Self {
        const NEGLIGIBLE: f64 = 1.0 / 1_099_511_627_776.0;
        let whole = |weight: f64, normalization: f64| {
            if (1.0 - weight).abs() * normalization < NEGLIGIBLE {
                1.0
            } else {
                weight
            }
        };
        if self.end - self.first > 1 && self.first_weight * self.normalization < NEGLIGIBLE {
            self.first += 1;
            self.first_weight = if self.end - self.first == 1 {
                self.last_weight
            } else {
                1.0
            };
        }
        if self.end - self.first > 1 && self.last_weight * self.normalization < NEGLIGIBLE {
            self.end -= 1;
            self.last_weight = if self.end - self.first == 1 {
                self.first_weight
            } else {
                1.0
            };
        }
        if self.end - self.first > 1 {
            self.first_weight = whole(self.first_weight, self.normalization);
            self.last_weight = whole(self.last_weight, self.normalization);
        }
        self
    }

    /// Whether every cell has weight one.
    pub(super) fn whole(&self) -> bool {
        self.first_weight == 1.0 && self.last_weight == 1.0
    }
}

impl Geometry {
    /// The geometry of an image of `width` × `height` samples, its unit
    /// square placed by `unit_to_device` (whose inverse is `inverse`).
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when device coordinates do not map to finite
    /// sample coordinates.
    pub(super) fn new(
        (width, height): (u32, u32),
        interpolate: bool,
        inverse: Transform,
    ) -> Result<Self> {
        let extent = (f64::from(width), f64::from(height));
        if !reaches(inverse, extent) {
            return Err(Error::limit("image sampling transform overflow"));
        }
        let inverse = inverse.then(Transform::new(extent.0, 0.0, 0.0, -extent.1, 0.0, extent.1));
        Ok(Self {
            width,
            height,
            inverse,
            mode: mode(inverse, interpolate),
            interpolate,
        })
    }

    /// The geometry of the same placement over the image with rows and
    /// columns exchanged ([`pagura_image::DecodedImage::transposed`]): a
    /// quarter turn becomes upright. Sample coordinates are exchanged
    /// exactly, so every footprint and weight stays the same.
    pub(super) fn transposed(&self) -> Self {
        let inverse = self
            .inverse
            .then(Transform::new(0.0, 1.0, 1.0, 0.0, 0.0, 0.0));
        Self {
            width: self.height,
            height: self.width,
            inverse,
            mode: mode(inverse, self.interpolate),
            interpolate: self.interpolate,
        }
    }

    /// The same placement upright, when its cross terms are noise, and the
    /// part of the full range by which that may move any texel lane's
    /// value. Each cross term is dropped, with its contribution at the
    /// centre of the image's device extent kept in the offsets, so a
    /// sample position a pixel of the image reaches moves by at most
    /// `across` samples along the rows and `down` along the columns. An
    /// area average over a footprint `|a|` samples wide then moves by at
    /// most `across / |a|` of its lane's range, and a bilinear value by at
    /// most `across` of it; `across / min(|a|, 1)` bounds both, and the
    /// part is that plus the same for `down`. Every lane's range is within
    /// the full range: converted colour, alpha and shape. Placements whose
    /// terms move positions by more than [`NEAR_UPRIGHT`] of a sample or
    /// footprint, or that are sampled nearest (a move of any size can
    /// select another sample), keep the exact placement. Whether a part is
    /// admissible is the target buffer's storage decision (ADR 0004 §3).
    /// `None` where the placement is upright already or not near it.
    pub(super) fn near_upright(&self) -> Option<(Self, f64)> {
        let t = self.inverse;
        if t.b == 0.0 && t.c == 0.0 || self.mode == Mode::Nearest {
            return None;
        }
        // The image's device extent, with a pixel around it for the
        // footprints that reach into it.
        let forward = t.inverse()?;
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        let corners =
            [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(x, y)| Point::new(x, y) * forward);
        let (low, high) = corners.iter().fold(
            (
                Point::new(f64::INFINITY, f64::INFINITY),
                Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
            ),
            |(low, high), p| {
                (
                    Point::new(low.x.min(p.x), low.y.min(p.y)),
                    Point::new(high.x.max(p.x), high.y.max(p.y)),
                )
            },
        );
        let centre = Point::new(f64::midpoint(low.x, high.x), f64::midpoint(low.y, high.y));
        let reach = Point::new((high.x - low.x) / 2.0 + 1.0, (high.y - low.y) / 2.0 + 1.0);
        // Sample columns move by `c` per device row and rows by `b` per
        // device column.
        let across = t.c.abs() * reach.y;
        let down = t.b.abs() * reach.x;
        let (along, over) = (across / t.a.abs().min(1.0), down / t.d.abs().min(1.0));
        if !(along <= NEAR_UPRIGHT && over <= NEAR_UPRIGHT) {
            return None;
        }
        let inverse = Transform::new(
            t.a,
            0.0,
            0.0,
            t.d,
            t.e + t.c * centre.y,
            t.f + t.b * centre.x,
        );
        let mode = mode(inverse, self.interpolate);
        (mode == self.mode).then(|| {
            let upright = Self {
                inverse,
                mode,
                ..self.clone()
            };
            (upright, along + over)
        })
    }

    /// How device pixels map to samples.
    pub(super) fn mode(&self) -> Mode {
        self.mode
    }

    /// Device pixels to sample space.
    pub(super) fn inverse(&self) -> Transform {
        self.inverse
    }

    /// Stored samples per row and rows.
    pub(super) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// An upper bound of the texels one device pixel reads.
    pub(super) fn cost(&self) -> usize {
        let t = self.inverse;
        let cells = |extent: f64, bound: u32| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a finite, non-negative extent clamped to the image's samples"
            )]
            let cells = extent.ceil().min(f64::from(bound)) as usize;
            cells.saturating_add(2)
        };
        match self.mode {
            Mode::Nearest => 1,
            Mode::Bilinear => 4,
            Mode::Area => cells(t.a.abs() + t.c.abs(), self.width)
                .saturating_mul(cells(t.b.abs() + t.d.abs(), self.height)),
        }
    }

    /// The texels a bilinear span over device `columns` reads from each of
    /// its two source rows when the placement is upright, which
    /// [`Self::fill`] then reads once a span; 0 otherwise.
    pub(super) fn row_cells(&self, (x0, x1): (u32, u32)) -> usize {
        let t = self.inverse;
        if self.mode != Mode::Bilinear || t.b != 0.0 || t.c != 0.0 {
            return 0;
        }
        // A pixel's left tap moves at most |a| cells a column, and the
        // right tap adds one; `reaches` keeps the extent finite.
        let cells = (t.a.abs() * f64::from(x1.saturating_sub(x0))).ceil() + 2.0;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a finite, non-negative count capped by the image width"
        )]
        let cells = cells.min(f64::from(self.width)) as usize;
        cells
    }

    /// Writes the values of device pixels `x .. x + len` of row `y`, each
    /// `texels.lanes()` values, into `out`; `rows` keeps the source rows
    /// of upright bilinear spans.
    pub(super) fn fill(
        &self,
        pixels: Pixels<'_>,
        texels: &impl Texels,
        out: &mut [f64],
        rows: &mut BilinearRows,
    ) {
        let Pixels { x, y, len, .. } = pixels;
        let lanes = texels.lanes();
        let opaque = texels.opaque();
        match self.mode {
            Mode::Nearest => {
                for (value, px) in out.chunks_exact_mut(lanes).take(len).zip(x..) {
                    let (color, rest) = value.split_at_mut(lanes - usize::from(opaque) * 2);
                    texels.get(self.nearest(px, y), color);
                    rest.fill(1.0);
                }
            }
            Mode::Bilinear => {
                if !self.bilinear_rows(pixels, texels, out, rows) {
                    for (value, px) in out.chunks_exact_mut(lanes).take(len).zip(x..) {
                        self.bilinear(px, y, texels, value);
                    }
                }
            }
            // Minified placements average through their separable or
            // contour plans.
            Mode::Area => {}
        }
    }

    /// The source position of a device pixel's centre.
    fn center(&self, x: u32, y: u32) -> Point {
        Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5) * self.inverse
    }

    fn nearest(&self, x: u32, y: u32) -> usize {
        let center = self.center(x, y);
        self.index(center.x.floor(), center.y.floor())
    }

    /// [`Self::bilinear`] for a span of an upright placement. The inverse's
    /// cross terms are signed zeros, so a pixel centre's sample `y` is
    /// `±0 + d·y + f`, the same for every column (and its floor and fraction
    /// with it), while its `x` depends on the column alone. Every pixel thus
    /// takes its taps from the same two source rows, read once through
    /// `rows`, and, rounding being monotone, from the columns between those
    /// of the first and last pixel. Each tap adds its texel as
    /// [`Texels::add`] does, in the same order, so every pixel has the bits
    /// of [`Self::bilinear`]. False, with nothing written, where `rows`
    /// cannot hold the span's rows.
    fn bilinear_rows(
        &self,
        Pixels { x, y, len, .. }: Pixels<'_>,
        texels: &impl Texels,
        out: &mut [f64],
        rows: &mut BilinearRows,
    ) -> bool {
        let t = self.inverse;
        if t.b != 0.0 || t.c != 0.0 || len == 0 {
            return false;
        }
        let lanes = texels.lanes();
        let n = lanes - usize::from(texels.opaque()) * 2;
        let right = self.width.saturating_sub(1);
        let last = x.saturating_add(u32::try_from(len - 1).unwrap_or(u32::MAX));
        let (first, last) = (self.center(x, y), self.center(last, y));
        let (a, b) = ((first.x - 0.5).floor(), (last.x - 0.5).floor());
        let begin = coordinate(a.min(b), right);
        let end = coordinate(a.max(b) + 1.0, right) + 1;
        let top = (first.y - 0.5).floor();
        let lines = [top, top + 1.0].map(|line| coordinate(line, self.height.saturating_sub(1)));
        let Some((upper, lower)) = rows.read(texels, (lines, self.width), (begin, end), n) else {
            return false;
        };
        let span = RowTaps {
            geometry: self,
            y,
            fy: (first.y - 0.5) - top,
            begin,
            rows: (upper, lower),
        };
        let out = out.chunks_exact_mut(lanes).take(len).zip(x..);
        match n {
            1 => span.fill::<1>(out, texels, n),
            3 => span.fill::<3>(out, texels, n),
            4 => span.fill::<4>(out, texels, n),
            5 => span.fill::<5>(out, texels, n),
            6 => span.fill::<6>(out, texels, n),
            _ => span.fill::<0>(out, texels, n),
        }
        true
    }

    fn bilinear(&self, x: u32, y: u32, texels: &impl Texels, out: &mut [f64]) {
        let center = self.center(x, y);
        let (x, y) = (center.x - 0.5, center.y - 0.5);
        let (left, top) = (x.floor(), y.floor());
        let (fx, fy) = (x - left, y - top);
        out.fill(0.0);
        let (color, rest) = split(out, texels.opaque());
        let mut alpha = 0.0;
        for (x, y, w) in [
            (left, top, (1.0 - fx) * (1.0 - fy)),
            (left + 1.0, top, fx * (1.0 - fy)),
            (left, top + 1.0, (1.0 - fx) * fy),
            (left + 1.0, top + 1.0, fx * fy),
        ] {
            texels.add(self.index(x, y), w, color);
            alpha += w;
        }
        rest.fill(alpha);
    }

    fn index(&self, x: f64, y: f64) -> usize {
        let x = coordinate(x, self.width.saturating_sub(1));
        let y = coordinate(y, self.height.saturating_sub(1));
        sample_index(u64::from(y) * u64::from(self.width) + u64::from(x))
    }
}

/// The source rows upright bilinear spans read, kept for the next span:
/// two slots of half the capacity each.
#[derive(Debug, Default)]
pub(super) struct BilinearRows {
    values: Vec<f64>,
    kept: [Option<Kept>; 2],
}

/// The texels a slot holds: source row `line`, columns `begin..end`, `n`
/// values each.
#[derive(Debug, Clone, Copy)]
struct Kept {
    line: u32,
    begin: u32,
    end: u32,
    n: usize,
}

impl BilinearRows {
    /// Rows in `values`, whose capacity the caller has charged.
    pub(super) fn new(values: Vec<f64>) -> Self {
        Self {
            values,
            kept: [None; 2],
        }
    }

    pub(super) fn bytes(&self) -> usize {
        self.values.capacity() * size_of::<f64>()
    }

    /// Forgets the rows read, before the storage serves another render.
    pub(super) fn reset(&mut self) {
        self.kept = [None; 2];
    }

    /// Whether a span has read rows.
    #[cfg(test)]
    pub(super) fn read_any(&self) -> bool {
        self.kept.iter().any(Option::is_some)
    }

    /// The texels of the source rows `lines` of an image `width` samples
    /// wide over `cells`, `n` values each: from the slots that hold them, or
    /// read into the others. `None` where a slot cannot hold them.
    fn read(
        &mut self,
        texels: &impl Texels,
        (lines, width): ([u32; 2], u32),
        (begin, end): (u32, u32),
        n: usize,
    ) -> Option<(&[f64], &[f64])> {
        let slot = self.values.capacity() / 2;
        let values = usize::try_from(end - begin).ok()?.checked_mul(n)?;
        if values > slot {
            return None;
        }
        let holds = |kept: Option<Kept>, line: u32| {
            kept.is_some_and(|k| k.line == line && k.n == n && k.begin <= begin && end <= k.end)
        };
        let mut found = lines.map(|line| (0..2).find(|&s| holds(self.kept[s], line)));
        let all = super::scratch::grown(&mut self.values, 2 * slot);
        for i in 0..2 {
            if found[i].is_some() {
                continue;
            }
            // The slot the other row does not take.
            let s = found[1 - i].map_or(0, |other| 1 - other);
            texels.row(
                (lines[i], width),
                (begin, end),
                &mut all[s * slot..s * slot + values],
            );
            self.kept[s] = Some(Kept {
                line: lines[i],
                begin,
                end,
                n,
            });
            found[i] = Some(s);
            if lines[1 - i] == lines[i] {
                found[1 - i] = Some(s);
            }
        }
        let row = |s: Option<usize>| {
            let s = s.unwrap_or(0);
            let from = self.kept[s].map_or(0, |k| u32_to_usize(begin - k.begin) * n);
            let at = s * slot + from;
            all.get(at..at + values).unwrap_or(&[])
        };
        Some((row(found[0]), row(found[1])))
    }
}

/// The two source rows of an upright bilinear span, read from column
/// `begin`, and the fraction of the row its pixel centres share.
struct RowTaps<'a> {
    geometry: &'a Geometry,
    y: u32,
    fy: f64,
    begin: u32,
    rows: (&'a [f64], &'a [f64]),
}

impl<'a> RowTaps<'a> {
    /// Writes the pixels of `out`, texels of `N` values (`n` for 0) summed
    /// from the rows; a pixel with a tap outside them takes
    /// [`Geometry::bilinear`].
    fn fill<'o, const N: usize>(
        &self,
        out: impl Iterator<Item = (&'o mut [f64], u32)>,
        texels: &impl Texels,
        n: usize,
    ) {
        let n = if N == 0 { n } else { N };
        let geometry = self.geometry;
        let (upper, lower) = self.rows;
        let right = geometry.width.saturating_sub(1);
        let texel = |row: &'a [f64], column: f64| {
            let cell = usize::try_from(coordinate(column, right).checked_sub(self.begin)?).ok()?;
            row.get(cell * n..(cell + 1) * n)
        };
        let fy = self.fy;
        for (value, px) in out {
            let sx = geometry.center(px, self.y).x - 0.5;
            let left = sx.floor();
            let fx = sx - left;
            let weights = [
                (1.0 - fx) * (1.0 - fy),
                fx * (1.0 - fy),
                (1.0 - fx) * fy,
                fx * fy,
            ];
            let taps = [
                texel(upper, left),
                texel(upper, left + 1.0),
                texel(lower, left),
                texel(lower, left + 1.0),
            ];
            let [Some(t0), Some(t1), Some(t2), Some(t3)] = taps else {
                geometry.bilinear(px, self.y, texels, value);
                continue;
            };
            let (color, rest) = value.split_at_mut(n);
            for ((((sum, v0), v1), v2), v3) in color.iter_mut().zip(t0).zip(t1).zip(t2).zip(t3) {
                let mut value = 0.0 + v0 * weights[0];
                value += v1 * weights[1];
                value += v2 * weights[2];
                value += v3 * weights[3];
                *sum = value;
            }
            let alpha = 0.0 + weights[0] + weights[1] + weights[2] + weights[3];
            for slot in rest {
                *slot = alpha;
            }
        }
    }
}

/// Whether every device pixel corner within the u32 grid maps finitely into
/// the sample space of an image whose unit square spans `extent` samples
/// under the device-to-unit-square `inverse`, so footprints never overflow
/// while painting. A placement that fails it is narrower than 2^-290 of a
/// pixel along one axis, and covers no visible part of any pixel.
pub(super) fn reaches(inverse: Transform, extent: (f64, f64)) -> bool {
    let inverse = inverse.then(Transform::new(extent.0, 0.0, 0.0, -extent.1, 0.0, extent.1));
    let reach = f64::from(u32::MAX) + 1.0;
    [inverse.a, inverse.b, inverse.c, inverse.d]
        .iter()
        .map(|v| v.abs() * reach)
        .chain([inverse.e.abs(), inverse.f.abs()])
        .fold(0.0, |sum: f64, v| sum + v)
        .is_finite()
}

/// How a placement whose device pixels map by `t` to sample space samples.
fn mode(t: Transform, interpolate: bool) -> Mode {
    let minifying = t.a.hypot(t.b) > 1.0 + 1e-9 || t.c.hypot(t.d) > 1.0 + 1e-9;
    if minifying {
        Mode::Area
    } else if interpolate {
        Mode::Bilinear
    } else {
        Mode::Nearest
    }
}

/// The colour lanes of a texel sum and, for opaque texels, the alpha and
/// shape lanes the geometry fills.
fn split(out: &mut [f64], opaque: bool) -> (&mut [f64], &mut [f64]) {
    let colour = out.len() - if opaque { 2 } else { 0 };
    out.split_at_mut(colour)
}

/// A row-major sample index; decoded images are addressable, so it fits.
pub(super) fn sample_index(index: u64) -> usize {
    usize::try_from(index).unwrap_or(usize::MAX)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sample coordinates clamped to u32 image bounds"
)]
fn coordinate(value: f64, maximum: u32) -> u32 {
    value.clamp(0.0, f64::from(maximum)) as u32
}
