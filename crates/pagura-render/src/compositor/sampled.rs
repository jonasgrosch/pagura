//! Affine image sampling. Magnification uses nearest neighbours or bilinear
//! interpolation of associated colour and alpha. Minification integrates the
//! source-pixel rectangles intersecting the inverse-mapped device-pixel polygon.
//! Axis-aligned minification uses the streaming separable reduction.
//! Integer aligned scales and quarter turns select source pixels exactly.
use pagura_core::{Error, Result};
use pagura_raster::{Point, Transform};

#[derive(Clone, Copy, Default)]
pub(super) struct Texel {
    pub(super) color: [f64; 4],
    pub(super) alpha: f64,
    pub(super) shape: f64,
}
/// One ordered weighted sum. Scalar masks and colour samples share geometry.
pub(super) trait Weighted: Copy + Default {
    fn add(&mut self, value: Self, weight: f64);
    fn scale(self, weight: f64) -> Self;
}
impl Weighted for f64 {
    #[inline]
    fn add(&mut self, value: Self, weight: f64) {
        *self += value * weight;
    }
    #[inline]
    fn scale(self, weight: f64) -> Self {
        self * weight
    }
}
impl Weighted for Texel {
    #[inline]
    fn add(&mut self, value: Self, weight: f64) {
        for (sum, c) in self.color.iter_mut().zip(value.color) {
            *sum += c * weight;
        }
        self.alpha += value.alpha * weight;
        self.shape += value.shape * weight;
    }
    #[inline]
    fn scale(mut self, weight: f64) -> Self {
        self.color = self.color.map(|v| v * weight);
        self.alpha *= weight;
        self.shape *= weight;
        self
    }
}

pub(super) struct Sampler {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) interpolate: bool,
    pub(super) inverse: Transform,
}
impl Sampler {
    pub(super) fn minifying(&self) -> bool {
        let t = self.inverse;
        t.a.hypot(t.b) > 1.0 + 1e-9 || t.c.hypot(t.d) > 1.0 + 1e-9
    }

    /// The source position of a device pixel's centre.
    pub(super) fn center(&self, x: u32, y: u32) -> Point {
        Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5) * self.inverse
    }

    pub(super) fn nearest(&self, x: u32, y: u32) -> Result<usize> {
        let center = self.center(x, y);
        self.index(center.x.floor(), center.y.floor())
    }

    pub(super) fn magnify<T: Weighted>(
        &self,
        x: u32,
        y: u32,
        work: &mut usize,
        maximum: usize,
        mut texel: impl FnMut(usize) -> Result<T>,
    ) -> Result<T> {
        if !self.interpolate {
            charge(work, 1, maximum)?;
            return texel(self.nearest(x, y)?);
        }
        let center = self.center(x, y);
        charge(work, 4, maximum)?;
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
            out.add(texel(self.index(x, y)?)?, w);
        }
        Ok(out)
    }
    pub(super) fn area<T: Weighted>(
        &self,
        x: u32,
        y: u32,
        work: &mut usize,
        maximum: usize,
        mut texel: impl FnMut(usize) -> Result<T>,
    ) -> Result<T> {
        let (x, y) = (f64::from(x), f64::from(y));
        let polygon = [
            Point::new(x, y),
            Point::new(x + 1.0, y),
            Point::new(x + 1.0, y + 1.0),
            Point::new(x, y + 1.0),
        ]
        .map(|p| p * self.inverse);
        if polygon.iter().any(|p| !p.is_finite()) {
            return Err(Error::limit("image sampling transform overflow"));
        }
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
        let count =
            usize::try_from(u64::from(x1.saturating_sub(x0)) * u64::from(y1.saturating_sub(y0)))
                .map_err(|_| Error::limit("image sampling work overflow"))?;
        charge(work, count, maximum)?;
        let mut out = T::default();
        let mut weight = 0.0;
        for row in y0..y1 {
            for col in x0..x1 {
                let area = intersection(&polygon, f64::from(col), f64::from(row));
                if area > 0.0 {
                    out.add(texel(self.index(f64::from(col), f64::from(row))?)?, area);
                    weight += area;
                }
            }
        }
        Ok(if weight > 0.0 {
            out.scale(1.0 / weight)
        } else {
            out
        })
    }
    fn index(&self, x: f64, y: f64) -> Result<usize> {
        let x = coordinate(x, self.width.saturating_sub(1));
        let y = coordinate(y, self.height.saturating_sub(1));
        usize::try_from(u64::from(y) * u64::from(self.width) + u64::from(x))
            .map_err(|_| Error::limit("image index exceeds usize"))
    }
}
pub(super) fn charge(work: &mut usize, count: usize, maximum: usize) -> Result<()> {
    *work = work
        .checked_add(count)
        .filter(|&n| n <= maximum)
        .ok_or(Error::limit("image sampling work exceeded"))?;
    Ok(())
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sample coordinates clamped to u32 image bounds"
)]
pub(super) fn coordinate(value: f64, maximum: u32) -> u32 {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separable_reduction_matches_polygon_integration() {
        for inverse in [
            Transform::new(2.4, 0.0, 0.0, 3.2, -1.25, -2.75),
            Transform::new(-2.4, 0.0, 0.0, -3.2, 31.1, 29.7),
            Transform::new(0.0, -3.2, 2.4, 0.0, -1.25, 29.7),
        ] {
            let sampler = Sampler {
                width: 32,
                height: 32,
                interpolate: false,
                inverse,
            };
            let mut sampler = sampler.prepare(0, 10, usize::MAX).unwrap();
            for y in 0..10 {
                for x in 0..10 {
                    let points = [
                        Point::new(f64::from(x), f64::from(y)),
                        Point::new(f64::from(x + 1), f64::from(y)),
                        Point::new(f64::from(x + 1), f64::from(y + 1)),
                        Point::new(f64::from(x), f64::from(y + 1)),
                    ]
                    .map(|p| p * inverse);
                    let mut sum = 0.0;
                    let mut weight = 0.0;
                    for row in 0..32 {
                        for col in 0..32 {
                            let area = intersection(&points, f64::from(col), f64::from(row));
                            sum += area * f64::from((row + col) % 2);
                            weight += area;
                        }
                    }
                    let mut work = 0;
                    let sample = sampler
                        .sample(x, y, &mut work, usize::MAX, |index| {
                            let value = ((index / 32 + index % 32) % 2) as f64;
                            Ok(Texel {
                                color: [value; 4],
                                alpha: 1.0,
                                shape: 1.0,
                            })
                        })
                        .unwrap();
                    let expected = if weight > 0.0 { sum / weight } else { 0.0 };
                    assert!(
                        (sample.color[0] - expected).abs() < 1e-12,
                        "{inverse:?} ({x},{y}) {} {expected}",
                        sample.color[0]
                    );
                }
            }
        }
    }
}
