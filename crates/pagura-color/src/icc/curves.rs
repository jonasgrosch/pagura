//! Forward curves: ICC.1:2010 10.5 and 10.16.

use super::{Budget, fixed, sample_index, slice, u16_at, u32_at, unsupported, usize_f64};
use pagura_core::{Error, Result};

#[derive(Debug, Clone)]
pub(super) enum Curve {
    Identity,
    Gamma(f64),
    Samples(Vec<f64>),
    Parametric(u16, [f64; 7]),
}
impl Curve {
    pub(super) fn retained_bytes(&self) -> usize {
        if let Self::Samples(v) = self {
            v.capacity().saturating_mul(size_of::<f64>())
        } else {
            0
        }
    }
    pub(super) fn extent(data: &[u8]) -> Result<usize> {
        match slice(data, 0, 4)? {
            b"curv" => usize::try_from(u32_at(data, 8)?)
                .ok()
                .and_then(|n| n.checked_mul(2))
                .and_then(|n| n.checked_add(12))
                .ok_or(Error::limit("ICC curve extent overflow")),
            b"para" => {
                let count = match u16_at(data, 8)? {
                    0 => 1,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    4 => 7,
                    _ => return Err(unsupported("unsupported ICC parametric curve")),
                };
                Ok(12 + 4 * count)
            }
            _ => Err(unsupported("unsupported ICC curve tag type")),
        }
    }

    pub(super) fn parse(data: &[u8], budget: &mut Budget) -> Result<Self> {
        super::format::zeros(slice(data, 4, 4)?)?;
        match slice(data, 0, 4)? {
            b"curv" => {
                let count = usize::try_from(u32_at(data, 8)?)
                    .map_err(|_| Error::limit("ICC curve size overflow"))?;
                budget.charge(count)?;
                if data.len() != Self::extent(data)? {
                    return Err(Error::syntax("invalid ICC curve extent"));
                }
                slice(
                    data,
                    12,
                    count
                        .checked_mul(2)
                        .ok_or(Error::limit("ICC curve size overflow"))?,
                )?;
                match count {
                    0 => Ok(Self::Identity),
                    1 => {
                        let gamma = f64::from(u16_at(data, 12)?) / 256.0;
                        if gamma <= 0.0 {
                            return Err(Error::syntax("invalid ICC curve gamma"));
                        }
                        Ok(Self::Gamma(gamma))
                    }
                    _ => {
                        let mut samples = Vec::new();
                        samples
                            .try_reserve_exact(count)
                            .map_err(|_| Error::limit("ICC curve allocation failed"))?;
                        for i in 0..count {
                            samples.push(f64::from(u16_at(data, 12 + 2 * i)?) / 65535.0);
                        }
                        Ok(Self::Samples(samples))
                    }
                }
            }
            b"para" => {
                super::format::zeros(slice(data, 10, 2)?)?;
                if data.len() != Self::extent(data)? {
                    return Err(Error::syntax("invalid ICC parametric curve extent"));
                }
                let kind = u16_at(data, 8)?;
                let count = match kind {
                    0 => 1,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    4 => 7,
                    _ => return Err(unsupported("unsupported ICC parametric curve")),
                };
                let mut params = [0.0; 7];
                for (i, p) in params.iter_mut().enumerate().take(count) {
                    *p = fixed(data, 12 + 4 * i)?;
                }
                if params[0] <= 0.0 || (kind > 0 && params[1] <= 0.0) {
                    return Err(Error::syntax("invalid ICC parametric curve"));
                }
                if kind >= 3 && params[4] <= 1.0 && params[1] * params[4].max(0.0) + params[2] < 0.0
                {
                    return Err(Error::syntax("ICC curve power includes negative inputs"));
                }
                Ok(Self::Parametric(kind, params))
            }
            _ => Err(unsupported("unsupported ICC curve tag type")),
        }
    }
    pub(super) fn evaluate(&self, x: f64) -> Result<f64> {
        let value = match self {
            Self::Identity => x,
            Self::Gamma(g) => x.powf(*g),
            Self::Samples(values) => {
                let position = x * usize_f64(values.len() - 1);
                let low = sample_index(position).min(values.len() - 1);
                let high = (low + 1).min(values.len() - 1);
                values[low] + (position - usize_f64(low)) * (values[high] - values[low])
            }
            // Table 65's type 4 offset and lower inequalities are inconsistent
            // with its parameter list. Use +e above d and c*x+f below d, and
            // complementary lower branches (the conventional ICC model).
            Self::Parametric(kind, [g, a, b, c, d, e, f]) => match kind {
                0 => x.powf(*g),
                1 => {
                    if a * x >= -b {
                        (a * x + b).max(0.0).powf(*g)
                    } else {
                        0.0
                    }
                }
                2 => {
                    if a * x >= -b {
                        (a * x + b).max(0.0).powf(*g) + c
                    } else {
                        *c
                    }
                }
                3 => {
                    if x >= *d {
                        (a * x + b).powf(*g)
                    } else {
                        c * x
                    }
                }
                _ => {
                    if x >= *d {
                        (a * x + b).powf(*g) + e
                    } else {
                        c * x + f
                    }
                }
            },
        };
        // Positive overflow has a defined clipped value (10.16). NaN and
        // negative infinity still indicate genuinely undefined arithmetic.
        if value.is_nan() || value == f64::NEG_INFINITY {
            return Err(Error::syntax("undefined ICC curve arithmetic"));
        }
        Ok(value.clamp(0.0, 1.0))
    }
}
