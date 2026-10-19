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
    pub(super) fn normalized_byte_grid(&self) -> bool {
        self.bits == 8
            && self.size.len() == 1
            && (!self.cubic || self.size[0] < 4)
            && self.encode.as_slice() == [[0.0, (self.size[0] - 1) as f64]]
            && self.decode.iter().all(|&r| r == [0.0, 1.0])
    }

    // Parsed bytes and complementary nonnegative weights keep every output
    // finite and in [0,1]. Identity Decode/Range passes cannot change a bit.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "encoded coordinates are clamped to the parsed grid extent"
    )]
    pub(super) fn evaluate_normalized_bytes(
        &self,
        input: f64,
        output: &mut [f64],
        work: &mut usize,
        limits: &Limits,
    ) -> Result<()> {
        let x = input.clamp(0.0, 1.0) * (self.size[0] - 1) as f64;
        let low = x.floor() as usize;
        let t = x - low as f64;
        let high = if t == 0.0 {
            low
        } else {
            (low + 1).min(self.size[0] - 1)
        };
        let corners = if t == 0.0 { 1usize } else { 2 };
        let cost = corners
            .checked_mul(1 + output.len())
            .ok_or(Error::limit("sample evaluation work exceeded"))?;
        *work = work
            .checked_add(cost)
            .filter(|&n| n <= limits.operations)
            .ok_or(Error::limit("sample evaluation work exceeded"))?;
        let tuple = |index: usize| -> Result<&[u8]> {
            let start = index
                .checked_mul(output.len())
                .ok_or(Error::limit("sample index overflow"))?;
            self.data
                .get(start..start + output.len())
                .ok_or(Error::syntax("truncated sampled function"))
        };
        let a = tuple(low)?;
        let b = tuple(high)?;
        for ((out, &a), &b) in output.iter_mut().zip(a).zip(b) {
            let a = f64::from(a) / 255.0;
            *out = if t == 0.0 {
                a
            } else {
                (1.0 - t) * a + t * (f64::from(b) / 255.0)
            };
        }
        Ok(())
    }
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
    pub(super) fn evaluate_into(
        &self,
        input: &[f64],
        domain: &[[f64; 2]],
        result: &mut [f64],
        work: &mut usize,
        limits: &Limits,
    ) -> Result<()> {
        if self.bits == 8 && self.size.len() == 1 && (!self.cubic || self.size[0] < 4) {
            return self.linear_bytes(input[0], domain[0], result, work, limits);
        }
        // Shadings normally have one or two inputs. Keep their interpolation
        // support on the stack; larger PDF functions retain the bounded path.
        let mut stack = [Weights::EMPTY; 4];
        let mut heap = Vec::new();
        let positions = if input.len() <= stack.len() {
            &mut stack[..input.len()]
        } else {
            heap.try_reserve_exact(input.len())
                .map_err(|_| Error::limit("sample positions allocation failed"))?;
            heap.resize(input.len(), Weights::EMPTY);
            heap.as_mut_slice()
        };
        let outputs = result.len();
        let mut vertices = 1_usize;
        for (slot, (((&x, domain), encode), &size)) in positions
            .iter_mut()
            .zip(input.iter().zip(domain).zip(&self.encode).zip(&self.size))
        {
            let x = interpolate(x.clamp(domain[0], domain[1]), *domain, *encode)
                .clamp(0.0, (size - 1) as f64);
            if !x.is_finite() {
                return Err(Error::syntax("sample encode arithmetic overflow"));
            }
            let position = Weights::new(x, size, self.cubic);
            vertices = vertices
                .checked_mul(position.len)
                .filter(|&n| n <= limits.operations)
                .ok_or(Error::limit("sampled interpolation corner budget exceeded"))?;
            *slot = position;
        }
        result.fill(0.0);
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
        Ok(())
    }
    // Linear byte grids need only two contiguous sample tuples. Preserve the
    // general tensor evaluator's low-weight product, high-weight product, add
    // and Decode order, without constructing weights or walking bit offsets.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "encoded coordinates are clamped to the parsed grid extent"
    )]
    fn linear_bytes(
        &self,
        input: f64,
        domain: [f64; 2],
        output: &mut [f64],
        work: &mut usize,
        limits: &Limits,
    ) -> Result<()> {
        let size = self.size[0];
        let x = interpolate(input.clamp(domain[0], domain[1]), domain, self.encode[0])
            .clamp(0.0, (size - 1) as f64);
        if !x.is_finite() {
            return Err(Error::syntax("sample encode arithmetic overflow"));
        }
        let low = x.floor() as usize;
        let t = x - low as f64;
        let high = if t == 0.0 {
            low
        } else {
            (low + 1).min(size - 1)
        };
        let corners = if t == 0.0 { 1_usize } else { 2 };
        let cost = corners
            .checked_mul(1 + output.len())
            .ok_or(Error::limit("sample evaluation work exceeded"))?;
        *work = work
            .checked_add(cost)
            .filter(|&v| v <= limits.operations)
            .ok_or(Error::limit("sample evaluation work exceeded"))?;
        let tuple = |index: usize| -> Result<&[u8]> {
            let start = index
                .checked_mul(output.len())
                .ok_or(Error::limit("sample index overflow"))?;
            self.data
                .get(start..start + output.len())
                .ok_or(Error::syntax("truncated sampled function"))
        };
        let a = tuple(low)?;
        let b = tuple(high)?;
        for (((out, &a), &b), &range) in output.iter_mut().zip(a).zip(b).zip(&self.decode) {
            let a = f64::from(a) / 255.0;
            let value = if t == 0.0 {
                a
            } else {
                (1.0 - t) * a + t * (f64::from(b) / 255.0)
            };
            *out = interpolate(value, [0.0, 1.0], range);
        }
        Ok(())
    }
    fn sample(&self, index: usize) -> Result<f64> {
        let width =
            usize::try_from(self.bits).map_err(|_| Error::limit("sample width exceeds usize"))?;
        let start = index
            .checked_mul(width)
            .ok_or(Error::limit("sample offset overflows"))?;
        let value = if self.bits < 8 {
            let byte = *self
                .data
                .get(start / 8)
                .ok_or(Error::syntax("truncated sampled function"))?;
            u64::from((byte >> (8 - width - start % 8)) & ((1 << width) - 1))
        } else if self.bits == 12 {
            let bytes = self
                .data
                .get(start / 8..start / 8 + 2)
                .ok_or(Error::syntax("truncated sampled function"))?;
            (u64::from(u16::from_be_bytes([bytes[0], bytes[1]])) >> (4 - start % 8)) & 4095
        } else {
            let bytes = self
                .data
                .get(start / 8..start / 8 + width / 8)
                .ok_or(Error::syntax("truncated sampled function"))?;
            bytes
                .iter()
                .fold(0_u64, |value, &byte| (value << 8) | u64::from(byte))
        };
        Ok(value as f64 / ((1_u64 << self.bits) - 1) as f64)
    }
}

/// Nonzero support in one sample dimension; tensor products are budgeted above.
#[derive(Clone, Copy)]
struct Weights {
    values: [(usize, f64); 4],
    len: usize,
}
impl Weights {
    const EMPTY: Self = Self {
        values: [(0, 0.0); 4],
        len: 0,
    };
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

#[cfg(test)]
mod packing_tests {
    use super::*;
    #[test]
    fn packed_samples_match_bit_reference_at_every_offset() {
        let data: Arc<[u8]> = (0..240_u16)
            .map(|n| (n.wrapping_mul(73) ^ (n >> 2)).to_le_bytes()[0])
            .collect();
        for bits in [1, 2, 4, 8, 12, 16, 24, 32] {
            let function = Sampled {
                size: vec![1],
                bits,
                cubic: false,
                data: data.clone(),
                encode: vec![[0.0, 1.0]],
                decode: vec![[0.0, 1.0]],
            };
            for index in 0..data.len() * 8 / bits as usize {
                let start = index * bits as usize;
                let mut value = 0_u64;
                for bit in start..start + bits as usize {
                    value = (value << 1) | u64::from((data[bit / 8] >> (7 - bit % 8)) & 1);
                }
                assert_eq!(
                    function.sample(index).unwrap(),
                    value as f64 / ((1_u64 << bits) - 1) as f64
                );
            }
        }
    }
}

#[cfg(test)]
mod linear_tests {
    use super::*;
    use crate::functions::{Body, Function};
    #[test]
    fn normalized_byte_functions_equal_independent_sixteen_bit_grids() {
        let limits = Limits::default();
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let data = [a, b, 255 - a, b, a, 255 - b];
                let sampled = Sampled {
                    size: vec![2],
                    bits: 8,
                    cubic: false,
                    data: Arc::from(data),
                    encode: vec![[0.0, 1.0]],
                    decode: vec![[0.0, 1.0]; 3],
                };
                assert!(sampled.normalized_byte_grid());
                let fast = Function {
                    domain: vec![[0.0, 1.0]],
                    range: Some(vec![[0.0, 1.0]; 3]),
                    outputs: 3,
                    body: Body::Sampled(sampled.clone()),
                    limits,
                    object: None,
                    normalized_bytes: true,
                };
                let mut general = fast.clone();
                general.normalized_bytes = false;
                let mut wide = sampled;
                wide.bits = 16;
                wide.data = data.into_iter().flat_map(|v| [v, v]).collect();
                general.body = Body::Sampled(wide);
                for x in [-1.0, -0.0, 0.0, 0.125, 0.314_159, 0.5, 0.999_9, 1.0, 2.0] {
                    let (mut av, mut bv) = ([0.0; 3], [0.0; 3]);
                    let (mut aw, mut bw) = (0, 0);
                    fast.evaluate_into(&[x], &mut av, &mut aw).unwrap();
                    general.evaluate_into(&[x], &mut bv, &mut bw).unwrap();
                    assert_eq!(av.map(f64::to_bits), bv.map(f64::to_bits));
                    assert_eq!(aw, bw);
                }
            }
        }
    }

    #[test]
    fn normalized_byte_functions_keep_errors_and_work_at_budget_boundaries() {
        for operations in 0..=12 {
            let limits = Limits {
                operations,
                ..Limits::default()
            };
            let samples = Sampled {
                size: vec![2],
                bits: 8,
                cubic: false,
                data: Arc::from([0, 255, 255, 255, 0, 0]),
                encode: vec![[0.0, 1.0]],
                decode: vec![[0.0, 1.0]; 3],
            };
            let fast = Function {
                domain: vec![[0.0, 1.0]],
                range: Some(vec![[0.0, 1.0]; 3]),
                outputs: 3,
                body: Body::Sampled(samples),
                limits,
                object: Some(pagura_core::ObjectId::new(7, 0)),
                normalized_bytes: true,
            };
            let mut general = fast.clone();
            general.normalized_bytes = false;
            for x in [0.0, 0.25, 1.0, f64::NAN] {
                let (mut a, mut b) = ([0.0; 3], [0.0; 3]);
                let (mut aw, mut bw) = (0, 0);
                let ar = fast.evaluate_into(&[x], &mut a, &mut aw);
                let br = general.evaluate_into(&[x], &mut b, &mut bw);
                assert_eq!(ar.map_err(|e| e.to_string()), br.map_err(|e| e.to_string()));
                assert_eq!(a.map(f64::to_bits), b.map(f64::to_bits));
                assert_eq!(aw, bw);
                if operations >= 9 && x == 0.25 {
                    assert_eq!(a, [0.25, 0.75, 0.75]);
                }
            }
        }
    }
    #[test]
    fn byte_linear_grids_equal_the_same_rationals_encoded_at_sixteen_bits() {
        let limits = Limits::default();
        for a in 0..=255_u8 {
            for b in 0..=255_u8 {
                let bytes = [a, b, 255 - b, 255 - a];
                let mut byte = Sampled {
                    size: vec![2],
                    bits: 8,
                    cubic: false,
                    data: Arc::from(bytes),
                    encode: vec![[0.0, 1.0]],
                    decode: vec![[-0.25, 1.25], [1.0, 0.0]],
                };
                let mut wide = byte.clone();
                wide.bits = 16;
                wide.data = bytes.into_iter().flat_map(|v| [v, v]).collect();
                if a % 2 == 0 {
                    byte.encode[0] = [1.0, 0.0];
                    wide.encode[0] = [1.0, 0.0];
                }
                for x in [-1.0, 0.0, 0.125, 0.314_159, 0.5, 0.999_9, 1.0, 2.0] {
                    let (mut fast, mut general) = ([0.0; 2], [0.0; 2]);
                    let (mut fast_work, mut general_work) = (0, 0);
                    byte.evaluate_into(&[x], &[[0.0, 1.0]], &mut fast, &mut fast_work, &limits)
                        .unwrap();
                    wide.evaluate_into(
                        &[x],
                        &[[0.0, 1.0]],
                        &mut general,
                        &mut general_work,
                        &limits,
                    )
                    .unwrap();
                    assert_eq!(fast.map(f64::to_bits), general.map(f64::to_bits));
                    assert_eq!(fast_work, general_work);
                }
            }
        }
    }
}
