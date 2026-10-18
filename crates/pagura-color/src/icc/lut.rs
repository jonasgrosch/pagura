//! Forward lut8Type/lut16Type pipelines (ICC.1:2010 10.8–10.9).

use super::{
    At, Budget,
    clut::{Clut, interpolate, samples},
    format::zeros,
};
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Result};

#[derive(Debug, Clone)]
pub(super) struct Lut {
    input: Vec<Vec<f64>>,
    clut: Clut,
    output: Vec<Vec<f64>>,
}
impl Lut {
    pub(super) fn parse(data: &Cursor<'_>, inputs: usize, budget: &mut Budget) -> Result<Self> {
        zeros(data, 4, 4)?;
        let channels = data.bytes(8, 4)?;
        if usize::from(channels[0]) != inputs || channels[1] != 3 || channels[3] != 0 {
            return Err(Error::syntax("invalid ICC LUT channel counts or padding"));
        }
        // PDF Table 67 excludes XYZ source profiles, so the legacy matrix must
        // be identity (10.8/10.9); Lab source values are normalized separately.
        let mut matrix = data.cursor_at(12)?;
        for i in 0..9 {
            if matrix.fixed()? != if i % 4 == 0 { 1.0 } else { 0.0 } {
                return Err(Error::syntax("ICC non-XYZ LUT matrix is not identity"));
            }
        }
        let sixteen = data.bytes(0, 4)? == b"mft2";
        let (precision, n, m) = if sixteen {
            (2, usize::from(matrix.u16()?), usize::from(matrix.u16()?))
        } else {
            (1, 256, 256)
        };
        if !(2..=4096).contains(&n) || !(2..=4096).contains(&m) {
            return Err(Error::syntax("invalid ICC LUT curve sample counts"));
        }
        let mut at = matrix;
        let input = tables(&mut at, inputs, n, precision, budget)?;
        let grids = vec![channels[2]; inputs];
        let clut = Clut::parse(&at, &grids, precision, budget)?;
        at.skip(clut.encoded_len(precision))?;
        let output = tables(&mut at, 3, m, precision, budget)?;
        if !at.at_end() {
            return Err(at.error("invalid ICC LUT extent"));
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
    data: &mut Cursor<'_>,
    channels: usize,
    count: usize,
    precision: usize,
    budget: &mut Budget,
) -> Result<Vec<Vec<f64>>> {
    budget.values(channels * count)?; // channels <= 4, count <= 4096.
    let mut curves = Vec::with_capacity(channels);
    for _ in 0..channels {
        curves.push(samples(data, count, precision)?);
    }
    Ok(curves)
}
