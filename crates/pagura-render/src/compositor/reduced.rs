//! Separable area integration (ADR 0010), streamed in device scan order.
//! Two boundary rows per column suffice even for mixed shrinking/enlarging.
//! Only requested pixels are evaluated; clipping never evaluates hidden colours.
use super::sampled::{Sampler, Texel, charge, coordinate};
use pagura_core::{Error, Result};

#[derive(Clone, Copy, Default)]
struct Span {
    first: u32,
    end: u32,
    low: f64,
    high: f64,
}

impl Span {
    fn new(pixel: u32, scale: f64, offset: f64, maximum: u32) -> Result<Self> {
        let a = f64::from(pixel) * scale + offset;
        let b = (f64::from(pixel) + 1.0) * scale + offset;
        if !a.is_finite() || !b.is_finite() {
            return Err(Error::limit("image sampling transform overflow"));
        }
        let low = a.min(b).clamp(0.0, f64::from(maximum));
        let high = a.max(b).clamp(0.0, f64::from(maximum));
        Ok(Self {
            first: coordinate(low.floor(), maximum),
            end: coordinate(if high > low { high.ceil() } else { low.floor() }, maximum),
            low,
            high,
        })
    }

    fn weight(self, cell: u32) -> f64 {
        (self.high.min(f64::from(cell) + 1.0) - self.low.max(f64::from(cell))).max(0.0)
    }

    fn normalize(self, value: Texel) -> Texel {
        let width = self.high - self.low;
        value.scale(if width > 0.0 { 1.0 / width } else { 0.0 })
    }
}

#[derive(Default)]
struct Column {
    span: Span,
    rows: [Option<(u32, Texel)>; 2],
}

pub(super) struct Prepared {
    sampler: Sampler,
    columns: Vec<Column>,
    origin: u32,
    swapped: bool,
    vertical: Option<(u32, Span)>,
}

impl Sampler {
    pub(super) fn prepare(self, origin: u32, width: u32, bytes: usize) -> Result<Prepared> {
        let t = self.inverse;
        if !t.is_finite() {
            return Err(Error::limit("image sampling transform overflow"));
        }
        let mut out = Prepared {
            swapped: t.a == 0.0 && t.d == 0.0,
            sampler: self,
            columns: Vec::new(),
            origin,
            vertical: None,
        };
        if (t.a.hypot(t.b) > 1.0 + 1e-9 || t.c.hypot(t.d) > 1.0 + 1e-9)
            && ((t.b == 0.0 && t.c == 0.0) || out.swapped)
        {
            let count = usize::try_from(width)
                .ok()
                .filter(|&n| {
                    n.checked_mul(size_of::<Column>())
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

impl Prepared {
    pub(super) fn bytes(&self) -> usize {
        self.columns.capacity() * size_of::<Column>()
    }

    pub(super) fn sample(
        &mut self,
        x: u32,
        y: u32,
        work: &mut usize,
        maximum: usize,
        texel: impl FnMut(usize) -> Result<Texel>,
    ) -> Result<Texel> {
        if self.columns.is_empty() {
            return self.sampler.sample(x, y, work, maximum, texel);
        }
        self.reduce(x, y, work, maximum, texel)
    }

    fn reduce(
        &mut self,
        x: u32,
        y: u32,
        work: &mut usize,
        maximum: usize,
        mut texel: impl FnMut(usize) -> Result<Texel>,
    ) -> Result<Texel> {
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
        let column = x
            .checked_sub(self.origin)
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| self.columns.get_mut(n))
            .ok_or(Error::invalid_argument("pixel outside image reduction"))?;
        charge(
            work,
            usize::try_from(span.end - span.first)
                .map_err(|_| Error::limit("image row work exceeds usize"))?,
            maximum,
        )?;
        let mut out = Texel::default();
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
                    let mut value = Texel::default();
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
