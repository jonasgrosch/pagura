//! One component's native samples, reconstructed while its entropy-coded
//! blocks are consumed.
use super::super::entropy::{Bits, Huffman};
use super::basis::Reference;
use pagura_core::{Error, Result};

/// Sample bytes for eight-by-eight blocks in block-row order, initialized to
/// a background value, plus per-block flags.
pub(in crate::dct) struct Plane {
    samples: Vec<u8>,
    /// Per block: nonzero once the block differs from the background.
    dirty: Vec<u8>,
    pitch: usize,
    scratch: [i32; 64],
    reference: Reference,
    /// Joined zero-DC-difference and end-of-block prefixes, built at the
    /// first block from that block's tables.
    shortcut: Vec<u32>,
    /// The last dequantized DC value and its reference sample.
    dc_cache: (i32, u8),
    background: u8,
    blocks: usize,
}

fn filled<T: Clone>(length: usize, value: T, message: &'static str) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| Error::limit(message))?;
    values.resize(length, value);
    Ok(values)
}

impl Plane {
    pub(in crate::dct) fn new(blocks: usize, stride: usize, background: u8) -> Result<Self> {
        let length = blocks
            .checked_mul(64)
            .ok_or(Error::limit("JPEG native storage overflow"))?;
        let mut shortcut = Vec::new();
        shortcut
            .try_reserve_exact(4096)
            .map_err(|_| Error::limit("JPEG native prefix allocation failed"))?;
        Ok(Self {
            samples: filled(length, background, "JPEG native allocation failed")?,
            dirty: filled(blocks, 0, "JPEG block flag allocation failed")?,
            pitch: stride * 8,
            scratch: [0; 64],
            reference: Reference::new(),
            shortcut,
            dc_cache: (0, 128),
            background,
            blocks,
        })
    }

    pub(in crate::dct) fn take_samples(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.samples)
    }

    pub(in crate::dct) fn take_dirty(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.dirty)
    }

    pub(in crate::dct) fn background(&self) -> u8 {
        self.background
    }

    pub(in crate::dct) fn samples(&self) -> &[u8] {
        &self.samples
    }

    pub(in crate::dct) fn pitch(&self) -> usize {
        self.pitch
    }

    pub(in crate::dct) fn bytes(&self) -> usize {
        self.samples.capacity()
            + self.dirty.capacity()
            + self.shortcut.capacity() * size_of::<u32>()
    }

    /// Decodes the block at (`row`, `column`) of this component and stores its
    /// samples.
    pub(in crate::dct) fn decode(
        &mut self,
        (row, column): (usize, usize),
        dc: &Huffman,
        ac: &Huffman,
        bits: &mut Bits<'_>,
        predictor: &mut i32,
        quant: &[u16; 64],
    ) -> Result<()> {
        let stride = self.pitch / 8;
        let index = row * stride + column;
        if column >= stride || index >= self.blocks {
            return Err(bits.error("JPEG block outside component"));
        }
        if self.shortcut.is_empty() {
            let table = bits.native_shortcut(dc, ac)?;
            self.shortcut.extend_from_slice(&table);
        }
        // A background block repeats the previous DC with an immediate end
        // of block. The predictor/quantizer product is in the baseline range.
        if self.dc_cache.1 == self.background
            && predictor.wrapping_mul(i32::from(quant[0])) == self.dc_cache.0
            && bits.native_zero(&self.shortcut)?
        {
            return Ok(());
        }
        let (nonconstant, l1) =
            bits.native_coefficients(dc, ac, predictor, &mut self.scratch, quant, &self.shortcut)?;
        let constant = if self.scratch[0] == self.dc_cache.0 {
            self.dc_cache.1
        } else {
            let value = self.reference.dc(self.scratch[0]);
            self.dc_cache = (self.scratch[0], value);
            value
        };
        if !nonconstant && constant == self.background {
            return Ok(());
        }
        let output = if nonconstant {
            self.reconstruct(bits, l1)?
        } else {
            [[constant; 8]; 8]
        };
        self.dirty[index] = 1;
        let base = (row * self.pitch + column) * 8;
        for (y, row) in output.iter().enumerate() {
            let start = base + y * self.pitch;
            self.samples
                .get_mut(start..start + 8)
                .ok_or_else(|| bits.error("JPEG block outside component"))?
                .copy_from_slice(row);
        }
        Ok(())
    }

    /// Certified f32 reconstruction of the dequantized block in `scratch`
    /// with coefficient L1 norm `l1`.
    ///
    /// The f32 transform's error is at most `l1 · 2⁻²⁰ + 2⁻¹⁴` (an outward
    /// rounded bound on the worst coefficient multiplier, 8.3e-7, plus the
    /// shift and rounding). Samples whose bounds round to different bytes are
    /// recomputed in binary64 in the reference's order.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "outward-rounded error bound and clamped byte samples; the coefficient bound covers f32 conversion"
    )]
    #[inline(never)]
    fn reconstruct(&mut self, bits: &mut Bits<'_>, l1: u64) -> Result<[[u8; 8]; 8]> {
        let input = self.scratch.map(|v| v as f32);
        let l1 = l1 as f64;
        let bound = l1 * (1.0 / 1_048_576.0) + 1.0 / 16384.0;
        let error = (bound as f32) * 1.000_000_2;
        let values = super::spatial::spatial(&input);
        let byte = |value: f32| ((value.clamp(0.0, 255.0)) as u32).to_le_bytes()[0];
        let values = values.as_flattened();
        let lower: [u8; 64] = std::array::from_fn(|i| byte(values[i] + 128.5 - error));
        let upper: [u8; 64] = std::array::from_fn(|i| byte(values[i] + 128.5 + error));
        let mut result: [[u8; 8]; 8] =
            std::array::from_fn(|y| std::array::from_fn(|x| lower[y * 8 + x]));
        let mut mask = 0u64;
        if lower != upper {
            for (i, (low, high)) in lower.iter().zip(&upper).enumerate() {
                mask |= u64::from(low != high) << i;
            }
        }
        if mask != 0 {
            let work = self.reference.refine(&self.scratch, mask, &mut result);
            bits.charge_refinement(work)?;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coefficients(state: &mut u32, case: usize) -> [i32; 64] {
        let amplitude = if case.is_multiple_of(4) {
            1_000_000
        } else {
            512
        };
        let mut values = std::array::from_fn(|i| {
            *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if i != 0 && case.is_multiple_of(3) && *state & 7 != 0 {
                0
            } else {
                i32::try_from(*state % amplitude).unwrap() - i32::try_from(amplitude / 2).unwrap()
            }
        });
        if values[1..].iter().all(|&v| v == 0) {
            values[1] = 1;
        }
        values
    }

    #[test]
    fn certified_samples_are_the_ordered_annex_a_result() {
        let reference = Reference::new();
        let mut plane = Plane::new(1, 1, 255).unwrap();
        let mut bits = Bits::new(&[], 0);
        bits.limit_stream(0, 100_000_000);
        let mut state = 0x75a3_194bu32;
        for case in 0..2048 {
            let input = coefficients(&mut state, case);
            let l1 = input.iter().map(|v| u64::from(v.unsigned_abs())).sum();
            plane.scratch = input;
            let actual = plane.reconstruct(&mut bits, l1).unwrap();
            let mut exact = [[0u8; 8]; 8];
            reference.refine(&input, u64::MAX, &mut exact);
            assert_eq!(actual, exact, "case {case}");
        }
    }
}
