//! The previous renderer's exact image sampling (its sampled, reduced and
//! texel code), ported in its original structure without budgets: a
//! straightforward, independent oracle for the sampler.

use super::{Case, MaskUse, base, converter};
use crate::source::image::texels::Convert;
use pagura_color::ColorSpace;
use pagura_image::{DecodedImage, ImageKind, MaskKind};
use pagura_raster::{Point, Transform};

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Texel {
    pub(super) color: [f64; 4],
    pub(super) alpha: f64,
    pub(super) shape: f64,
}

trait Weighted: Copy + Default {
    fn add(&mut self, value: Self, weight: f64);
    #[must_use]
    fn scale(self, weight: f64) -> Self;
}

impl Weighted for Texel {
    fn add(&mut self, value: Self, weight: f64) {
        for (sum, c) in self.color.iter_mut().zip(value.color) {
            *sum += c * weight;
        }
        self.alpha += value.alpha * weight;
        self.shape += value.shape * weight;
    }
    fn scale(mut self, weight: f64) -> Self {
        self.color = self.color.map(|v| v * weight);
        self.alpha *= weight;
        self.shape *= weight;
        self
    }
}

impl Weighted for f64 {
    fn add(&mut self, value: Self, weight: f64) {
        *self += value * weight;
    }
    fn scale(self, weight: f64) -> Self {
        self * weight
    }
}

struct Sampler {
    width: u32,
    height: u32,
    interpolate: bool,
    inverse: Transform,
}

impl Sampler {
    fn minifying(&self) -> bool {
        let t = self.inverse;
        t.a.hypot(t.b) > 1.0 + 1e-9 || t.c.hypot(t.d) > 1.0 + 1e-9
    }

    fn center(&self, x: u32, y: u32) -> Point {
        Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5) * self.inverse
    }

    fn nearest(&self, x: u32, y: u32) -> usize {
        let center = self.center(x, y);
        self.index(center.x.floor(), center.y.floor())
    }

    fn magnify<T: Weighted>(&self, x: u32, y: u32, mut texel: impl FnMut(usize) -> T) -> T {
        if !self.interpolate {
            return texel(self.nearest(x, y));
        }
        let center = self.center(x, y);
        let (x, y) = (center.x - 0.5, center.y - 0.5);
        let (left, top) = (x.floor(), y.floor());
        let (fx, fy) = (x - left, y - top);
        let mut out = T::default();
        for (x, y, w) in [
            (left, top, (1.0 - fx) * (1.0 - fy)),
            (left + 1.0, top, fx * (1.0 - fy)),
            (left, top + 1.0, (1.0 - fx) * fy),
            (left + 1.0, top + 1.0, fx * fy),
        ] {
            out.add(texel(self.index(x, y)), w);
        }
        out
    }

    fn area<T: Weighted>(&self, x: u32, y: u32, mut texel: impl FnMut(usize) -> T) -> T {
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
        let mut out = T::default();
        let mut weight = 0.0;
        for row in y0..y1 {
            for col in x0..x1 {
                let area = intersection(&polygon, f64::from(col), f64::from(row));
                if area > 0.0 {
                    out.add(texel(self.index(f64::from(col), f64::from(row))), area);
                    weight += area;
                }
            }
        }
        if weight > 0.0 {
            out.scale(1.0 / weight)
        } else {
            out
        }
    }

    fn index(&self, x: f64, y: f64) -> usize {
        let x = coordinate(x, self.width.saturating_sub(1));
        let y = coordinate(y, self.height.saturating_sub(1));
        usize::try_from(u64::from(y) * u64::from(self.width) + u64::from(x)).unwrap()
    }

    fn prepare<T: Weighted>(self, origin: u32, width: u32) -> Prepared<T> {
        let t = self.inverse;
        let minifying = self.minifying();
        let mut out = Prepared {
            swapped: t.a == 0.0 && t.d == 0.0,
            sampler: self,
            columns: Vec::new(),
            origin,
            vertical: None,
            minifying,
        };
        if out.minifying && ((t.b == 0.0 && t.c == 0.0) || out.swapped) {
            let (scale, offset, maximum) = if out.swapped {
                (t.b, t.f, out.sampler.height)
            } else {
                (t.a, t.e, out.sampler.width)
            };
            for index in 0..width {
                out.columns.push(Column {
                    span: Span::new(origin + index, scale, offset, maximum),
                    rows: [None; 2],
                });
            }
        }
        out
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sample coordinates clamped to u32 image bounds"
)]
fn coordinate(value: f64, maximum: u32) -> u32 {
    value.clamp(0.0, f64::from(maximum)) as u32
}

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

#[derive(Clone, Copy, Default)]
struct Span {
    first: u32,
    end: u32,
    first_weight: f64,
    last_weight: f64,
    normalization: f64,
}

impl Span {
    fn new(pixel: u32, scale: f64, offset: f64, maximum: u32) -> Self {
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

    fn weight(self, cell: u32) -> f64 {
        if cell == self.first {
            self.first_weight
        } else if cell + 1 == self.end {
            self.last_weight
        } else {
            1.0
        }
    }

    fn normalize<T: Weighted>(self, value: T) -> T {
        value.scale(self.normalization)
    }
}

struct Column<T> {
    span: Span,
    rows: [Option<(u32, T)>; 2],
}

struct Prepared<T> {
    sampler: Sampler,
    columns: Vec<Column<T>>,
    origin: u32,
    swapped: bool,
    vertical: Option<(u32, Span)>,
    minifying: bool,
}

impl<T: Weighted> Prepared<T> {
    fn sample(&mut self, x: u32, y: u32, texel: impl FnMut(usize) -> T) -> T {
        if !self.columns.is_empty() {
            self.reduce(x, y, texel)
        } else if self.minifying {
            self.sampler.area(x, y, texel)
        } else {
            self.sampler.magnify(x, y, texel)
        }
    }

    fn reduce(&mut self, x: u32, y: u32, mut texel: impl FnMut(usize) -> T) -> T {
        let t = self.sampler.inverse;
        if self.vertical.is_none_or(|(row, _)| row != y) {
            let (scale, offset, bound) = if self.swapped {
                (t.c, t.e, self.sampler.width)
            } else {
                (t.d, t.f, self.sampler.height)
            };
            self.vertical = Some((y, Span::new(y, scale, offset, bound)));
        }
        let span = self.vertical.unwrap().1;
        let column = &mut self.columns[usize::try_from(x - self.origin).unwrap()];
        let mut out = T::default();
        for step in span.first..span.end {
            let row = if (if self.swapped { t.c } else { t.d }) < 0.0 {
                span.end - 1 - (step - span.first)
            } else {
                step
            };
            let value =
                if let Some((_, value)) = column.rows.iter().flatten().find(|(r, _)| *r == row) {
                    *value
                } else {
                    let mut value = T::default();
                    for col in column.span.first..column.span.end {
                        let (col, row) = if self.swapped { (row, col) } else { (col, row) };
                        let index = usize::try_from(
                            u64::from(row) * u64::from(self.sampler.width) + u64::from(col),
                        )
                        .unwrap();
                        let cell = if self.swapped { row } else { col };
                        value.add(texel(index), column.span.weight(cell));
                    }
                    let value = column.span.normalize(value);
                    column.rows.swap(0, 1);
                    column.rows[0] = Some((row, value));
                    value
                };
            out.add(value, span.weight(row));
        }
        span.normalize(out)
    }
}

/// The sampler of an image whose unit square spans its extent in stored
/// samples, placed by `inverse`.
fn sampler_for(image: &DecodedImage, inverse: Transform) -> Sampler {
    let (width, height) = image.extent();
    Sampler {
        width: image.width,
        height: image.height,
        interpolate: image.interpolate,
        inverse: inverse.then(Transform::new(width, 0.0, 0.0, -height, 0.0, height)),
    }
}

fn max_sample(image: &DecodedImage, component: usize) -> f64 {
    f64::from(u16::MAX >> (16 - image.component_bits(component).unwrap()))
}

/// `Image::components`: decoded components of one pixel.
fn components(image: &DecodedImage, decode: &[[f64; 2]], pixel: usize, values: &mut [f64; 32]) {
    let n = usize::from(image.components);
    for (component, out) in values.iter_mut().enumerate().take(n) {
        let sample = image.samples.get(pixel * n + component).unwrap();
        let [low, high] = decode[component];
        *out = low + f64::from(sample) / max_sample(image, component) * (high - low);
    }
}

/// `mask_texel`'s alpha.
fn mask_alpha(image: &DecodedImage, decode: [f64; 2], pixel: usize) -> f64 {
    let sample = image.samples.get(pixel).unwrap();
    if let ImageKind::Stencil { inverted } = image.kind {
        f64::from(u8::from((sample == 1) == inverted))
    } else {
        let mut values = [0.0; 32];
        components(image, &[decode], pixel, &mut values);
        values[0].clamp(0.0, 1.0)
    }
}

pub(super) struct Oracle<'a> {
    case: &'a Case,
    convert: Option<Box<dyn Convert>>,
    image: Prepared<Texel>,
    mask: Option<Prepared<f64>>,
}

impl<'a> Oracle<'a> {
    pub(super) fn new(case: &'a Case, unit_to_device: Transform, target: usize) -> Self {
        Self::with_columns(case, unit_to_device, target, (0, 256))
    }

    /// As [`Self::new`] for device columns `x0..x1`.
    pub(super) fn with_columns(
        case: &'a Case,
        unit_to_device: Transform,
        target: usize,
        (x0, x1): (u32, u32),
    ) -> Self {
        let inverse = unit_to_device.inverse().unwrap();
        Self {
            case,
            convert: case
                .space
                .as_ref()
                .map(|space| converter(base(space), target).0),
            image: sampler_for(&case.image, inverse).prepare(x0, x1 - x0),
            mask: case
                .mask
                .as_ref()
                .map(|(mask, _, _)| sampler_for(mask, inverse).prepare(x0, x1 - x0)),
        }
    }

    /// `image_texel`, or a stencil's `mask_texel` coloured.
    fn texel(case: &Case, convert: Option<&dyn Convert>, pixel: usize) -> Texel {
        let image = &case.image;
        if case.space.is_none() {
            let alpha = mask_alpha(image, [0.0, 1.0], pixel);
            let mut color = [0.0; 4];
            for (out, value) in color.iter_mut().zip(case.stencil_color.iter().flatten()) {
                *out = *value;
            }
            return Texel {
                color: color.map(|v| v * alpha),
                alpha,
                shape: alpha,
            };
        }
        let mut values = [0.0; 32];
        components(image, &case.decode, pixel, &mut values);
        let count = if let Some(ColorSpace::Indexed(indexed)) = &case.space {
            let index = values[0].round().clamp(0.0, f64::from(indexed.high));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "index"
            )]
            let index = index as usize;
            let n = indexed.base.components();
            let bytes = &indexed.lookup[index * n..(index + 1) * n];
            for (out, &byte) in values.iter_mut().zip(bytes) {
                *out = f64::from(byte) / 255.0;
            }
            n
        } else {
            usize::from(image.components)
        };
        let mut shape = 1.0;
        let mut alpha = 1.0;
        if let Some(ranges) = &case.key {
            let offset = pixel * usize::from(image.components);
            if ranges.iter().enumerate().all(|(i, &[lo, hi])| {
                image
                    .samples
                    .get(offset + i)
                    .is_some_and(|v| v >= lo && v <= hi)
            }) {
                shape = 0.0;
                alpha = 0.0;
            }
        }
        if let MaskKind::Embedded(plane) = &image.mask {
            let opacity = f64::from(plane.samples.get(pixel).unwrap())
                / f64::from(u16::MAX >> (16 - plane.bits_per_component));
            alpha *= opacity;
            if case.alpha_is_shape {
                shape *= opacity;
            }
            if plane.premultiplied {
                for c in values.iter_mut().take(count) {
                    *c = if opacity > 0.0 { *c / opacity } else { 0.0 };
                }
            }
        }
        if let Some((mask, decode, MaskUse::Soft(Some(matte)))) = &case.mask {
            let opacity = mask_alpha(mask, *decode, pixel);
            for (value, &matte) in values.iter_mut().zip(matte) {
                *value = if opacity > 0.0 {
                    matte + (*value - matte) / opacity
                } else {
                    0.0
                };
            }
        }
        let mut color = [0.0; 4];
        if let Some(convert) = convert {
            convert.convert(&values[..count], &mut color[..convert.channels()]);
        }
        Texel {
            color: color.map(|v| v * alpha),
            alpha,
            shape,
        }
    }

    /// One pixel as the previous renderer composited it, before coverage.
    pub(super) fn pixel(&mut self, x: u32, y: u32) -> Texel {
        let (case, convert) = (self.case, self.convert.as_deref());
        let mut value = self
            .image
            .sample(x, y, |pixel| Self::texel(case, convert, pixel));
        if let (Some((mask, decode, kind)), Some(sampler)) = (&case.mask, &mut self.mask) {
            let factor = sampler.sample(x, y, |pixel| mask_alpha(mask, *decode, pixel));
            value.color = value.color.map(|v| v * factor);
            value.alpha *= factor;
            if matches!(kind, MaskUse::Stencil) || case.alpha_is_shape {
                value.shape *= factor;
            }
        }
        Texel {
            color: if value.alpha == 1.0 {
                value.color
            } else if value.alpha > 0.0 {
                value.color.map(|v| v / value.alpha)
            } else {
                [0.0; 4]
            },
            ..value
        }
    }
}
