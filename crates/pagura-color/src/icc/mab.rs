//! Forward multi-stage transforms (ICC.1:2010 10.10, Tables 42–43).

use super::{Budget, clut::Clut, curves::Curve, fixed, format::zeros, slice, u32_at};
use pagura_core::{Error, Result};

#[derive(Debug, Clone)]
pub(super) struct Mab {
    a: Vec<Curve>,
    clut: Option<Clut>,
    m: Vec<Curve>,
    matrix: Option<[[f64; 4]; 3]>,
    b: Vec<Curve>,
}
impl Mab {
    pub(super) fn parse(data: &[u8], inputs: usize, budget: &mut Budget) -> Result<Self> {
        zeros(slice(data, 4, 4)?)?;
        let channels = slice(data, 8, 4)?;
        if usize::from(channels[0]) != inputs || channels[1] != 3 || channels[2..] != [0, 0] {
            return Err(Error::syntax("invalid ICC mAB channels or padding"));
        }
        let mut offsets = [0; 5];
        for (i, offset) in offsets.iter_mut().enumerate() {
            *offset = usize::try_from(u32_at(data, 12 + i * 4)?)
                .map_err(|_| Error::limit("ICC mAB offset overflow"))?;
            if *offset != 0 && (*offset < 32 || !offset.is_multiple_of(4) || *offset >= data.len())
            {
                return Err(Error::syntax("invalid ICC mAB element offset"));
            }
        }
        let [b, matrix, m, clut, a] = offsets;
        if b == 0
            || (matrix == 0) != (m == 0)
            || (clut == 0) != (a == 0)
            || (inputs != 3 && clut == 0)
        {
            return Err(Error::syntax("invalid ICC mAB element combination"));
        }
        let element = |offset| -> Result<&[u8]> {
            let end = offsets
                .iter()
                .copied()
                .filter(|&v| v > offset)
                .min()
                .unwrap_or(data.len());
            slice(data, offset, end - offset)
        };
        let b = curves(element(b)?, 3, budget)?;
        let a = if a == 0 {
            Vec::new()
        } else {
            curves(element(a)?, inputs, budget)?
        };
        let m = if m == 0 {
            Vec::new()
        } else {
            curves(element(m)?, 3, budget)?
        };
        let clut = if clut == 0 {
            None
        } else {
            Some(parse_clut(element(clut)?, inputs, budget)?)
        };
        let matrix = if matrix == 0 {
            None
        } else {
            let bytes = element(matrix)?;
            if bytes.len() != 48 {
                return Err(Error::syntax("invalid ICC mAB matrix extent"));
            }
            let mut values = [[0.0; 4]; 3];
            for (row, values) in values.iter_mut().enumerate() {
                for (col, v) in values.iter_mut().take(3).enumerate() {
                    *v = fixed(bytes, (row * 3 + col) * 4)?;
                }
                values[3] = fixed(bytes, 36 + row * 4)?;
            }
            Some(values)
        };
        Ok(Self {
            a,
            clut,
            m,
            matrix,
            b,
        })
    }
    pub(super) fn evaluate(&self, mut values: [f64; 4]) -> Result<[f64; 3]> {
        apply(&self.a, &mut values)?;
        if let Some(clut) = &self.clut {
            values = clut.evaluate(values);
        }
        apply(&self.m, &mut values)?;
        if let Some(matrix) = self.matrix {
            let next = matrix.map(|row| {
                (row[0] * values[0] + row[1] * values[1] + row[2] * values[2] + row[3])
                    .clamp(0.0, 1.0)
            });
            values[..3].copy_from_slice(&next);
        }
        apply(&self.b, &mut values)?;
        Ok([values[0], values[1], values[2]])
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.a.iter().chain(&self.m).chain(&self.b).fold(
            self.clut.as_ref().map_or(0, Clut::retained_bytes),
            |n, curve| {
                n.saturating_add(size_of::<Curve>())
                    .saturating_add(curve.retained_bytes())
            },
        )
    }
}
fn apply(curves: &[Curve], values: &mut [f64; 4]) -> Result<()> {
    for (value, curve) in values.iter_mut().zip(curves) {
        *value = curve.evaluate(value.clamp(0.0, 1.0))?;
    }
    Ok(())
}
fn curves(data: &[u8], count: usize, budget: &mut Budget) -> Result<Vec<Curve>> {
    let mut curves = Vec::with_capacity(count);
    let mut at = 0;
    for i in 0..count {
        let rest = data
            .get(at..)
            .ok_or(Error::syntax("truncated ICC mAB curve"))?;
        let length = Curve::extent(rest)?;
        curves.push(Curve::parse(slice(rest, 0, length)?, budget)?);
        at = at
            .checked_add(length)
            .ok_or(Error::limit("ICC mAB curve offset overflow"))?;
        // The final tag's external alignment bytes are excluded from its
        // declared size (7.3.5). Internal element boundaries remain aligned.
        if i + 1 == count && at == data.len() {
            break;
        }
        let padding = (4 - at % 4) % 4;
        zeros(slice(data, at, padding)?)?;
        at += padding;
    }
    if at != data.len() {
        return Err(Error::syntax("invalid ICC mAB curve block extent"));
    }
    Ok(curves)
}
fn parse_clut(data: &[u8], inputs: usize, budget: &mut Budget) -> Result<Clut> {
    let grids = slice(data, 0, 16)?;
    zeros(&grids[inputs..])?;
    zeros(slice(data, 17, 3)?)?;
    let precision = usize::from(slice(data, 16, 1)?[0]);
    let rest = data
        .get(20..)
        .ok_or(Error::syntax("truncated ICC mAB CLUT"))?;
    let clut = Clut::parse(rest, &grids[..inputs], precision, budget)?;
    let end = 20 + clut.encoded_len(precision);
    if end == data.len() {
        return Ok(clut);
    }
    let padding = (4 - end % 4) % 4;
    zeros(slice(data, end, padding)?)?;
    if end + padding != data.len() {
        return Err(Error::syntax("invalid ICC mAB CLUT extent"));
    }
    Ok(clut)
}
