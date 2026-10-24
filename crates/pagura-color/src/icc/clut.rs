//! Bounded multilinear CLUT interpolation (ICC.1:2010 10.8–10.10).
//!
//! Multilinear interpolation is project policy where ICC leaves interpolation
//! unspecified; a two-point axis is linear as required by those clauses.

use super::{Budget, sample_index, usize_f64};
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Result};

#[derive(Debug, Clone)]
pub(super) struct Clut {
    grids: Vec<usize>,
    values: Vec<f64>,
}
impl Clut {
    /// Parses a CLUT whose samples start `data`; `data` may extend beyond it.
    pub(super) fn parse(
        data: &Cursor<'_>,
        grids: &[u8],
        precision: usize,
        budget: &mut Budget,
    ) -> Result<Self> {
        if grids.is_empty() || grids.len() > 4 || grids.iter().any(|&n| n < 2) {
            return Err(Error::syntax("invalid ICC CLUT dimensions"));
        }
        if !matches!(precision, 1 | 2) {
            return Err(Error::syntax("invalid ICC CLUT precision"));
        }
        // At most 3 × 255^4 values, which fits u64 but not a 32-bit usize:
        // counting in u64 lets the work budget, not the address width,
        // decide, so every target reports the same error.
        let count = grids
            .iter()
            .fold(3_u64, |n, &g| n.saturating_mul(u64::from(g)));
        budget.values(usize::try_from(count).unwrap_or(usize::MAX))?;
        let count = usize::try_from(count).map_err(|_| Error::limit("ICC CLUT size overflow"))?;
        let values = samples(&mut data.clone(), count, precision)?;
        Ok(Self {
            grids: grids.iter().map(|&n| usize::from(n)).collect(),
            values,
        })
    }
    pub(super) fn evaluate(&self, input: [f64; 4]) -> [f64; 4] {
        let mut bases = [0; 4];
        let mut fractions = [0.0; 4];
        for (i, &grid) in self.grids.iter().enumerate() {
            let x = input[i].clamp(0.0, 1.0) * usize_f64(grid - 1);
            bases[i] = sample_index(x).min(grid - 2);
            fractions[i] = x - usize_f64(bases[i]);
        }
        let mut out = [0.0; 4];
        // PDF source profiles have at most four components: at most 16 corners.
        for corner in 0..(1 << self.grids.len()) {
            let mut weight = 1.0;
            let mut index = 0;
            for (i, &grid) in self.grids.iter().enumerate() {
                let high = (corner >> i) & 1;
                weight *= if high == 1 {
                    fractions[i]
                } else {
                    1.0 - fractions[i]
                };
                index = index * grid + bases[i] + high;
            }
            for (channel, value) in out.iter_mut().take(3).enumerate() {
                *value += weight * self.values[index * 3 + channel];
            }
        }
        out
    }
    pub(super) fn encoded_len(&self, precision: usize) -> usize {
        self.values.len() * precision // Admitted storage <= decoded_bytes / 8, precision <= 2.
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.values
            .capacity()
            .saturating_mul(size_of::<f64>())
            .saturating_add(self.grids.capacity().saturating_mul(size_of::<usize>()))
    }
}

/// Reads `count` normalized 8- or 16-bit table entries, advancing `data`.
pub(super) fn samples(data: &mut Cursor<'_>, count: usize, precision: usize) -> Result<Vec<f64>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("ICC table allocation failed"))?;
    for _ in 0..count {
        values.push(if precision == 1 {
            f64::from(data.u8()?) / 255.0
        } else {
            f64::from(data.u16()?) / 65535.0
        });
    }
    Ok(values)
}

pub(super) fn interpolate(values: &[f64], x: f64) -> f64 {
    let position = x.clamp(0.0, 1.0) * usize_f64(values.len() - 1);
    let low = sample_index(position).min(values.len() - 2);
    let fraction = position - usize_f64(low);
    values[low] * (1.0 - fraction) + values[low + 1] * fraction
}
