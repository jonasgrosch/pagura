//! Shared, half-up grid crossings advanced by an exact quotient and remainder.

use super::geometry::GRID;
use pagura_core::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub(super) struct Dda {
    boundary: i64,
    value: i64,
    remainder: i64,
    divisor: i64,
    step: i64,
    tail: i64,
}

impl Dda {
    pub(super) fn new(
        mut a: [i64; 2],
        mut b: [i64; 2],
        first: i64,
        axis: usize,
    ) -> Result<Option<Self>> {
        if axis > 1 {
            return Err(Error::invalid_argument("invalid crossing axis"));
        }
        if a[axis] > b[axis] {
            std::mem::swap(&mut a, &mut b);
        }
        if first <= a[axis] || first >= b[axis] {
            return Ok(None);
        }
        let other = 1 - axis;
        let extent = i128::from(b[axis]) - i128::from(a[axis]);
        let divisor = 2 * extent;
        let delta = i128::from(b[other]) - i128::from(a[other]);
        let numerator = 2 * (i128::from(first) - i128::from(a[axis])) * delta + extent;
        let denominator =
            i64::try_from(divisor).map_err(|_| Error::limit("grid divisor exceeds i64"))?;
        let initial = quotient(numerator, denominator);
        let remainder = numerator - initial * divisor;
        let (step, tail) = if first + GRID < b[axis] {
            // Two internal grid crossings imply extent > GRID. Thus the
            // per-step quotient fits i64 even for endpoints at +/-10^12 px.
            let increment = 2 * i128::from(GRID) * delta;
            let step = quotient(increment, denominator);
            (step, increment - step * divisor)
        } else {
            (0, 0)
        };
        let narrow =
            |n| i64::try_from(n).map_err(|_| Error::limit("grid crossing state exceeds i64"));
        Ok(Some(Self {
            boundary: first,
            value: narrow(i128::from(a[other]) + initial)?,
            remainder: narrow(remainder)?,
            divisor: narrow(divisor)?,
            step: narrow(step)?,
            tail: narrow(tail)?,
        }))
    }

    pub(super) fn at(&mut self, boundary: i64) -> Result<i64> {
        if boundary == self.boundary {
            return Ok(self.value);
        }
        if boundary != self.boundary + GRID {
            return Err(Error::invalid_argument("nonconsecutive grid crossing"));
        }
        self.value += self.step;
        // Both terms are below divisor (< 2^62), so their sum fits i64.
        self.remainder += self.tail;
        if self.remainder >= self.divisor {
            self.remainder -= self.divisor;
            self.value += 1;
        }
        self.boundary = boundary;
        Ok(self.value)
    }
}

// Most device edges have a small numerator. Avoid the wide division helper
// when a native signed division gives exactly the same Euclidean quotient.
pub(super) fn quotient(numerator: i128, denominator: i64) -> i128 {
    if let Ok(numerator) = i64::try_from(numerator) {
        i128::from(numerator.div_euclid(denominator))
    } else {
        numerator.div_euclid(i128::from(denominator))
    }
}

pub(super) fn ordinate(
    a: [i64; 2],
    b: [i64; 2],
    at: i64,
    axis: usize,
    state: &mut Option<Dda>,
) -> Result<i64> {
    if axis > 1 {
        return Err(Error::invalid_argument("invalid crossing axis"));
    }
    if at == a[axis] {
        return Ok(a[1 - axis]);
    }
    if at == b[axis] {
        return Ok(b[1 - axis]);
    }
    state
        .as_mut()
        .ok_or(Error::invalid_argument("missing grid crossing state"))?
        .at(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_crossings_equal_independent_closed_form_at_every_grid_line() {
        let mut seed = 76_723_u64;
        for axis in [0, 1] {
            for offset in [0, 1_i64 << 50, -(1_i64 << 50)] {
                for _ in 0..1000 {
                    let mut coordinate = || {
                        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                        i64::from(u32::try_from(seed >> 32).unwrap()) % (64 * GRID) - 32 * GRID
                    };
                    let mut a = [coordinate() + offset, coordinate() + offset];
                    let mut b = [coordinate() + offset, coordinate() + offset];
                    if a[axis] > b[axis] {
                        std::mem::swap(&mut a, &mut b);
                    }
                    let first = (a[axis].div_euclid(GRID) + 1) * GRID;
                    let Some(mut dda) = Dda::new(a, b, first, axis).unwrap() else {
                        continue;
                    };
                    for at in (first..b[axis]).step_by(usize::try_from(GRID).unwrap()) {
                        let extent = i128::from(b[axis]) - i128::from(a[axis]);
                        let num = 2
                            * (i128::from(at) - i128::from(a[axis]))
                            * (i128::from(b[1 - axis]) - i128::from(a[1 - axis]))
                            + extent;
                        let expected = i128::from(a[1 - axis]) + num.div_euclid(2 * extent);
                        assert_eq!(i128::from(dda.at(at).unwrap()), expected);
                    }
                }
            }
        }
    }
}
