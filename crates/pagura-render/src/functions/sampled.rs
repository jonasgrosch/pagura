//! Packed sampled functions with multilinear interpolation (7.10.2).

use super::{interpolate, numbers, pairs_unordered};
use crate::features;
use pagura_core::{Dictionary, Error, Limits, Result, Value};
use pagura_document::Document;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(super) struct Sampled {
    size: Vec<usize>,
    bits: u32,
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
        if let Some(order) = d.get(b"Order") {
            match document.resolve(order)?.as_integer() {
                Some(1) => {}
                Some(3) if size.iter().all(|&n| n < 4) => {}
                Some(3) => {
                    return Err(Error::unsupported(
                        features::FUNCTION_CUBIC,
                        "cubic sampled interpolation is not implemented",
                    ));
                }
                _ => return Err(Error::syntax("invalid sampled interpolation order")),
            }
        }
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
        let dimension = u32::try_from(input.len())
            .map_err(|_| Error::limit("sampled dimension exceeds u32"))?;
        let vertices = 1_usize
            .checked_shl(dimension)
            .filter(|&v| v <= limits.operations)
            .ok_or(Error::limit("sampled interpolation corner budget exceeded"))?;
        let mut positions = Vec::new();
        for (((&x, domain), encode), &size) in
            input.iter().zip(domain).zip(&self.encode).zip(&self.size)
        {
            let x = interpolate(x, *domain, *encode).clamp(0.0, (size - 1) as f64);
            if !x.is_finite() {
                return Err(Error::syntax("sample encode arithmetic overflow"));
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "sample coordinate clamped to the allocated grid extent"
            )]
            let low = x.floor() as usize;
            positions.push((low, (low + 1).min(size - 1), x - low as f64));
        }
        let mut result = vec![0.0; outputs];
        for corner in 0..vertices {
            *work = work
                .checked_add(input.len() + outputs)
                .filter(|&v| v <= limits.operations)
                .ok_or(Error::limit("sample evaluation work exceeded"))?;
            let (mut offset, mut stride, mut weight) = (0_usize, outputs, 1.0);
            for (dimension, (&(low, high, t), &size)) in
                positions.iter().zip(&self.size).enumerate()
            {
                let upper = (corner >> dimension) & 1 != 0;
                offset = offset
                    .checked_add(
                        if upper { high } else { low }
                            .checked_mul(stride)
                            .ok_or(Error::limit("sample index overflow"))?,
                    )
                    .ok_or(Error::limit("sample index overflow"))?;
                stride = stride
                    .checked_mul(size)
                    .ok_or(Error::limit("sample stride overflow"))?;
                weight *= if upper { t } else { 1.0 - t };
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
