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
use pagura_image::{DecodedImage, MaskKind};
use pagura_interpret::{Image, ImageMask};
use pagura_raster::{Point, Transform};

/// Values per sample of an image, row by row from the top.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Values {
    width: u32,
    height: u32,
    stride: usize,
    data: Vec<f64>,
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
            data,
        })
    }

    fn at(&self, column: f64, row: f64) -> &[f64] {
        let x = clamp_index(column, self.width);
        let y = clamp_index(row, self.height);
        let start = (y * self.width as usize + x) * self.stride;
        self.data
            .get(start..start + self.stride)
            .unwrap_or_default()
    }
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

    /// The value of device pixel `(x, y)`, written into `out`.
    pub(crate) fn sample(&self, values: &Values, (x, y): (u32, u32), out: &mut [f64]) {
        out.fill(0.0);
        if self.minifies() {
            self.area(values, (x, y), out);
        } else if self.interpolate {
            self.bilinear(values, (x, y), out);
        } else {
            let c = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5) * self.inverse;
            add(out, values.at(c.x.floor(), c.y.floor()), 1.0);
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
            add(out, values.at(column, row), weight);
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
                    for (sum, v) in sums.iter_mut().zip(values.at(column, row)) {
                        sum.add(v * area);
                    }
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

/// The decoded components of sample `index` of an image: undo stored
/// association (T.800 I.5.3.6) before applying `Decode` (8.9.5.2).
fn components(
    image: &Image,
    decoded: &DecodedImage,
    index: usize,
    association: f64,
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
        let sample = if association > 0.0 {
            f64::from(raw) / maximum / association
        } else {
            0.0
        };
        out[k] = low + sample * (high - low);
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
    components(image, decoded, index, 1.0, &mut value)?;
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
        let (embedded_opacity, premultiplied) = match &self.decoded.mask {
            MaskKind::Embedded(plane) => {
                let raw = plane
                    .samples
                    .get(index)
                    .ok_or(Error::syntax("missing image opacity"))?;
                let bits = u32::from(plane.bits_per_component.clamp(1, 16));
                (
                    f64::from(raw) / f64::from(u16::MAX >> (16 - bits)),
                    plane.premultiplied,
                )
            }
            _ => (1.0, false),
        };
        // Indexed codes first select their preblended base-space colours;
        // ordinary stored component values are unassociated before Decode.
        let association = if premultiplied && self.palette.is_none() {
            embedded_opacity
        } else {
            1.0
        };
        let mut c = [0.0; pagura_color::MAX_COMPONENTS];
        let mut count = components(self.image, self.decoded, index, association, &mut c)?;
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
        alpha *= embedded_opacity;
        if self.alpha_is_shape {
            shape *= embedded_opacity;
        }
        if premultiplied && self.palette.is_some() {
            for v in c.iter_mut().take(count) {
                *v = if embedded_opacity > 0.0 {
                    *v / embedded_opacity
                } else {
                    0.0
                };
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
}
