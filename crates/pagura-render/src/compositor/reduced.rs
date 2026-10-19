//! Separable area integration (ADR 0010), streamed in device scan order.
//! Two boundary rows per column suffice even for mixed shrinking/enlarging.
//! Only requested pixels are evaluated; clipping never evaluates hidden colours.
use super::sampled::{Sampler, Texel, Weighted, charge, coordinate};
use pagura_core::{Error, Result};

/// The source cells under one device pixel along one axis: `first..end`, with
/// fractional end weights, unit interior weights and the reciprocal extent.
#[derive(Clone, Copy, Default)]
pub(super) struct Span {
    pub(super) first: u32,
    pub(super) end: u32,
    pub(super) first_weight: f64,
    pub(super) last_weight: f64,
    pub(super) normalization: f64,
}

impl Span {
    pub(super) fn new(pixel: u32, scale: f64, offset: f64, maximum: u32) -> Result<Self> {
        let a = f64::from(pixel) * scale + offset;
        let b = (f64::from(pixel) + 1.0) * scale + offset;
        if !a.is_finite() || !b.is_finite() {
            return Err(Error::limit("image sampling transform overflow"));
        }
        let low = a.min(b).clamp(0.0, f64::from(maximum));
        let high = a.max(b).clamp(0.0, f64::from(maximum));
        let first = coordinate(low.floor(), maximum);
        let end = coordinate(if high > low { high.ceil() } else { low.floor() }, maximum);
        let weight =
            |cell: u32| (high.min(f64::from(cell) + 1.0) - low.max(f64::from(cell))).max(0.0);
        Ok(Self {
            first,
            end,
            first_weight: weight(first),
            last_weight: weight(end.saturating_sub(1)),
            normalization: if high > low { 1.0 / (high - low) } else { 0.0 },
        })
    }

    /// Whether no source cell lies under the pixel, as when its extent
    /// rounds to a point; the sampler then contributes zero opacity.
    pub(super) fn is_empty(&self) -> bool {
        self.end <= self.first
    }

    pub(super) fn weight(self, cell: u32) -> f64 {
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

#[derive(Default)]
struct Column<T: Weighted> {
    span: Span,
    rows: [Option<(u32, T)>; 2],
}

pub(super) struct Prepared<T: Weighted = Texel> {
    sampler: Sampler,
    columns: Vec<Column<T>>,
    origin: u32,
    swapped: bool,
    vertical: Option<(u32, Span)>,
    minifying: bool,
}

impl Sampler {
    pub(super) fn prepare(self, origin: u32, width: u32, bytes: usize) -> Result<Prepared> {
        self.prepare_sample(origin, width, bytes)
    }
    pub(super) fn prepare_alpha(
        self,
        origin: u32,
        width: u32,
        bytes: usize,
    ) -> Result<Prepared<f64>> {
        self.prepare_sample(origin, width, bytes)
    }
    fn prepare_sample<T: Weighted>(
        self,
        origin: u32,
        width: u32,
        bytes: usize,
    ) -> Result<Prepared<T>> {
        let t = self.inverse;
        if !t.is_finite() {
            return Err(Error::limit("image sampling transform overflow"));
        }
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
            let count = usize::try_from(width)
                .ok()
                .filter(|&n| {
                    n.checked_mul(size_of::<Column<T>>())
                        .is_some_and(|n| n <= bytes)
                })
                .ok_or(Error::limit("image reduction memory exceeded"))?;
            out.columns
                .try_reserve_exact(count)
                .map_err(|_| Error::limit("image reduction allocation failed"))?;
            let (scale, offset, maximum) = if out.swapped {
                (t.b, t.f, out.sampler.height)
            } else {
                (t.a, t.e, out.sampler.width)
            };
            for index in 0..width {
                let pixel = origin
                    .checked_add(index)
                    .ok_or(Error::limit("image column overflow"))?;
                out.columns.push(Column {
                    span: Span::new(pixel, scale, offset, maximum)?,
                    ..Default::default()
                });
            }
        }
        Ok(out)
    }
}

impl<T: Weighted> Prepared<T> {
    pub(super) fn bytes(&self) -> usize {
        self.columns.capacity() * size_of::<Column<T>>()
    }

    pub(super) fn sample(
        &mut self,
        x: u32,
        y: u32,
        work: &mut usize,
        maximum: usize,
        texel: impl FnMut(usize) -> Result<T>,
    ) -> Result<T> {
        if !self.columns.is_empty() {
            self.reduce(x, y, work, maximum, texel)
        } else if self.minifying {
            self.sampler.area(x, y, work, maximum, texel)
        } else {
            self.sampler.magnify(x, y, work, maximum, texel)
        }
    }

    fn reduce(
        &mut self,
        x: u32,
        y: u32,
        work: &mut usize,
        maximum: usize,
        mut texel: impl FnMut(usize) -> Result<T>,
    ) -> Result<T> {
        let t = self.sampler.inverse;
        if self.vertical.is_none_or(|(row, _)| row != y) {
            let (scale, offset, bound) = if self.swapped {
                (t.c, t.e, self.sampler.width)
            } else {
                (t.d, t.f, self.sampler.height)
            };
            self.vertical = Some((y, Span::new(y, scale, offset, bound)?));
        }
        let span = self
            .vertical
            .ok_or(Error::invalid_argument("missing image row"))?
            .1;
        let index = x
            .checked_sub(self.origin)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(Error::invalid_argument("pixel outside image reduction"))?;
        let column = self
            .columns
            .get_mut(index)
            .ok_or(Error::invalid_argument("pixel outside image reduction"))?;
        charge(
            work,
            usize::try_from(span.end - span.first)
                .map_err(|_| Error::limit("image row work exceeds usize"))?,
            maximum,
        )?;
        let mut out = T::default();
        for step in span.first..span.end {
            // Retain the boundary reached next in either scan direction.
            let row = if (if self.swapped { t.c } else { t.d }) < 0.0 {
                span.end - 1 - (step - span.first)
            } else {
                step
            };
            let value =
                if let Some((_, value)) = column.rows.iter().flatten().find(|(r, _)| *r == row) {
                    *value
                } else {
                    charge(
                        work,
                        usize::try_from(column.span.end - column.span.first)
                            .map_err(|_| Error::limit("image column work exceeds usize"))?,
                        maximum,
                    )?;
                    let mut value = T::default();
                    for col in column.span.first..column.span.end {
                        let (col, row) = if self.swapped { (row, col) } else { (col, row) };
                        let index = usize::try_from(
                            u64::from(row) * u64::from(self.sampler.width) + u64::from(col),
                        )
                        .map_err(|_| Error::limit("image index exceeds usize"))?;
                        let cell = if self.swapped { row } else { col };
                        value.add(texel(index)?, column.span.weight(cell));
                    }
                    let value = column.span.normalize(value);
                    column.rows.swap(0, 1);
                    column.rows[0] = Some((row, value));
                    value
                };
            out.add(value, span.weight(row));
        }
        Ok(span.normalize(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::Transform;

    #[test]
    fn streamed_rows_preserve_exact_samples_across_chunks_and_flips() {
        for inverse in [
            Transform::new(2.0, 0.0, 0.0, 0.25, 0.0, 0.0),
            Transform::new(-2.0, 0.0, 0.0, -0.25, 8.0, 8.0),
            Transform::new(0.0, -2.0, 0.25, 0.0, 0.0, 8.0),
            Transform::new(2.25, 0.0, 0.0, 0.3, -0.5, -0.25),
        ] {
            let make = || Sampler {
                width: 8,
                height: 8,
                interpolate: true,
                inverse,
            };
            let mut sampler = make().prepare(0, 4, usize::MAX).unwrap();
            let mut visits = [0; 64];
            let texel = |index: usize| Texel {
                color: [f64::from(u32::try_from(index).unwrap()).powi(2) / 4096.0; 4],
                alpha: 0.5,
                shape: 1.0,
            };
            for y0 in (0..32).step_by(3) {
                for x0 in [0, 2] {
                    let mut chunk = make().prepare(x0, 2, usize::MAX).unwrap();
                    for y in y0..(y0 + 3).min(32) {
                        for x in x0..x0 + 2 {
                            let actual = sampler
                                .sample(x, y, &mut 0, usize::MAX, |index| {
                                    visits[index] += 1;
                                    Ok(texel(index))
                                })
                                .unwrap();
                            let expected = chunk
                                .sample(x, y, &mut 0, usize::MAX, |index| Ok(texel(index)))
                                .unwrap();
                            assert_eq!(
                                actual.color.map(f64::to_bits),
                                expected.color.map(f64::to_bits)
                            );
                            assert_eq!(actual.alpha.to_bits(), expected.alpha.to_bits());
                            assert_eq!(actual.shape.to_bits(), expected.shape.to_bits());
                        }
                    }
                }
            }
            if inverse.a.abs() == 2.0 || inverse.b.abs() == 2.0 {
                // Two source columns contribute to each destination column;
                // each horizontal integral is evaluated only once per row.
                assert_eq!(visits, [1; 64]);
            }
        }
    }

    #[test]
    fn large_area_half_ties_match_independent_rational_vectors() {
        let input = pagura_testkit::fixture("paint", "image-area-thresholds.txt");
        for line in std::str::from_utf8(&input)
            .unwrap()
            .lines()
            .filter(|s| !s.starts_with('#'))
        {
            let values: Vec<u32> = line
                .split_whitespace()
                .map(|s| s.parse().unwrap())
                .collect();
            let [side, low, expected] = values[..] else {
                panic!("invalid area vector");
            };
            let mut sampler = Sampler {
                width: side,
                height: side,
                interpolate: false,
                inverse: Transform::new(f64::from(side), 0.0, 0.0, f64::from(side), 0.0, 0.0),
            }
            .prepare(0, 1, usize::MAX)
            .unwrap();
            let value = sampler
                .sample(0, 0, &mut 0, usize::MAX, |index| {
                    let high = u32::try_from(index % 2).unwrap();
                    Ok(Texel {
                        color: [f64::from(low + high) / 255.0; 4],
                        alpha: 1.0,
                        shape: 1.0,
                    })
                })
                .unwrap();
            assert_eq!(
                u32::from(super::super::quantize(value.color[0])),
                expected,
                "{side} {low}"
            );
        }
    }

    #[test]
    fn reduction_is_bounded_and_does_not_evaluate_unrequested_columns() {
        let make = || Sampler {
            width: 8,
            height: 8,
            interpolate: false,
            inverse: Transform::new(2.0, 0.0, 0.0, 0.5, 0.0, 0.0),
        };
        assert!(make().prepare(0, 4, 0).is_err());
        let mut sampler = make().prepare(0, 4, usize::MAX).unwrap();
        let mut visits = 0;
        for y in 0..2 {
            sampler
                .sample(1, y, &mut 0, usize::MAX, |index| {
                    assert!((2..4).contains(&(index % 8)));
                    assert!(index < 8);
                    visits += 1;
                    Ok(Texel::default())
                })
                .unwrap();
        }
        assert_eq!(visits, 2);
        // Rounded endpoints can coincide during strongly anisotropic scaling.
        // Empty intervals must never evaluate zero-weight colour functions.
        let mut collapsed = Sampler {
            width: 2,
            height: 1,
            interpolate: false,
            inverse: Transform::new(2.0, 0.0, 0.0, 1e-20, 0.0, 0.5),
        }
        .prepare(0, 1, usize::MAX)
        .unwrap();
        let value = collapsed
            .sample(0, 0, &mut 0, usize::MAX, |_| {
                panic!("empty intervals must not evaluate colours")
            })
            .unwrap();
        assert_eq!(value.alpha, 0.0);
        assert_eq!(value.shape, 0.0);

        assert!(
            make()
                .prepare(0, 4, usize::MAX)
                .unwrap()
                .sample(0, 0, &mut 0, 0, |_| panic!("work must be reserved first"))
                .is_err()
        );
    }
}

#[cfg(test)]
mod scalar_tests {
    use super::*;
    use pagura_raster::Transform;
    #[test]
    fn scalar_masks_keep_alpha_bits_visits_and_work_for_every_sampling_geometry() {
        for inverse in [
            Transform::new(2.4, 0.0, 0.0, 3.2, -1.25, -2.75),
            Transform::new(-2.4, 0.0, 0.0, -3.2, 31.1, 29.7),
            Transform::new(0.0, -3.2, 2.4, 0.0, -1.25, 29.7),
            Transform::new(1.8, 0.2, 0.3, 1.6, -1.0, -2.0),
            Transform::new(0.5, 0.0, 0.0, 0.7, -0.25, 0.125),
        ] {
            for interpolate in [false, true] {
                let make = || Sampler {
                    width: 32,
                    height: 32,
                    interpolate,
                    inverse,
                };
                let mut color = make().prepare(0, 10, usize::MAX).unwrap();
                let mut alpha = make().prepare_alpha(0, 10, usize::MAX).unwrap();
                if color.bytes() > 0 {
                    assert!(alpha.bytes() < color.bytes());
                    assert!(make().prepare_alpha(0, 10, alpha.bytes()).is_ok());
                    assert!(make().prepare(0, 10, alpha.bytes()).is_err());
                }
                for y in 0..10 {
                    for x in 0..10 {
                        let (mut work_a, mut work_b) = (0, 0);
                        let (mut visits_a, mut visits_b) = (Vec::new(), Vec::new());
                        let sample = |index: usize| {
                            f64::from(u8::try_from((index * 73 + 19) % 256).unwrap()) / 255.0
                        };
                        let expected = color
                            .sample(x, y, &mut work_a, usize::MAX, |i| {
                                visits_a.push(i);
                                Ok(Texel {
                                    alpha: sample(i),
                                    color: [0.0; 4],
                                    shape: 1.0,
                                })
                            })
                            .unwrap();
                        let actual = alpha
                            .sample(x, y, &mut work_b, usize::MAX, |i| {
                                visits_b.push(i);
                                Ok(sample(i))
                            })
                            .unwrap();
                        assert_eq!(actual.to_bits(), expected.alpha.to_bits());
                        assert_eq!(work_a, work_b);
                        assert_eq!(visits_a, visits_b);
                    }
                }
            }
        }
    }

    #[test]
    fn scalar_masks_match_independent_box_and_bilinear_rationals() {
        let make = |scale| Sampler {
            width: 2,
            height: 2,
            interpolate: true,
            inverse: Transform::scale(scale, scale),
        };
        let levels = [0_u8, 64, 128, 255];
        let mut box_filter = make(2.0).prepare_alpha(0, 1, 4096).unwrap();
        let value = box_filter
            .sample(0, 0, &mut 0, 100, |i| Ok(f64::from(levels[i]) / 255.0))
            .unwrap();
        assert_eq!((value * 255.0).round(), 112.0); // (0+64+128+255)/4 = 111.75.
        let mut bilinear = make(0.5).prepare_alpha(0, 4, 4096).unwrap();
        let value = bilinear
            .sample(1, 1, &mut 0, 100, |i| Ok(f64::from(levels[i]) / 255.0))
            .unwrap();
        assert_eq!((value * 255.0).round(), 52.0); // (9*0+3*64+3*128+255)/16.
    }
}
