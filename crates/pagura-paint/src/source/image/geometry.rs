//! Where a device pixel samples an image (8.9.5.3, ADR 0004): the nearest
//! sample of its centre, or with `Interpolate` the bilinear weights of the
//! four samples around it, when the image is not minified; otherwise the
//! exact area average of the source samples under the pixel's footprint,
//! separably for upright placements and quarter turns, by polygon clipping
//! for any other.
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

    /// Adds the texel of sample `index` to `sum`, as [`Self::add`] with a
    /// weight of exactly 1 does.
    fn add_unit(&self, index: usize, sum: &mut [f64]) {
        self.add(index, 1.0, sum);
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
    /// The exact area average over separable footprints: device columns
    /// step along source columns (or rows, when `swapped` by a quarter turn).
    Separable {
        /// Device columns step along source rows.
        swapped: bool,
    },
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
    /// weight: they move the average by less than 2^-40 of the colour
    /// range, far below the quantizer's tie guard, where floating-point
    /// error in the transform otherwise makes a footprint of two samples
    /// touch four.
    #[must_use]
    pub(super) fn trimmed(mut self) -> Self {
        const NEGLIGIBLE: f64 = 1.0 / 1_099_511_627_776.0;
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
        self
    }

    fn weight(self, cell: u32) -> f64 {
        if cell == self.first {
            self.first_weight
        } else if cell.wrapping_add(1) == self.end {
            self.last_weight
        } else {
            1.0
        }
    }
}

impl Geometry {
    /// The geometry of an image of `width` × `height` stored samples whose
    /// unit square spans `extent` samples, placed by `unit_to_device`
    /// (whose inverse is `inverse`).
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when device coordinates do not map to finite
    /// sample coordinates.
    pub(super) fn new(
        (width, height): (u32, u32),
        extent: (f64, f64),
        interpolate: bool,
        inverse: Transform,
    ) -> Result<Self> {
        let inverse = inverse.then(Transform::new(extent.0, 0.0, 0.0, -extent.1, 0.0, extent.1));
        // Every device pixel corner within the u32 grid maps finitely, so
        // footprints never overflow while painting.
        let reach = f64::from(u32::MAX) + 1.0;
        let finite = [inverse.a, inverse.b, inverse.c, inverse.d]
            .iter()
            .map(|v| v.abs() * reach)
            .chain([inverse.e.abs(), inverse.f.abs()])
            .fold(0.0, |sum: f64, v| sum + v)
            .is_finite();
        if !finite {
            return Err(Error::limit("image sampling transform overflow"));
        }
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

    /// The column and row of the sample under the centre of device pixel
    /// (`x`, `y`), as [`Self::fill`] finds it without interpolation.
    pub(super) fn nearest_cell(&self, x: u32, y: u32) -> (u32, u32) {
        let center = self.center(x, y);
        (
            coordinate(center.x.floor(), self.width.saturating_sub(1)),
            coordinate(center.y.floor(), self.height.saturating_sub(1)),
        )
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
            Mode::Separable { .. } | Mode::Area => cells(t.a.abs() + t.c.abs(), self.width)
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
            Mode::Area => {
                for (value, px) in pixels.zip(x..) {
                    self.area(px, y, texels, value);
                }
            }
            Mode::Separable { swapped } => {
                let t = self.inverse;
                let (scale, offset, bound) = if swapped {
                    (t.c, t.e, self.width)
                } else {
                    (t.d, t.f, self.height)
                };
                let rows = Footprint::new(y, scale, offset, bound);
                let mut line = [0.0; MAX_LANES];
                let line = &mut line[..lanes];
                for (value, px) in pixels.zip(x..) {
                    self.reduce((px, rows, swapped), texels, value, line);
                }
            }
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

    fn area(&self, x: u32, y: u32, texels: &impl Texels, out: &mut [f64]) {
        let (x, y) = (f64::from(x), f64::from(y));
        let polygon = [
            Point::new(x, y),
            Point::new(x + 1.0, y),
            Point::new(x + 1.0, y + 1.0),
            Point::new(x, y + 1.0),
        ]
        .map(|p| p * self.inverse);
        let (low, high) = polygon
            .iter()
            .fold((polygon[0], polygon[0]), |(low, high), p| {
                (
                    Point::new(low.x.min(p.x), low.y.min(p.y)),
                    Point::new(high.x.max(p.x), high.y.max(p.y)),
                )
            });
        let x0 = coordinate(low.x.floor(), self.width);
        let y0 = coordinate(low.y.floor(), self.height);
        let x1 = coordinate(high.x.ceil(), self.width);
        let y1 = coordinate(high.y.ceil(), self.height);
        out.fill(0.0);
        let (color, rest) = split(out, texels.opaque());
        let mut weight = 0.0;
        for row in y0..y1 {
            for col in x0..x1 {
                let area = intersection(&polygon, f64::from(col), f64::from(row));
                if area > 0.0 {
                    texels.add(self.index(f64::from(col), f64::from(row)), area, color);
                    weight += area;
                }
            }
        }
        // An opaque texel's alpha sums exactly as the weights do.
        rest.fill(weight);
        if weight > 0.0 {
            let scale = 1.0 / weight;
            for value in out.iter_mut() {
                *value *= scale;
            }
        }
    }

    /// The separable area average: each source line under the pixel's
    /// column footprint, normalized, then the lines weighted by the row
    /// footprint, in the order the lines are reached.
    fn reduce(
        &self,
        (x, rows, swapped): (u32, Footprint, bool),
        texels: &impl Texels,
        out: &mut [f64],
        line: &mut [f64],
    ) {
        let t = self.inverse;
        let (scale, offset, maximum) = if swapped {
            (t.b, t.f, self.height)
        } else {
            (t.a, t.e, self.width)
        };
        let columns = Footprint::new(x, scale, offset, maximum);
        let descending = (if swapped { t.c } else { t.d }) < 0.0;
        let opaque = texels.opaque();
        let width = u64::from(self.width);
        // The source step between cells of a line, and from line to line.
        let (step, stride) = if swapped { (width, 1) } else { (1, width) };
        let first = u64::from(columns.first) * step;
        // An opaque line's alpha: its cells' weights, as its texels sum them.
        let mut line_alpha = 0.0;
        if opaque {
            for cell in columns.first..columns.end {
                line_alpha += columns.weight(cell);
            }
        }
        out.fill(0.0);
        let (color, rest) = split(out, opaque);
        let line = &mut line[..color.len()];
        let mut alpha = 0.0;
        let cells = columns.end.saturating_sub(columns.first);
        for offset in rows.first..rows.end {
            let row = if descending {
                rows.end - 1 - (offset - rows.first)
            } else {
                offset
            };
            line.fill(0.0);
            // Cells lie within the image, so their indices cannot overflow.
            let base = u64::from(row).wrapping_mul(stride).wrapping_add(first);
            let index =
                |cell: u32| sample_index(base.wrapping_add(u64::from(cell).wrapping_mul(step)));
            // The first and last cells are weighted; the cells between are
            // whole, and a whole texel adds as itself.
            if cells > 0 {
                texels.add(index(0), columns.first_weight, line);
            }
            for cell in 1..cells.saturating_sub(1) {
                texels.add_unit(index(cell), line);
            }
            if cells > 1 {
                texels.add(index(cells - 1), columns.last_weight, line);
            }
            let weight = rows.weight(row);
            for (sum, value) in color.iter_mut().zip(line.iter()) {
                *sum += value * columns.normalization * weight;
            }
            alpha += line_alpha * columns.normalization * weight;
        }
        rest.fill(alpha);
        for value in out.iter_mut() {
            *value *= rows.normalization;
        }
    }

    fn index(&self, x: f64, y: f64) -> usize {
        let x = coordinate(x, self.width.saturating_sub(1));
        let y = coordinate(y, self.height.saturating_sub(1));
        sample_index(u64::from(y) * u64::from(self.width) + u64::from(x))
    }
}

/// How a placement whose device pixels map by `t` to sample space samples.
fn mode(t: Transform, interpolate: bool) -> Mode {
    let minifying = t.a.hypot(t.b) > 1.0 + 1e-9 || t.c.hypot(t.d) > 1.0 + 1e-9;
    let swapped = t.a == 0.0 && t.d == 0.0;
    if minifying && ((t.b == 0.0 && t.c == 0.0) || swapped) {
        Mode::Separable { swapped }
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
fn sample_index(index: u64) -> usize {
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

/// The area of `polygon` within the unit cell at (`x`, `y`).
fn intersection(polygon: &[Point; 4], x: f64, y: f64) -> f64 {
    let mut points = [Point::default(); 12];
    points[..4].copy_from_slice(polygon);
    let mut len = 4;
    for (axis, bound, sign) in [
        (0, x, 1.0),
        (0, x + 1.0, -1.0),
        (1, y, 1.0),
        (1, y + 1.0, -1.0),
    ] {
        if len == 0 {
            return 0.0;
        }
        let mut output = [Point::default(); 12];
        let mut count = 0;
        for i in 0..len {
            let (a, b) = (points[i], points[(i + 1) % len]);
            let coord = |p: Point| if axis == 0 { p.x } else { p.y };
            let (da, db) = (sign * (coord(a) - bound), sign * (coord(b) - bound));
            if da >= 0.0 {
                output[count] = a;
                count += 1;
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                output[count] = Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
                count += 1;
            }
        }
        points = output;
        len = count;
    }
    // Translate to the cell to avoid cancellation at large source coordinates.
    (0..len)
        .map(|i| {
            let a = points[i];
            let b = points[(i + 1) % len];
            (a.x - x) * (b.y - y) - (a.y - y) * (b.x - x)
        })
        .sum::<f64>()
        .abs()
        * 0.5
}
