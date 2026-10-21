//! Forward multi-stage transforms (ICC.1:2010 10.10, Tables 42–43).

use super::{Arithmetic, At, Budget, clut::Clut, curves::Curve, format::zeros};
use pagura_core::bytes::Cursor;
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
    pub(super) fn parse(data: &Cursor<'_>, inputs: usize, budget: &mut Budget) -> Result<Self> {
        zeros(data, 4, 4)?;
        let channels = data.bytes(8, 4)?;
        if usize::from(channels[0]) != inputs || channels[1] != 3 || channels[2..] != [0, 0] {
            return Err(Error::syntax("invalid ICC mAB channels or padding"));
        }
        let length = data.data().len();
        let mut fields = data.cursor_at(12)?;
        let mut offsets = [0; 5];
        for offset in &mut offsets {
            *offset = usize::try_from(fields.u32()?)
                .map_err(|_| Error::limit("ICC mAB offset overflow"))?;
            if *offset != 0 && (*offset < 32 || !offset.is_multiple_of(4) || *offset >= length) {
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
        let element = |offset| -> Result<Cursor<'_>> {
            let end = offsets
                .iter()
                .copied()
                .filter(|&v| v > offset)
                .min()
                .unwrap_or(length);
            data.sub(offset, end - offset)
        };
        let b = curves(&element(b)?, 3, budget)?;
        let a = if a == 0 {
            Vec::new()
        } else {
            curves(&element(a)?, inputs, budget)?
        };
        let m = if m == 0 {
            Vec::new()
        } else {
            curves(&element(m)?, 3, budget)?
        };
        let clut = if clut == 0 {
            None
        } else {
            Some(parse_clut(&element(clut)?, inputs, budget)?)
        };
        let matrix = if matrix == 0 {
            None
        } else {
            let mut bytes = element(matrix)?;
            if bytes.data().len() != 48 {
                return Err(bytes.error("invalid ICC mAB matrix extent"));
            }
            // Nine coefficients row by row, then the three offsets (10.10.3).
            let mut values = [[0.0; 4]; 3];
            for row in &mut values {
                for v in row.iter_mut().take(3) {
                    *v = bytes.fixed()?;
                }
            }
            for row in &mut values {
                row[3] = bytes.fixed()?;
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
    pub(super) fn evaluate(&self, mut values: [f64; 4], arithmetic: &mut Arithmetic) -> [f64; 3] {
        apply(&self.a, &mut values, arithmetic);
        if let Some(clut) = &self.clut {
            values = clut.evaluate(values);
        }
        apply(&self.m, &mut values, arithmetic);
        if let Some(matrix) = self.matrix {
            let next = matrix.map(|row| {
                (row[0] * values[0] + row[1] * values[1] + row[2] * values[2] + row[3])
                    .clamp(0.0, 1.0)
            });
            values[..3].copy_from_slice(&next);
        }
        apply(&self.b, &mut values, arithmetic);
        [values[0], values[1], values[2]]
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
fn apply(curves: &[Curve], values: &mut [f64; 4], arithmetic: &mut Arithmetic) {
    for (value, curve) in values.iter_mut().zip(curves) {
        *value = curve.evaluate(value.clamp(0.0, 1.0), arithmetic);
    }
}
fn curves(data: &Cursor<'_>, count: usize, budget: &mut Budget) -> Result<Vec<Curve>> {
    let length = data.data().len();
    let mut curves = Vec::with_capacity(count);
    let mut at = 0;
    for i in 0..count {
        let curve = data.cursor_at(at)?;
        let extent = Curve::extent(&curve)?;
        curves.push(Curve::parse(&data.sub(at, extent)?, budget)?);
        at = at
            .checked_add(extent)
            .ok_or(Error::limit("ICC mAB curve offset overflow"))?;
        // The final tag's external alignment bytes are excluded from its
        // declared size (7.3.5). Internal element boundaries remain aligned.
        if i + 1 == count && at == length {
            break;
        }
        let padding = (4 - at % 4) % 4;
        zeros(data, at, padding)?;
        at += padding;
    }
    if at != length {
        return Err(Error::syntax("invalid ICC mAB curve block extent"));
    }
    Ok(curves)
}
fn parse_clut(data: &Cursor<'_>, inputs: usize, budget: &mut Budget) -> Result<Clut> {
    let length = data.data().len();
    let grids = data.bytes(0, 16)?;
    zeros(data, inputs, 16 - inputs)?;
    zeros(data, 17, 3)?;
    let precision = usize::from(data.u8_at(16)?);
    let clut = Clut::parse(
        &data.cursor_at(20)?,
        grids.get(..inputs).unwrap_or_default(),
        precision,
        budget,
    )?;
    let end = 20 + clut.encoded_len(precision);
    if end == length {
        return Ok(clut);
    }
    let padding = (4 - end % 4) % 4;
    zeros(data, end, padding)?;
    if end + padding != length {
        return Err(Error::syntax("invalid ICC mAB CLUT extent"));
    }
    Ok(clut)
}
