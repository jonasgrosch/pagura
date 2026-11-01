//! Image sampling (8.9.5, ADR 0004): where the image is minified, the
//! exact-area average of the samples under the device pixel; where it is
//! magnified, the nearest sample, or bilinear interpolation of the four
//! nearest with `Interpolate`.
//!
//! Samples are first turned into values per sample (associated colour,
//! alpha and shape, or one opacity), so averaging never mixes colour that
//! has not been converted.

use crate::color::Prepared;
use pagura_core::{Error, Result};
use pagura_image::{DecodedImage, DecodedTiles, MaskKind};
use pagura_interpret::{Image, ImageMask};
use pagura_raster::{Point, Transform};
use std::rc::Rc;

#[cfg(test)]
std::thread_local! {
    static PACKED_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod packed_work_tests;

/// Values per sample of an image, row by row from the top.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Values {
    width: u32,
    height: u32,
    stride: usize,
    data: ValuesData,
}

/// Dense converted values retain the established arithmetic. Packed opacity
/// reads present logical source bits lazily, independently of physical tiles.
#[derive(Debug, Clone, PartialEq)]
enum ValuesData {
    Dense(Vec<f64>),
    Packed {
        tiles: Rc<DecodedTiles>,
        endpoints: [f64; 2],
    },
}

impl Values {
    /// `stride` values for each of `width × height` samples, computed by
    /// `value(sample index, out)`.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error for an image beyond `max_values` values and
    /// the errors of `value`.
    pub(crate) fn build(
        (width, height): (u32, u32),
        stride: usize,
        max_values: usize,
        mut value: impl FnMut(usize, &mut [f64]) -> Result<()>,
    ) -> Result<Self> {
        let count = usize::try_from(u64::from(width) * u64::from(height))
            .map_err(|_| Error::limit("image too large for the reference"))?;
        let total = count
            .checked_mul(stride)
            .filter(|&n| n <= max_values)
            .ok_or(Error::limit("image too large for the reference"))?;
        let mut data = vec![0.0; total];
        for (i, out) in data.chunks_exact_mut(stride.max(1)).enumerate() {
            value(i, out)?;
        }
        Ok(Self {
            width,
            height,
            stride,
            data: ValuesData::Dense(data),
        })
    }

    /// A complete Gray1 soft mask without one f64 allocation per source bit.
    pub(crate) fn packed(tiles: Rc<DecodedTiles>, decode: [f64; 2]) -> Result<Self> {
        if decode.iter().any(|value| !value.is_finite()) {
            return Err(Error::syntax("invalid tiled mask Decode range"));
        }
        Ok(Self {
            width: tiles.width(),
            height: tiles.height(),
            stride: 1,
            data: ValuesData::Packed {
                tiles,
                endpoints: decode.map(|value| value.clamp(0.0, 1.0)),
            },
        })
    }

    /// Exact rectangle integral of the piecewise-constant logical bit grid.
    /// Only packed opacity uses this byte-counting path; converted dense
    /// samples retain their polygon integration and arithmetic unchanged.
    fn rectangle_mean(&self, low: Point, high: Point) -> Option<f64> {
        let ValuesData::Packed { tiles, endpoints } = &self.data else {
            return None;
        };
        let x0 = low.x.max(0.0);
        let x1 = high.x.min(f64::from(self.width));
        let y0 = low.y.max(0.0);
        let y1 = high.y.min(f64::from(self.height));
        if x1 <= x0 || y1 <= y0 {
            return Some(0.0);
        }
        let left = clamp_index(x0.floor(), self.width);
        let right = clamp_index(x1.ceil() - 1.0, self.width);
        let top = clamp_index(y0.floor(), self.height);
        let bottom = clamp_index(y1.ceil() - 1.0, self.height);
        let lx = f64::from(u32::try_from(left).ok()?);
        let rx = f64::from(u32::try_from(right).ok()?);
        let mut sum = Sum::default();
        for row in top..=bottom {
            let y = u32::try_from(row).ok()?;
            let line = tiles.row(y)?;
            let bit = |x: usize| {
                #[cfg(test)]
                PACKED_VISITS.set(PACKED_VISITS.get().saturating_add(1));
                line.get(x / 8)
                    .map(|byte| u32::from((byte >> (7 - x % 8)) & 1))
            };
            let ones = if left == right {
                f64::from(bit(left)?) * (x1 - x0)
            } else {
                f64::from(bit(left)?) * (lx + 1.0 - x0)
                    + f64::from(bit(right)?) * (x1 - rx)
                    + f64::from(logical_ones(line, left + 1, right)?)
            };
            let height = y1.min(f64::from(y) + 1.0) - y0.max(f64::from(y));
            sum.add(ones * height);
        }
        let area = (x1 - x0) * (y1 - y0);
        Some((endpoints[0] * (area - sum.value()) + endpoints[1] * sum.value()) / area)
    }

    fn visit(&self, column: f64, row: f64, consume: impl FnOnce(&[f64])) {
        let x = clamp_index(column, self.width);
        let y = clamp_index(row, self.height);
        match &self.data {
            ValuesData::Dense(data) => {
                let start = (y * self.width as usize + x) * self.stride;
                consume(data.get(start..start + self.stride).unwrap_or_default());
            }
            ValuesData::Packed { tiles, endpoints } => {
                let code = u32::try_from(x)
                    .ok()
                    .zip(u32::try_from(y).ok())
                    .and_then(|(x, y)| tiles.get(x, y));
                let value = code
                    .and_then(|code| endpoints.get(usize::from(code)))
                    .copied()
                    .unwrap_or(0.0);
                consume(&[value]);
            }
        }
    }
}

/// Count only logical cells in a half-open source-row interval. Complete
/// byte spans use popcount; partial leading/trailing bytes exclude padbits.
fn logical_ones(row: &[u8], mut start: usize, end: usize) -> Option<u32> {
    let mut ones = 0_u32;
    while start < end && !start.is_multiple_of(8) {
        #[cfg(test)]
        PACKED_VISITS.set(PACKED_VISITS.get().saturating_add(1));
        ones = ones.checked_add(u32::from((row.get(start / 8)? >> (7 - start % 8)) & 1))?;
        start += 1;
    }
    if start < end {
        let boundary = end / 8;
        for byte in row.get(start / 8..boundary)? {
            #[cfg(test)]
            PACKED_VISITS.set(PACKED_VISITS.get().saturating_add(1));
            ones = ones.checked_add(byte.count_ones())?;
        }
        start = boundary * 8;
        while start < end {
            #[cfg(test)]
            PACKED_VISITS.set(PACKED_VISITS.get().saturating_add(1));
            ones = ones.checked_add(u32::from((row.get(start / 8)? >> (7 - start % 8)) & 1))?;
            start += 1;
        }
    }
    Some(ones)
}

/// An index within `0..size`, from a coordinate clamped into range.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..size, which fits u32"
)]
fn clamp_index(value: f64, size: u32) -> usize {
    value.clamp(0.0, f64::from(size.saturating_sub(1))) as usize
}

/// The mapping from device pixels to an image's samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sampler {
    width: u32,
    height: u32,
    interpolate: bool,
    /// Device space to sample space.
    inverse: Transform,
}

/// How an image is sampled where it is magnified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Magnify {
    /// The nearest sample.
    Nearest,
    /// Bilinear interpolation (`Interpolate`).
    Bilinear,
}

impl Sampler {
    /// The sampler of an image of `width × height` samples whose unit
    /// square maps to the device by `unit_to_device`; `None` when that map
    /// is singular, so the image paints nothing.
    pub(crate) fn new(
        (width, height): (u32, u32),
        magnify: Magnify,
        unit_to_device: Transform,
    ) -> Option<Self> {
        let inverse = unit_to_device.inverse()?;
        let (w, h) = (f64::from(width), f64::from(height));
        Some(Self {
            width,
            height,
            interpolate: magnify == Magnify::Bilinear,
            inverse: inverse.then(Transform::new(w, 0.0, 0.0, -h, 0.0, h)),
        })
    }

    /// Whether a device pixel spans more than one sample along either axis.
    fn minifies(&self) -> bool {
        let t = self.inverse;
        t.a.hypot(t.b) > 1.0 + 1e-9 || t.c.hypot(t.d) > 1.0 + 1e-9
    }

    /// Admit packed-mask reconstruction visits cumulatively before sampling.
    /// The sole caller supplies complete Packed values with these dimensions.
    /// Axis-aligned area uses the existing byte-counting rectangle integral;
    /// other modes retain their complete logical-cell bounds. No meter resets
    /// per row, tile or repeated image instance.
    pub(crate) fn charge_packed_work(
        &self,
        pixels: impl Iterator<Item = (u32, u32, f64)>,
        work: &mut pagura_core::Meter,
    ) -> Result<()> {
        let area = self.minifies();
        let t = self.inverse;
        if [t.a, t.b, t.c, t.d, t.e, t.f]
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(Error::limit("reference packed-mask transform overflow"));
        }
        let cells = |span: f64, size: u32| -> Result<usize> {
            if !span.is_finite() || span < 0.0 || size == 0 {
                return Err(Error::limit("reference packed-mask footprint overflow"));
            }
            clamp_index(span.ceil() + 1.0, size)
                .checked_add(1)
                .ok_or(Error::limit("reference packed-mask footprint overflow"))
        };
        let cost = if area {
            let columns = cells(t.a.abs() + t.c.abs(), self.width)?;
            let rows = cells(t.b.abs() + t.d.abs(), self.height)?;
            let columns = if (t.a == 0.0 && t.d == 0.0) || (t.b == 0.0 && t.c == 0.0) {
                // Endpoint bits + at most fourteen partial-byte bits, then
                // one visit per complete byte. Both bounds remain valid.
                columns.min(
                    columns
                        .div_ceil(8)
                        .checked_add(16)
                        .ok_or(Error::limit("reference packed-mask footprint overflow"))?,
                )
            } else {
                columns
            };
            columns
                .checked_mul(rows)
                .ok_or(Error::limit("reference packed-mask footprint overflow"))?
        } else if self.interpolate {
            4
        } else {
            1
        };
        for (x, y, _) in pixels {
            work.charge(cost)?;
            let (x, y) = (f64::from(x), f64::from(y));
            for (x, y) in [(x, y), (x + 1.0, y), (x, y + 1.0), (x + 1.0, y + 1.0)] {
                let point = Point::new(x, y) * t;
                if !point.x.is_finite() || !point.y.is_finite() {
                    return Err(Error::limit(
                        "reference packed-mask sample position overflow",
                    ));
                }
            }
        }
        Ok(())
    }

    /// The value of device pixel `(x, y)`, written into `out`.
    pub(crate) fn sample(&self, values: &Values, (x, y): (u32, u32), out: &mut [f64]) {
        out.fill(0.0);
        if self.minifies() {
            self.area(values, (x, y), out);
        } else if self.interpolate {
            self.bilinear(values, (x, y), out);
        } else {
            let c = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5) * self.inverse;
            values.visit(c.x.floor(), c.y.floor(), |sample| add(out, sample, 1.0));
        }
    }

    fn bilinear(&self, values: &Values, (x, y): (u32, u32), out: &mut [f64]) {
        let c = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5) * self.inverse;
        let (x, y) = (c.x - 0.5, c.y - 0.5);
        let (left, top) = (x.floor(), y.floor());
        let (fx, fy) = (x - left, y - top);
        for (column, row, weight) in [
            (left, top, (1.0 - fx) * (1.0 - fy)),
            (left + 1.0, top, fx * (1.0 - fy)),
            (left, top + 1.0, (1.0 - fx) * fy),
            (left + 1.0, top + 1.0, fx * fy),
        ] {
            values.visit(column, row, |sample| add(out, sample, weight));
        }
    }

    /// The exact-area average over the samples the device pixel's preimage
    /// meets, within the image.
    fn area(&self, values: &Values, (x, y): (u32, u32), out: &mut [f64]) {
        let (x, y) = (f64::from(x), f64::from(y));
        let polygon = [(x, y), (x + 1.0, y), (x + 1.0, y + 1.0), (x, y + 1.0)]
            .map(|(x, y)| Point::new(x, y) * self.inverse);
        let low = polygon
            .iter()
            .fold(polygon[0], |m, p| Point::new(m.x.min(p.x), m.y.min(p.y)));
        let high = polygon
            .iter()
            .fold(polygon[0], |m, p| Point::new(m.x.max(p.x), m.y.max(p.y)));
        let t = self.inverse;
        if ((t.a == 0.0 && t.d == 0.0) || (t.b == 0.0 && t.c == 0.0))
            && let Some(value) = values.rectangle_mean(low, high)
        {
            if let Some(out) = out.first_mut() {
                *out = value;
            }
            return;
        }
        let columns = low.x.floor().max(0.0)..high.x.ceil().min(f64::from(self.width));
        let rows = low.y.floor().max(0.0)..high.y.ceil().min(f64::from(self.height));
        // Footprints can hold millions of samples: compensated sums keep
        // the mean within a few roundings, so exact ties stay ties.
        let mut sums = [Sum::default(); MAX_VALUES];
        let mut weight = Sum::default();
        let mut row = rows.start;
        while row < rows.end {
            let mut column = columns.start;
            while column < columns.end {
                let area = clipped_area(&polygon, column, row);
                if area > 0.0 {
                    values.visit(column, row, |sample| {
                        for (sum, v) in sums.iter_mut().zip(sample) {
                            sum.add(v * area);
                        }
                    });
                    weight.add(area);
                }
                column += 1.0;
            }
            row += 1.0;
        }
        let weight = weight.value();
        if weight > 0.0 {
            for (out, sum) in out.iter_mut().zip(&sums) {
                *out = sum.value() / weight;
            }
        }
    }
}

/// The most values one sample carries: channels, alpha and shape.
const MAX_VALUES: usize = crate::color::MAX + 2;

/// A sum with Neumaier's compensation.
#[derive(Debug, Clone, Copy, Default)]
struct Sum {
    total: f64,
    error: f64,
}

impl Sum {
    fn add(&mut self, value: f64) {
        let total = self.total + value;
        self.error += if self.total.abs() >= value.abs() {
            (self.total - total) + value
        } else {
            (value - total) + self.total
        };
        self.total = total;
    }

    fn value(self) -> f64 {
        self.total + self.error
    }
}

fn add(out: &mut [f64], values: &[f64], weight: f64) {
    for (out, v) in out.iter_mut().zip(values) {
        *out += v * weight;
    }
}

/// The area of a convex polygon within the unit cell at `(x, y)`, by
/// clipping it to the cell's four sides.
fn clipped_area(polygon: &[Point; 4], x: f64, y: f64) -> f64 {
    let mut points: Vec<Point> = polygon.to_vec();
    for (vertical, bound, sign) in [
        (true, x, 1.0),
        (true, x + 1.0, -1.0),
        (false, y, 1.0),
        (false, y + 1.0, -1.0),
    ] {
        let side = |p: Point| sign * (if vertical { p.x } else { p.y } - bound);
        let mut next = Vec::with_capacity(points.len() + 2);
        for (i, &a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            let (da, db) = (side(a), side(b));
            if da >= 0.0 {
                next.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                let t = da / (da - db);
                next.push(Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y)));
            }
        }
        points = next;
        if points.is_empty() {
            return 0.0;
        }
    }
    let mut twice = 0.0;
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        twice += (a.x - x) * (b.y - y) - (a.y - y) * (b.x - x);
    }
    twice.abs() * 0.5
}

/// The decoded components of sample `index` of an image: `Decode` applied
/// to each raw value (8.9.5.2).
pub(crate) fn components(
    image: &Image,
    decoded: &DecodedImage,
    index: usize,
    out: &mut [f64],
) -> Result<usize> {
    let count = usize::from(decoded.components);
    for k in 0..count.min(out.len()) {
        let raw = decoded
            .samples
            .get(index * count + k)
            .ok_or(Error::syntax("missing image sample"))?;
        let bits = decoded
            .component_bits(k)
            .unwrap_or(decoded.bits_per_component);
        let maximum = f64::from(u16::MAX >> 16_u32.saturating_sub(u32::from(bits.clamp(1, 16))));
        let [low, high] = image.decode.get(k).copied().unwrap_or([0.0, maximum]);
        out[k] = low + f64::from(raw) / maximum * (high - low);
    }
    Ok(count)
}

/// Whether the raw components of sample `index` fall in every colour-key
/// range (8.9.6.4), which masks the sample out.
fn keyed(ranges: &[[u16; 2]], decoded: &DecodedImage, index: usize) -> bool {
    let count = usize::from(decoded.components);
    ranges.iter().enumerate().all(|(k, &[low, high])| {
        decoded
            .samples
            .get(index * count + k)
            .is_some_and(|v| v >= low && v <= high)
    })
}

/// The opacity of sample `index` of a stencil or soft-mask image: a stencil
/// paints where its decoded value is 0 (8.9.6.2), a soft mask is its
/// decoded gray level (11.6.5.3).
pub(crate) fn opacity(image: &Image, decoded: &DecodedImage, index: usize) -> Result<f64> {
    let mut value = [0.0; 1];
    components(image, decoded, index, &mut value)?;
    Ok(if image.space.is_none() {
        1.0 - value[0].clamp(0.0, 1.0)
    } else {
        value[0].clamp(0.0, 1.0)
    })
}

/// What a colour image's samples become: `channels` values of associated
/// colour, then alpha and shape.
pub(crate) struct ColorSamples<'a> {
    /// The image.
    pub(crate) image: &'a Image,
    /// Its samples.
    pub(crate) decoded: &'a DecodedImage,
    /// Its soft mask, for `Matte`.
    pub(crate) matte: Option<(&'a [f64], &'a Image, &'a DecodedImage)>,
    /// The conversion of its colour space into the target, or of the
    /// palette's base space with [`Self::palette`].
    pub(crate) convert: &'a Prepared,
    /// An Indexed space looked up before the matte is removed, which
    /// happens in the base space (11.6.5.3).
    pub(crate) palette: Option<&'a pagura_color::Indexed>,
    /// The target's channels.
    pub(crate) channels: usize,
    /// Whether alpha is shape, so embedded opacity scales shape too.
    pub(crate) alpha_is_shape: bool,
}

impl ColorSamples<'_> {
    /// Writes sample `index`'s values.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for missing samples.
    pub(crate) fn value(&self, index: usize, out: &mut [f64]) -> Result<()> {
        let mut c = [0.0; pagura_color::MAX_COMPONENTS];
        let mut count = components(self.image, self.decoded, index, &mut c)?;
        if let Some(indexed) = self.palette {
            let base = crate::color::palette(indexed, c[0]);
            count = base.len().min(c.len());
            c[..count].copy_from_slice(&base[..count]);
        }
        let (mut alpha, mut shape) = (1.0, 1.0);
        if let ImageMask::ColorKey(ranges) = &self.image.mask
            && keyed(ranges, self.decoded, index)
        {
            (alpha, shape) = (0.0, 0.0);
        }
        if let MaskKind::Embedded(plane) = &self.decoded.mask {
            let raw = plane
                .samples
                .get(index)
                .ok_or(Error::syntax("missing image opacity"))?;
            let bits = u32::from(plane.bits_per_component.clamp(1, 16));
            let opacity = f64::from(raw) / f64::from(u16::MAX >> (16 - bits));
            alpha *= opacity;
            if self.alpha_is_shape {
                shape *= opacity;
            }
            if plane.premultiplied {
                for v in c.iter_mut().take(count) {
                    *v = if opacity > 0.0 { *v / opacity } else { 0.0 };
                }
            }
        }
        if let Some((matte, mask, decoded)) = self.matte {
            let opacity = opacity(mask, decoded, index)?;
            for (v, &m) in c.iter_mut().zip(matte).take(count) {
                *v = if opacity > 0.0 {
                    m + (*v - m) / opacity
                } else {
                    0.0
                };
            }
        }
        let mut color = [0.0; crate::color::MAX];
        self.convert
            .convert(&c[..count], &mut color[..self.channels]);
        for (out, v) in out.iter_mut().zip(&color[..self.channels]) {
            *out = v * alpha;
        }
        out[self.channels] = alpha;
        out[self.channels + 1] = shape;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(width: u32, height: u32) -> Values {
        Values::build((width, height), 1, usize::MAX, |i, out| {
            let (x, y) = (i % width as usize, i / width as usize);
            out[0] = f64::from(u8::from((x + y) % 2 == 1));
            Ok(())
        })
        .unwrap()
    }

    #[test]
    fn minified_samples_average_by_exact_area() {
        // A 4×4 checkerboard drawn into 2×2 device pixels shifted by a
        // quarter pixel: every pixel's preimage covers the board's cells
        // in known fractions.
        let values = checker(4, 4);
        let unit = Transform::new(2.0, 0.0, 0.0, -2.0, 0.25, 2.0);
        let sampler = Sampler::new((4, 4), Magnify::Nearest, unit).unwrap();
        let mut out = [0.0];
        sampler.sample(&values, (1, 1), &mut out);
        // Pixel (1, 1) maps to samples x 1.5..3.5 clipped to 1.5..3.5,
        // y 2..4: within the image, the columns 1.5–2, 2–3, 3–3.5 with
        // weights 1/2, 1, 1/2 and alternating values average to 1/2.
        assert!((out[0] - 0.5).abs() < 1e-12, "{}", out[0]);
        // Pixel (0, 0) maps to x -0.5..1.5, of which 0..1.5 lies in the
        // image: column 0 (weight 1, values 0 and 1 over two rows) and
        // half of column 1.
        sampler.sample(&values, (0, 0), &mut out);
        assert!((out[0] - 0.5).abs() < 1e-12, "{}", out[0]);
    }

    #[test]
    fn large_footprints_average_exactly_enough_to_keep_ties() {
        // A 1024×1024 image of alternating 95 and 96 in one device pixel:
        // the exact mean is the half level 95.5.
        let values = Values::build((1024, 1024), 1, usize::MAX, |i, out| {
            out[0] = f64::from(95 + u8::from(i % 2 == 1)) / 255.0;
            Ok(())
        })
        .unwrap();
        let unit = Transform::new(1.0, 0.0, 0.0, -1.0, 0.0, 1.0);
        let sampler = Sampler::new((1024, 1024), Magnify::Nearest, unit).unwrap();
        let mut out = [0.0];
        sampler.sample(&values, (0, 0), &mut out);
        assert!((out[0] * 255.0 - 95.5).abs() < 1e-11, "{}", out[0] * 255.0);
        assert_eq!(pagura_raster::quantize_unit(out[0]), 96);
    }

    #[test]
    fn magnified_samples_take_the_nearest_or_bilinear_value() {
        let values = checker(2, 1);
        let unit = Transform::new(8.0, 0.0, 0.0, -1.0, 0.0, 1.0);
        let nearest = Sampler::new((2, 1), Magnify::Nearest, unit).unwrap();
        let mut out = [0.0];
        nearest.sample(&values, (3, 0), &mut out);
        assert_eq!(out[0], 0.0);
        nearest.sample(&values, (4, 0), &mut out);
        assert_eq!(out[0], 1.0);
        let bilinear = Sampler::new((2, 1), Magnify::Bilinear, unit).unwrap();
        // Device x 4.5 is sample x 1.125: 5/8 of the way from the first
        // sample centre (0.5) to the second (1.5).
        bilinear.sample(&values, (4, 0), &mut out);
        assert!((out[0] - 0.625).abs() < 1e-12);
        assert!(Sampler::new((2, 1), Magnify::Nearest, Transform::scale(0.0, 1.0)).is_none());
    }
    #[test]
    fn packed_opacity_matches_independent_logical_bits_for_all_sampling_modes() {
        use pagura_core::{Limits, ObjectId};
        use pagura_document::{Document, OpenOptions};
        use pagura_image::{ImageSource, Orientation};
        use pagura_testkit::fixture;
        use std::rc::Rc;
        let limits = Limits {
            pixels: 10,
            ..Limits::default()
        };
        let document = Document::open(
            fixture("packed_mask_tiles", "small-flate.pdf"),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let source = ImageSource::from_xobject_with_tiling(
            &document,
            document.object(ObjectId::new(6, 0)).unwrap(),
        )
        .unwrap();
        let tiles = Rc::new(source.decode_tiles(Orientation::Rows, &limits).unwrap());
        assert_eq!(tiles.tiles().len(), 7);
        for decode in [[0.0_f64, 1.0], [1.0, 0.0], [-100.0, 100.0], [1e300, -1e300]] {
            let packed = Values::packed(Rc::clone(&tiles), decode).unwrap();
            // Literal independent rectangle, excluding all nonzero padbits.
            let dense = Values::build((10, 7), 1, 70, |index, out| {
                let code =
                    usize::from((2..8).contains(&(index % 10)) && (1..6).contains(&(index / 10)));
                out[0] = decode[code].clamp(0.0, 1.0);
                Ok(())
            })
            .unwrap();
            for unit in [
                Transform::scale(20.0, 20.0),
                Transform::scale(2.0, 2.0),
                Transform::new(-20.0, 0.0, 0.0, 20.0, 20.0, 0.0),
                Transform::new(0.0, 20.0, -20.0, 0.0, 20.0, 0.0),
            ] {
                for magnify in [Magnify::Nearest, Magnify::Bilinear] {
                    let sampler = Sampler::new((10, 7), magnify, unit).unwrap();
                    // Scalar mathematical samples; no20x20rendered surface
                    // is allocated or admitted under the10pixel budget.
                    for row in 0..20 {
                        for column in 0..20 {
                            let (mut a, mut b) = ([0.0], [0.0]);
                            sampler.sample(&packed, (column, row), &mut a);
                            sampler.sample(&dense, (column, row), &mut b);
                            assert_eq!(
                                a, b,
                                "decode{decode:?} transform{unit:?} {magnify:?} ({column},{row})"
                            );
                        }
                    }
                }
            }
        }
        assert!(Values::packed(tiles, [f64::NAN, 1.0]).is_err());
    }

    #[test]
    fn complete_byte_counts_never_include_logical_row_padding() {
        let row = [0b1010_1100, 0b0101_1111];
        let bits = [1_u32, 0, 1, 0, 1, 1, 0, 0, 0, 1, 0, 1, 1];
        for start in 0..=bits.len() {
            for end in start..=bits.len() {
                assert_eq!(
                    logical_ones(&row, start, end),
                    Some(bits[start..end].iter().sum()),
                    "interval{start}..{end}"
                );
            }
        }
        assert_eq!(logical_ones(&row, 0, 13), Some(7));
    }
}
