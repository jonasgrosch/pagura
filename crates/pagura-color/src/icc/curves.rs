//! Forward curves: ICC.1:2010 10.5 and 10.16.

use super::{Arithmetic, At, Budget, format::zeros, sample_index, unsupported, usize_f64};
use crate::affine::{Affine, Undecided};
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Result};

#[derive(Debug, Clone)]
pub(super) enum Curve {
    Identity,
    Gamma(f64),
    Samples(Vec<f64>),
    Parametric(Parametric),
}

/// A parametricCurveType function (10.16, Table 65) with its parameters.
#[derive(Debug, Clone, Copy)]
pub(super) enum Parametric {
    /// `Y = X^g`.
    Power { g: f64 },
    /// `Y = (aX + b)^g` above `-b/a`, else 0 (CIE 122-1996).
    Cie122 { g: f64, a: f64, b: f64 },
    /// `Y = (aX + b)^g + c` above `-b/a`, else `c` (IEC 61966-3).
    Iec61966 { g: f64, a: f64, b: f64, c: f64 },
    /// `Y = (aX + b)^g` from `d`, else `cX` (IEC 61966-2.1, sRGB).
    Srgb {
        g: f64,
        a: f64,
        b: f64,
        c: f64,
        d: f64,
    },
    /// `Y = (aX + b)^g + e` from `d`, else `cX + f`.
    Full {
        g: f64,
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        e: f64,
        f: f64,
    },
}

/// The parameter count of each parametric function type (Table 65).
fn parameters(kind: u16) -> Result<usize> {
    Ok(match kind {
        0 => 1,
        1 => 3,
        2 => 4,
        3 => 5,
        4 => 7,
        _ => return Err(unsupported("unsupported ICC parametric curve")),
    })
}

impl Curve {
    pub(super) fn retained_bytes(&self) -> usize {
        if let Self::Samples(v) = self {
            v.capacity().saturating_mul(size_of::<f64>())
        } else {
            0
        }
    }
    /// The encoded size of the curve starting `data`, before alignment.
    pub(super) fn extent(data: &Cursor<'_>) -> Result<usize> {
        match data.bytes(0, 4)? {
            b"curv" => usize::try_from(data.u32_at(8)?)
                .ok()
                .and_then(|n| n.checked_mul(2))
                .and_then(|n| n.checked_add(12))
                .ok_or(Error::limit("ICC curve extent overflow")),
            b"para" => Ok(12 + 4 * parameters(data.u16_at(8)?)?),
            _ => Err(unsupported("unsupported ICC curve tag type")),
        }
    }

    pub(super) fn parse(data: &Cursor<'_>, budget: &mut Budget) -> Result<Self> {
        zeros(data, 4, 4)?;
        let length = data.data().len();
        match data.bytes(0, 4)? {
            b"curv" => {
                let count = usize::try_from(data.u32_at(8)?)
                    .map_err(|_| Error::limit("ICC curve size overflow"))?;
                budget.values(count)?;
                if length != Self::extent(data)? {
                    return Err(Error::syntax("invalid ICC curve extent"));
                }
                let mut samples = data.cursor_at(12)?;
                match count {
                    0 => Ok(Self::Identity),
                    1 => {
                        let gamma = f64::from(samples.u16()?) / 256.0;
                        if gamma <= 0.0 {
                            return Err(Error::syntax("invalid ICC curve gamma"));
                        }
                        Ok(Self::Gamma(gamma))
                    }
                    _ => {
                        let mut values = Vec::new();
                        values
                            .try_reserve_exact(count)
                            .map_err(|_| Error::limit("ICC curve allocation failed"))?;
                        for _ in 0..count {
                            values.push(f64::from(samples.u16()?) / 65535.0);
                        }
                        Ok(Self::Samples(values))
                    }
                }
            }
            b"para" => {
                zeros(data, 10, 2)?;
                if length != Self::extent(data)? {
                    return Err(Error::syntax("invalid ICC parametric curve extent"));
                }
                let kind = data.u16_at(8)?;
                let mut values = data.cursor_at(12)?;
                let mut p = [0.0; 7];
                for p in p.iter_mut().take(parameters(kind)?) {
                    *p = values.fixed()?;
                }
                let [g, a, b, c, d, e, f] = p;
                if g <= 0.0 || (kind > 0 && a <= 0.0) {
                    return Err(Error::syntax("invalid ICC parametric curve"));
                }
                if kind >= 3 && d <= 1.0 && a * d.max(0.0) + b < 0.0 {
                    return Err(Error::syntax("ICC curve power includes negative inputs"));
                }
                Ok(Self::Parametric(match kind {
                    0 => Parametric::Power { g },
                    1 => Parametric::Cie122 { g, a, b },
                    2 => Parametric::Iec61966 { g, a, b, c },
                    3 => Parametric::Srgb { g, a, b, c, d },
                    _ => Parametric::Full {
                        g,
                        a,
                        b,
                        c,
                        d,
                        e,
                        f,
                    },
                }))
            }
            _ => Err(unsupported("unsupported ICC curve tag type")),
        }
    }
    /// The curve at `x`, clipped to `0..=1`. Undefined arithmetic (NaN or
    /// negative infinity) gives 0 and is recorded in `arithmetic`.
    pub(super) fn evaluate(&self, x: f64, arithmetic: &mut Arithmetic) -> f64 {
        match self {
            Self::Identity => finish(x, arithmetic),
            Self::Gamma(g) => finish(x.powf(*g), arithmetic),
            Self::Samples(values) => Self::sampled(values, x, arithmetic),
            Self::Parametric(curve) => finish(curve.evaluate(x), arithmetic),
        }
    }

    /// [`Self::evaluate`] of every value in place, the curve's kind chosen
    /// once.
    pub(super) fn evaluate_line(&self, values: &mut [f64], arithmetic: &mut Arithmetic) {
        match self {
            Self::Samples(samples) => {
                for value in values {
                    *value = Self::sampled(samples, *value, arithmetic);
                }
            }
            _ => {
                for value in values {
                    *value = self.evaluate(*value, arithmetic);
                }
            }
        }
    }

    /// Encloses what [`Self::evaluate`] computes for every value of `x`
    /// (see [`crate::affine`]), for the curves that interpolate: identity
    /// and sampled curves.
    pub(super) fn enclose(&self, x: Affine) -> Result<Affine, Undecided> {
        let Self::Samples(values) = self else {
            return match self {
                Self::Identity => x.clamp(0.0, 1.0),
                _ => Err(Undecided::Unsupported),
            };
        };
        let last = values.len() - 1;
        let position = x.scale(usize_f64(last));
        let [first, end] = position.cells(last)?;
        let mut value = Affine::point(0.0);
        for low in first..=end {
            let high = (low + 1).min(last);
            let cell = Affine::point(values[low]).add(
                position
                    .sub(Affine::point(usize_f64(low)))
                    .scale(values[high] - values[low]),
            );
            value = if low == first { cell } else { value.join(cell) };
        }
        value.clamp(0.0, 1.0)
    }

    /// [`Self::evaluate`] of a sampled curve.
    #[inline]
    fn sampled(values: &[f64], x: f64, arithmetic: &mut Arithmetic) -> f64 {
        let position = x * usize_f64(values.len() - 1);
        let low = sample_index(position).min(values.len() - 1);
        let high = (low + 1).min(values.len() - 1);
        finish(
            values[low] + (position - usize_f64(low)) * (values[high] - values[low]),
            arithmetic,
        )
    }
}

/// A curve's value clipped to `0..=1`; NaN and negative infinity give 0 and
/// are recorded in `arithmetic`.
#[inline]
fn finish(value: f64, arithmetic: &mut Arithmetic) -> f64 {
    // Positive overflow has a defined clipped value (10.16). NaN and
    // negative infinity indicate genuinely undefined arithmetic.
    if value.is_nan() || value == f64::NEG_INFINITY {
        arithmetic.undefined = true;
        return 0.0;
    }
    value.clamp(0.0, 1.0)
}

impl Parametric {
    fn evaluate(self, x: f64) -> f64 {
        match self {
            Self::Power { g } => x.powf(g),
            Self::Cie122 { g, a, b } => {
                if a * x >= -b {
                    (a * x + b).max(0.0).powf(g)
                } else {
                    0.0
                }
            }
            Self::Iec61966 { g, a, b, c } => {
                if a * x >= -b {
                    (a * x + b).max(0.0).powf(g) + c
                } else {
                    c
                }
            }
            Self::Srgb { g, a, b, c, d } => {
                if x >= d {
                    (a * x + b).powf(g)
                } else {
                    c * x
                }
            }
            // Table 65's type 4 offset and lower inequalities are inconsistent
            // with its parameter list. Use +e above d and c*x+f below d, and
            // complementary lower branches (the conventional ICC model).
            Self::Full {
                g,
                a,
                b,
                c,
                d,
                e,
                f,
            } => {
                if x >= d {
                    (a * x + b).powf(g) + e
                } else {
                    c * x + f
                }
            }
        }
    }
}
