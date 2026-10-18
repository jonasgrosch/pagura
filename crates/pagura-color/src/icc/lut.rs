//! Forward lut8Type/lut16Type pipelines (ICC.1:2010 10.8–10.9).

use super::{
    Budget,
    clut::{Clut, interpolate, samples},
    fixed,
    format::zeros,
    slice, u16_at,
};
use pagura_core::{Error, Result};

#[derive(Debug, Clone)]
pub(super) struct Lut {
    input: Vec<Vec<f64>>,
    clut: Clut,
    output: Vec<Vec<f64>>,
}
impl Lut {
    pub(super) fn parse(data: &[u8], inputs: usize, budget: &mut Budget) -> Result<Self> {
        zeros(slice(data, 4, 4)?)?;
        let channels = slice(data, 8, 4)?;
        if usize::from(channels[0]) != inputs || channels[1] != 3 || channels[3] != 0 {
            return Err(Error::syntax("invalid ICC LUT channel counts or padding"));
        }
        // PDF Table 67 excludes XYZ source profiles, so the legacy matrix must
        // be identity (10.8/10.9); Lab source values are normalized separately.
        for i in 0..9 {
            if fixed(data, 12 + 4 * i)? != if i % 4 == 0 { 1.0 } else { 0.0 } {
                return Err(Error::syntax("ICC non-XYZ LUT matrix is not identity"));
            }
        }
        let sixteen = slice(data, 0, 4)? == b"mft2";
        let (precision, n, m, mut at) = if sixteen {
            (
                2,
                usize::from(u16_at(data, 48)?),
                usize::from(u16_at(data, 50)?),
                52,
            )
        } else {
            (1, 256, 256, 48)
        };
        if !(2..=4096).contains(&n) || !(2..=4096).contains(&m) {
            return Err(Error::syntax("invalid ICC LUT curve sample counts"));
        }
        let input = tables(data, &mut at, inputs, n, precision, budget)?;
        let grids = vec![channels[2]; inputs];
        let rest = data
            .get(at..)
            .ok_or(Error::syntax("truncated ICC LUT CLUT"))?;
        let clut = Clut::parse(rest, &grids, precision, budget)?;
        at += clut.encoded_len(precision);
        let output = tables(data, &mut at, 3, m, precision, budget)?;
        if at != data.len() {
            return Err(Error::syntax("invalid ICC LUT extent"));
        }
        Ok(Self {
            input,
            clut,
            output,
        })
    }
    pub(super) fn evaluate(&self, mut values: [f64; 4]) -> [f64; 3] {
        for (value, curve) in values.iter_mut().zip(&self.input) {
            *value = interpolate(curve, *value);
        }
        let values = self.clut.evaluate(values);
        [0, 1, 2].map(|i| interpolate(&self.output[i], values[i]))
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.clut
            .retained_bytes()
            .saturating_add(self.input.iter().chain(&self.output).fold(0_usize, |n, v| {
                n.saturating_add(size_of::<Vec<f64>>())
                    .saturating_add(v.capacity().saturating_mul(8))
            }))
    }
}
fn tables(
    data: &[u8],
    at: &mut usize,
    channels: usize,
    count: usize,
    precision: usize,
    budget: &mut Budget,
) -> Result<Vec<Vec<f64>>> {
    budget.charge(channels * count)?;
    let length = count * precision; // channels <= 4, count <= 4096, precision <= 2.
    let mut curves = Vec::with_capacity(channels);
    for _ in 0..channels {
        curves.push(samples(slice(data, *at, length)?, count, precision)?);
        *at = at
            .checked_add(length)
            .ok_or(Error::limit("ICC LUT offset overflow"))?;
    }
    Ok(curves)
}
