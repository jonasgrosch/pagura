//! Packed sampled functions with linear and cubic spline interpolation (7.10.2).
//!
//! Order 3 uses a local interpolating Catmull–Rom spline: centred differences
//! determine interior tangents, and one-sided differences determine endpoint
//! tangents. This preserves samples and affine functions, including at the
//! boundaries. Clause 7.10.2 does not prescribe the spline's boundary conditions.
//! The project's reading of 7.10.2 applies the fewer-than-four-samples linear
//! fallback independently to each axis.

use super::{interpolate, numbers, pairs_unordered};
use pagura_core::{Dictionary, Error, Limits, Result, Value};
use pagura_document::Document;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) struct Sampled {
    size: Vec<usize>,
    bits: u32,
    cubic: bool,
    data: Arc<[u8]>,
    encode: Vec<[f64; 2]>,
    decode: Vec<[f64; 2]>,
}
impl Sampled {
    pub(super) fn parse(
        document: &Document,
        value: &Value,
        d: &Dictionary,
        inputs: usize,
        outputs: usize,
        range: &[[f64; 2]],
    ) -> Result<Self> {
        let id = value
            .as_reference()
            .ok_or(Error::syntax("sampled function must be a stream reference"))?;
        let object = document.object(id)?;
        if !object.is_stream() {
            return Err(Error::syntax("sampled function must be a stream"));
        }
        let size = numbers(document, d, b"Size", None)?
            .into_iter()
            .map(|v| {
                if v < 1.0 || v.fract() != 0.0 || v > f64::from(u32::MAX) {
                    return Err(Error::syntax("invalid sampled function size"));
                }
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "positive integral size bounded to u32 above"
                )]
                Ok(v as usize)
            })
            .collect::<Result<Vec<_>>>()?;
        if size.len() != inputs {
            return Err(Error::syntax("wrong sampled function size dimension"));
        }
        let bits = d
            .get(b"BitsPerSample")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_integer)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| matches!(v, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32))
            .ok_or(Error::syntax("invalid function BitsPerSample"))?;
        let cubic = match d
            .get(b"Order")
            .map(|v| document.resolve(v))
            .transpose()?
            .map(Value::as_integer)
        {
            None | Some(Some(1)) => false,
            Some(Some(3)) => true,
            _ => return Err(Error::syntax("invalid sampled interpolation order")),
        };
        let mut count = outputs;
        for &size in &size {
            count = count
                .checked_mul(size)
                .ok_or(Error::limit("sampled grid size overflows"))?;
        }
        let required = count
            .checked_mul(
                usize::try_from(bits).map_err(|_| Error::limit("sample width exceeds usize"))?,
            )
            .and_then(|v| v.checked_add(7))
            .map(|v| v / 8)
            .filter(|&v| v <= document.limits().decoded_bytes)
            .ok_or(Error::limit("sampled function byte budget exceeded"))?;
        let data = document.stream_data(object)?;
        if data.len() < required {
            return Err(Error::syntax("truncated sampled function"));
        }
        let defaults: Vec<_> = size.iter().map(|&n| [0.0, (n - 1) as f64]).collect();
        let encode = pairs_unordered(document, d, b"Encode", Some(&defaults))?;
        let decode = pairs_unordered(document, d, b"Decode", Some(range))?;
        if encode.len() != inputs || decode.len() != outputs {
            return Err(Error::syntax("wrong sampled encode or decode dimension"));
        }
        Ok(Self {
            size,
            bits,
            cubic,
            data: data
                .get(..required)
                .ok_or(Error::syntax("truncated sampled function"))?
                .into(),
            encode,
            decode,
        })
    }
    pub(super) fn evaluate(
        &self,
        input: &[f64],
        domain: &[[f64; 2]],
        outputs: usize,
        work: &mut usize,
        limits: &Limits,
    ) -> Result<Vec<f64>> {
        let mut positions = Vec::with_capacity(input.len());
        let mut vertices = 1_usize;
        for (((&x, domain), encode), &size) in
            input.iter().zip(domain).zip(&self.encode).zip(&self.size)
        {
            let x = interpolate(x, *domain, *encode).clamp(0.0, (size - 1) as f64);
            if !x.is_finite() {
                return Err(Error::syntax("sample encode arithmetic overflow"));
            }
            let position = Weights::new(x, size, self.cubic);
            vertices = vertices
                .checked_mul(position.len)
                .filter(|&n| n <= limits.operations)
                .ok_or(Error::limit("sampled interpolation corner budget exceeded"))?;
            positions.push(position);
        }
        let mut result = vec![0.0; outputs];
        for corner in 0..vertices {
            *work = work
                .checked_add(input.len() + outputs)
                .filter(|&v| v <= limits.operations)
                .ok_or(Error::limit("sample evaluation work exceeded"))?;
            let (mut offset, mut stride, mut weight) = (0_usize, outputs, 1.0);
            let mut remaining = corner;
            for (position, &size) in positions.iter().zip(&self.size) {
                let (index, factor) = position.values[remaining % position.len];
                remaining /= position.len;
                offset = index
                    .checked_mul(stride)
                    .and_then(|n| offset.checked_add(n))
                    .ok_or(Error::limit("sample index overflow"))?;
                stride = stride
                    .checked_mul(size)
                    .ok_or(Error::limit("sample stride overflow"))?;
                weight *= factor;
            }
            if weight == 0.0 {
                continue;
            }
            for (component, value) in result.iter_mut().enumerate() {
                let sample = self.sample(
                    offset
                        .checked_add(component)
                        .ok_or(Error::limit("sample index overflow"))?,
                )?;
                *value += weight * sample;
            }
        }
        for (value, range) in result.iter_mut().zip(&self.decode) {
            *value = interpolate(*value, [0.0, 1.0], *range);
        }
        Ok(result)
    }
    fn sample(&self, index: usize) -> Result<f64> {
        let width =
            usize::try_from(self.bits).map_err(|_| Error::limit("sample width exceeds usize"))?;
        let start = index
            .checked_mul(width)
            .ok_or(Error::limit("sample offset overflows"))?;
        let mut value = 0_u64;
        for bit in start
            ..start
                .checked_add(width)
                .ok_or(Error::limit("sample extent overflows"))?
        {
            let byte = self
                .data
                .get(bit / 8)
                .copied()
                .ok_or(Error::syntax("truncated sampled function"))?;
            value = (value << 1) | u64::from((byte >> (7 - bit % 8)) & 1);
        }
        Ok(value as f64 / ((1_u64 << self.bits) - 1) as f64)
    }
}

/// Nonzero support in one sample dimension; tensor products are budgeted above.
struct Weights {
    values: [(usize, f64); 4],
    len: usize,
}
impl Weights {
    fn new(x: f64, size: usize, cubic: bool) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "sample coordinate clamped to the allocated grid extent"
        )]
        let low = x.floor() as usize;
        let t = x - low as f64;
        let high = (low + 1).min(size - 1);
        if size == 1 || t == 0.0 {
            return Self {
                values: [(low, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
                len: 1,
            };
        }
        if !cubic || size < 4 {
            return Self {
                values: [(low, 1.0 - t), (high, t), (0, 0.0), (0, 0.0)],
                len: 2,
            };
        }
        let t2 = t * t;
        let t3 = t2 * t;
        let mut values = [
            (low.saturating_sub(1), -0.5 * t + t2 - 0.5 * t3),
            (low, 1.0 - 2.5 * t2 + 1.5 * t3),
            (high, 0.5 * t + 2.0 * t2 - 1.5 * t3),
            ((high + 1).min(size - 1), -0.5 * t2 + 0.5 * t3),
        ];
        // Extrapolate a missing neighbour linearly, so its contribution can be
        // folded into the two real endpoints without allocating ghost samples.
        if low == 0 {
            values[1].1 += 2.0 * values[0].1;
            values[2].1 -= values[0].1;
            values[0].1 = 0.0;
        }
        if high == size - 1 {
            values[1].1 -= values[3].1;
            values[2].1 += 2.0 * values[3].1;
            values[3].1 = 0.0;
        }
        Self { values, len: 4 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{ErrorKind, ObjectId};
    use pagura_document::OpenOptions;
    use pagura_testkit::fixture;

    #[test]
    fn singleton_dimensions_need_one_sample_and_keep_the_work_budget() {
        let bytes = fixture("recovery", "cubic-constant-many-inputs.pdf");
        let document = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
        let function =
            super::super::Function::parse(&document, &Value::Reference(ObjectId::new(5, 0)), 32, 1)
                .unwrap();
        assert_eq!(function.evaluate(&[0.0; 32]).unwrap(), [128.0 / 255.0]);
        let limited = Document::open(
            bytes,
            &OpenOptions {
                limits: Limits {
                    operations: 32,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let function =
            super::super::Function::parse(&limited, &Value::Reference(ObjectId::new(5, 0)), 32, 1)
                .unwrap();
        assert_eq!(
            function.evaluate(&[0.0; 32]).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}
