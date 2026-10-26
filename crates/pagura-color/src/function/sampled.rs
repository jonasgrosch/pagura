//! Packed sampled functions with linear and cubic spline interpolation
//! (7.10.2).
//!
//! Order 3 uses a local interpolating Catmull–Rom spline: centred
//! differences determine interior tangents, and one-sided differences
//! determine endpoint tangents. This preserves samples and affine functions,
//! including at the boundaries; clause 7.10.2 does not prescribe the spline's
//! boundary conditions. The fewer-than-four-samples linear fallback of
//! 7.10.2 applies to each axis independently.
//!
//! One-input 8-bit linear grids, the common shading case, take a shorter path
//! with the same sums in the same order.

use super::{SampledFunction, interpolate, interpolate_enclosed, make_finite};
use crate::affine::{Affine, Undecided};
use crate::space::MAX_COMPONENTS;
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

/// A validated sampled function body.
#[derive(Debug, Clone)]
pub(super) struct Sampled {
    domain: Box<[[f64; 2]]>,
    size: Box<[usize]>,
    bits: usize,
    /// `2^bits − 1`, the divisor of a raw sample.
    scale: f64,
    cubic: bool,
    data: Arc<[u8]>,
    encode: Box<[[f64; 2]]>,
    decode: Box<[[f64; 2]]>,
    path: Path,
    /// Most sample tuples one evaluation interpolates.
    corners: usize,
}

/// The evaluation a sampled function takes, chosen once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// One 8-bit linear input on the unit domain whose `Encode` spans the
    /// grid and whose `Decode` is the identity: both maps are exact
    /// identities, so the grid position is `x · (size − 1)`.
    UnitBytes,
    /// One 8-bit linear input.
    Bytes,
    /// The general tensor product.
    Tensor,
}

/// Nonzero support in one sample dimension.
#[derive(Clone, Copy)]
struct Weights {
    values: [(usize, f64); 4],
    len: usize,
}

impl Sampled {
    pub(super) fn retained_bytes(&self) -> usize {
        [
            size_of::<Self>(),
            size_of_val(self.domain.as_ref()),
            size_of_val(self.size.as_ref()),
            self.data.len(),
            2 * size_of::<usize>(),
            size_of_val(self.encode.as_ref()),
            size_of_val(self.decode.as_ref()),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }

    pub(super) fn new(
        parameters: SampledFunction,
        outputs: usize,
        unit_domain: bool,
        limits: &Limits,
    ) -> Result<Self> {
        let SampledFunction {
            domain,
            range,
            size,
            bits_per_sample,
            order,
            encode,
            decode,
            samples,
        } = parameters;
        if size.contains(&0) {
            return Err(Error::syntax("invalid sampled function size"));
        }
        let size: Box<[usize]> = size
            .iter()
            .map(|&n| {
                usize::try_from(n).map_err(|_| Error::limit("sampled function size exceeds usize"))
            })
            .collect::<Result<_>>()?;
        if size.len() != domain.len() {
            return Err(Error::syntax("wrong sampled function size dimension"));
        }
        if !matches!(bits_per_sample, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
            return Err(Error::syntax("invalid function BitsPerSample"));
        }
        let bits = usize::from(bits_per_sample);
        let cubic = match order {
            1 => false,
            3 => true,
            _ => return Err(Error::syntax("invalid sampled interpolation order")),
        };
        let mut count = outputs;
        for &n in &size {
            count = count
                .checked_mul(n)
                .ok_or(Error::limit("sampled grid size overflows"))?;
        }
        let required = count
            .checked_mul(bits)
            .and_then(|v| v.checked_add(7))
            .map(|v| v / 8)
            .filter(|&v| v <= limits.decoded_bytes)
            .ok_or(Error::limit("sampled function byte budget exceeded"))?;
        let data = samples
            .get(..required)
            .ok_or(Error::syntax("truncated sampled function"))?;
        let encode: Box<[[f64; 2]]> = match encode {
            Some(encode) => encode.into(),
            None => size.iter().map(|&n| [0.0, last(n)]).collect(),
        };
        let decode: Box<[[f64; 2]]> = decode.unwrap_or(range).into();
        if encode.len() != size.len() || decode.len() != outputs {
            return Err(Error::syntax("wrong sampled encode or decode dimension"));
        }
        super::finite(encode.iter().chain(decode.iter()).flatten().copied())?;
        let linear_bytes = bits == 8 && size.len() == 1 && (!cubic || size[0] < 4);
        let path = if linear_bytes
            && unit_domain
            && *encode == [[0.0, last(size[0])]]
            && decode.iter().all(|&r| r == [0.0, 1.0])
        {
            Path::UnitBytes
        } else if linear_bytes {
            Path::Bytes
        } else {
            Path::Tensor
        };
        let corners = size.iter().fold(1_usize, |corners, &n| {
            corners.saturating_mul(match n {
                1 => 1,
                2 | 3 => 2,
                _ if cubic => 4,
                _ => 2,
            })
        });
        Ok(Self {
            scale: ((1_u64 << bits) - 1) as f64,
            domain: domain.into(),
            size,
            bits,
            cubic,
            data: data.into(),
            encode,
            decode,
            path,
            corners,
        })
    }

    /// Most `function_work` units of the interpolation itself: `m + n` per
    /// sample tuple, as many tuples as the grid supports.
    pub(super) fn cost(&self) -> usize {
        self.size
            .len()
            .saturating_add(self.decode.len())
            .saturating_mul(self.corners)
    }

    /// Evaluates at the inputs `input(i)`, already clipped to the domain,
    /// into `output` (at most `n` values), before range clipping.
    pub(super) fn evaluate(&self, input: impl Fn(usize) -> f64, output: &mut [f64]) {
        let size = self.size.first().copied().unwrap_or(1);
        match self.path {
            Path::UnitBytes => self.interpolate_bytes(input(0) * last(size), output),
            Path::Bytes => {
                let x = grid_position(input(0), self.dimension(0));
                self.interpolate_bytes(x, output);
                self.apply_decode(output);
            }
            Path::Tensor => {
                self.tensor(input, output);
                self.apply_decode(output);
            }
        }
    }

    /// Encloses what [`Self::evaluate`] computes for one input over every
    /// value of `x`, already clipped to the domain (see [`crate::affine`]):
    /// linear interpolation in a grid of one input. Where the values lie in
    /// two cells near their boundary, both cells' interpolations are joined.
    pub(super) fn enclose(&self, x: Affine, output: &mut [Affine]) -> Result<(), Undecided> {
        let &[size] = &*self.size else {
            return Err(Undecided::Unsupported);
        };
        let position = match self.path {
            Path::UnitBytes => x.scale(last(size)),
            Path::Bytes | Path::Tensor if !self.cubic || size < 4 => {
                interpolate_enclosed(x, self.dimension(0).domain, self.dimension(0).encode)
                    .clamp(0.0, last(size))?
            }
            Path::Bytes | Path::Tensor => return Err(Undecided::Unsupported),
        };
        let n = self.decode.len();
        let [first, end] = position.cells(size - 1)?;
        for low in first..=end {
            let high = (low + 1).min(size - 1);
            // `x − low`, exact on the grid.
            let t = position.sub(Affine::point(low as f64));
            let one_minus = Affine::point(1.0).sub(t);
            for (component, out) in output.iter_mut().enumerate().take(n) {
                let [a, b] = [low, high].map(|i| self.sample(i * n + component));
                let value = if self.path == Path::Tensor {
                    // `0 + (1 − t)·a + t·b`, the corners summed in order; one
                    // sample, or `t` zero, gives `0 + 1·a`, which is the same.
                    Affine::point(0.0).add(one_minus.scale(a)).add(t.scale(b))
                } else {
                    // `(1 − t)·a + t·b`; where `t` is zero, `a`, which is the same.
                    one_minus.scale(a).add(t.scale(b))
                };
                *out = if low == first { value } else { out.join(value) };
            }
        }
        if self.path != Path::UnitBytes {
            for (value, &range) in output.iter_mut().zip(&self.decode) {
                *value = interpolate_enclosed(*value, [0.0, 1.0], range);
            }
        }
        Ok(())
    }

    /// The domain, `Encode` and size of input dimension `i`.
    fn dimension(&self, i: usize) -> Dimension {
        Dimension {
            domain: self.domain.get(i).copied().unwrap_or([0.0; 2]),
            encode: self.encode.get(i).copied().unwrap_or([0.0; 2]),
            size: self.size.get(i).copied().unwrap_or(1),
        }
    }

    fn apply_decode(&self, output: &mut [f64]) {
        for (value, range) in output.iter_mut().zip(&self.decode) {
            *value = interpolate(*value, [0.0, 1.0], *range);
        }
    }

    /// Linearly interpolates the two 8-bit tuples around grid position `x`,
    /// before Decode: `(1 − t) · low + t · high`, or the low tuple alone
    /// when `t` is zero, as the tensor path sums them.
    fn interpolate_bytes(&self, x: f64, output: &mut [f64]) {
        let size = self.size.first().copied().unwrap_or(1);
        let low = grid_index(x);
        let t = x - low as f64;
        let n = self.decode.len();
        let tuple = |index: usize| {
            let start = index.wrapping_mul(n);
            self.data.get(start..start.wrapping_add(n)).unwrap_or(&[])
        };
        let a = tuple(low);
        let unit = |byte: u8| UNIT[usize::from(byte)];
        if t == 0.0 {
            for (out, &a) in output.iter_mut().zip(a) {
                *out = unit(a);
            }
        } else {
            let b = tuple((low + 1).min(size - 1));
            for ((out, &a), &b) in output.iter_mut().zip(a).zip(b) {
                *out = (1.0 - t) * unit(a) + t * unit(b);
            }
        }
    }

    /// The tensor-product interpolation over every input, corner by corner
    /// with the first input varying fastest.
    fn tensor(&self, input: impl Fn(usize) -> f64, output: &mut [f64]) {
        let mut positions = [Weights::EMPTY; MAX_COMPONENTS];
        let mut vertices = 1_usize;
        for (i, slot) in positions.iter_mut().enumerate().take(self.size.len()) {
            let dimension = self.dimension(i);
            let x = grid_position(input(i), dimension);
            *slot = Weights::new(x, dimension.size, self.cubic);
            vertices = vertices.saturating_mul(slot.len);
        }
        let positions = positions.get(..self.size.len()).unwrap_or(&[]);
        let n = self.decode.len();
        output.fill(0.0);
        for corner in 0..vertices {
            let (mut offset, mut stride, mut weight) = (0_usize, n, 1.0);
            let mut remaining = corner;
            for (position, &size) in positions.iter().zip(&self.size) {
                let len = position.len.max(1);
                let (index, factor) = position
                    .values
                    .get(remaining % len)
                    .copied()
                    .unwrap_or((0, 0.0));
                remaining /= len;
                offset = offset.wrapping_add(index.wrapping_mul(stride));
                stride = stride.wrapping_mul(size);
                weight *= factor;
            }
            if weight == 0.0 {
                continue;
            }
            for (component, value) in output.iter_mut().enumerate() {
                *value += weight * self.sample(offset.wrapping_add(component));
            }
        }
    }

    /// Sample `index` of the packed table, scaled to `0..=1`. Widths below 8
    /// divide 8, so a sample never straddles a byte; 12-bit samples start on
    /// a nibble and span two bytes; wider ones are whole bytes.
    fn sample(&self, index: usize) -> f64 {
        let width = self.bits;
        let start = index.wrapping_mul(width);
        let byte = |i: usize| u64::from(self.data.get(i).copied().unwrap_or(0));
        let value = match width {
            1 | 2 | 4 => (byte(start / 8) >> (8 - width - start % 8)) & ((1 << width) - 1),
            12 => (((byte(start / 8) << 8) | byte(start / 8 + 1)) >> (4 - start % 8)) & 0xfff,
            _ => (start / 8..start / 8 + width / 8).fold(0_u64, |value, i| (value << 8) | byte(i)),
        };
        value as f64 / self.scale
    }
}

impl Weights {
    const EMPTY: Self = Self {
        values: [(0, 0.0); 4],
        len: 0,
    };

    fn new(x: f64, size: usize, cubic: bool) -> Self {
        let low = grid_index(x);
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
        // Extrapolate a missing neighbour linearly, so that its contribution
        // folds into the two real endpoints without ghost samples.
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

/// `byte / 255` for every byte, as the division gives it.
const UNIT: [f64; 256] = {
    let mut unit = [0.0; 256];
    let mut byte = 0;
    while byte < 256 {
        unit[byte] = byte as f64 / 255.0;
        byte += 1;
    }
    unit
};

/// One input dimension of the grid.
#[derive(Clone, Copy)]
struct Dimension {
    domain: [f64; 2],
    encode: [f64; 2],
    size: usize,
}

/// The encoded grid position of an input already clipped to its domain,
/// clipped to the grid; an undefined encoding reads as 0.
fn grid_position(x: f64, dimension: Dimension) -> f64 {
    make_finite(interpolate(x, dimension.domain, dimension.encode)).clamp(0.0, last(dimension.size))
}

/// The last grid index of a dimension of `size` samples, as a real.
#[expect(
    clippy::cast_precision_loss,
    reason = "sizes come from u32 values, which f64 holds exactly"
)]
fn last(size: usize) -> f64 {
    (size - 1) as f64
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "grid positions are finite and clipped to the grid extent"
)]
fn grid_index(x: f64) -> usize {
    x.floor() as usize
}
