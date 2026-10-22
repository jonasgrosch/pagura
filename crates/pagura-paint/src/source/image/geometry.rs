//! Where a device pixel samples an image (8.9.5.3, ADR 0004): the nearest
//! sample of its centre, or with `Interpolate` the bilinear weights of the
//! four samples around it, when the image is not minified; otherwise the
//! exact area average of the source samples under the pixel's footprint.
//! Upright placements sample separably ([`super::separable`]); any other
//! pixel by itself here, its area average by polygon clipping.
//!
//! Every weighted sum adds texels in a fixed order with the same f64
//! operations for every partition of the device grid, so a pixel's value
//! does not depend on which span asks for it.

use pagura_core::{Error, Result};
use pagura_raster::{Point, Transform};

/// The most values one texel holds: colour channels, alpha and shape.
pub(super) const MAX_LANES: usize = pagura_color::MAX_CHANNELS + 2;

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
    /// The exact area average over separable footprints of an upright
    /// placement.
    Separable,
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
            Mode::Separable | Mode::Area => cells(t.a.abs() + t.c.abs(), self.width)
                .saturating_mul(cells(t.b.abs() + t.d.abs(), self.height)),
        }
    }

    /// Writes the values of device pixels `x .. x + len` of row `y`, each
    /// `texels.lanes()` values, into `out`.
    pub(super) fn fill(
        &self,
        (x, y, len): (u32, u32, usize),
        texels: &impl Texels,
        out: &mut [f64],
    ) {
        let lanes = texels.lanes();
        let opaque = texels.opaque();
        let pixels = out.chunks_exact_mut(lanes).take(len);
        match self.mode {
            Mode::Nearest => {
                for (value, px) in pixels.zip(x..) {
                    let (color, rest) = value.split_at_mut(lanes - usize::from(opaque) * 2);
                    texels.get(self.nearest(px, y), color);
                    rest.fill(1.0);
                }
            }
            Mode::Bilinear => {
                for (value, px) in pixels.zip(x..) {
                    self.bilinear(px, y, texels, value);
                }
            }
            // Minified placements average through their separable or
            // contour plans.
            Mode::Area | Mode::Separable => {}
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
    if minifying && t.b == 0.0 && t.c == 0.0 {
        Mode::Separable
    } else if minifying {
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
