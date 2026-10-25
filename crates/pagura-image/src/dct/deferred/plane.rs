//! One component's native samples, reconstructed while its entropy-coded
//! blocks are consumed.
use super::super::entropy::{Bits, Huffman};
use super::basis::Reference;
use pagura_core::{Error, Result};

/// The blocks of a component and how its plane stores them.
#[derive(Debug, Clone, Copy)]
pub(in crate::dct) struct Shape {
    /// Blocks per block row.
    pub(in crate::dct) stride: usize,
    /// Block rows.
    pub(in crate::dct) rows: usize,
    /// Whether the plane stores rows and columns exchanged.
    pub(in crate::dct) transposed: bool,
}

/// Sample bytes for eight-by-eight blocks in block-row order, initialized to
/// a background value, plus per-block flags. Transposed, rows and columns
/// are exchanged, blocks and flags with them.
pub(in crate::dct) struct Plane {
    shape: Shape,
    samples: Vec<u8>,
    /// Per block: nonzero once the block differs from the background.
    dirty: Vec<u8>,
    pitch: usize,
    /// The block being decoded: dequantized coefficients, column by column.
    scratch: [i32; 64],
    reference: Reference,
    /// Joined zero-DC-difference and end-of-block prefixes, built at the
    /// first block from that block's tables.
    shortcut: Vec<u32>,
    /// The last dequantized DC value and its reference sample.
    dc_cache: (i32, u8),
    background: u8,
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
    pub(in crate::dct) fn new(shape: Shape, background: u8) -> Result<Self> {
        let blocks = shape
            .stride
            .checked_mul(shape.rows)
            .ok_or(Error::limit("JPEG native storage overflow"))?;
        let length = blocks
            .checked_mul(64)
            .ok_or(Error::limit("JPEG native storage overflow"))?;
        let mut shortcut = Vec::new();
        shortcut
            .try_reserve_exact(4096)
            .map_err(|_| Error::limit("JPEG native prefix allocation failed"))?;
        Ok(Self {
            shape,
            samples: filled(length, background, "JPEG native allocation failed")?,
            dirty: filled(blocks, 0, "JPEG block flag allocation failed")?,
            pitch: if shape.transposed {
                shape.rows * 8
            } else {
                shape.stride * 8
            },
            scratch: [0; 64],
            reference: Reference::new(),
            shortcut,
            dc_cache: (0, 128),
            background,
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

    /// Whether rows and columns are exchanged.
    pub(in crate::dct) fn transposed(&self) -> bool {
        self.shape.transposed
    }

    pub(in crate::dct) fn bytes(&self) -> usize {
        self.samples.capacity()
            + self.dirty.capacity()
            + self.shortcut.capacity() * size_of::<u32>()
    }

    /// Whether a block that repeats the previous DC with an immediate end of
    /// block is background. The predictor/quantizer product is in the
    /// baseline range.
    pub(in crate::dct) fn at_background(&self, predictor: i32, quant: &[u16; 64]) -> bool {
        self.dc_cache.1 == self.background
            && predictor.wrapping_mul(i32::from(quant[0])) == self.dc_cache.0
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
        let Shape {
            stride,
            rows,
            transposed,
        } = self.shape;
        if column >= stride || row >= rows {
            return Err(bits.error("JPEG block outside component"));
        }
        if self.shortcut.is_empty() {
            let table = bits.native_shortcut(dc, ac)?;
            self.shortcut.extend_from_slice(&table);
        }
        if self.at_background(*predictor, quant) && bits.native_zero(&self.shortcut)? {
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
        // Where the block is stored: exchanged with its rows and columns.
        // A block within the plane's `stride` × `rows` has a flag and 64
        // samples there, so these products cannot wrap.
        let (flag, base) = if transposed {
            (
                column.wrapping_mul(rows).wrapping_add(row),
                column.wrapping_mul(self.pitch).wrapping_add(row),
            )
        } else {
            (
                row.wrapping_mul(stride).wrapping_add(column),
                row.wrapping_mul(self.pitch).wrapping_add(column),
            )
        };
        *self
            .dirty
            .get_mut(flag)
            .ok_or_else(|| bits.error("JPEG block outside component"))? = 1;
        let output = if nonconstant {
            let block = self.reconstruct(bits, l1)?;
            if transposed { transpose(block) } else { block }
        } else {
            [[constant; 8]; 8]
        };
        for (y, row) in output.iter().enumerate() {
            let start = base
                .wrapping_mul(8)
                .wrapping_add(y.wrapping_mul(self.pitch));
            self.samples
                .get_mut(start..start.wrapping_add(8))
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
        clippy::cast_precision_loss,
        reason = "outward-rounded error bound and clamped byte samples; the coefficient bound covers f32 conversion"
    )]
    #[inline(never)]
    fn reconstruct(&mut self, bits: &mut Bits<'_>, l1: u64) -> Result<[[u8; 8]; 8]> {
        let mut input = [[0.0; 8]; 8];
        for (column, values) in input.iter_mut().zip(self.scratch.as_chunks::<8>().0) {
            *column = values.map(|v| v as f32);
        }
        let l1 = l1 as f64;
        let bound = l1 * (1.0 / 1_048_576.0) + 1.0 / 16384.0;
        let error = (bound as f32) * 1.000_000_2;
        let values = super::spatial::spatial(&input);
        let byte = crate::deferred::floor_byte;
        let mut result = [[0; 8]; 8];
        let mut upper = [0; 64];
        for ((low, high), &value) in result
            .as_flattened_mut()
            .iter_mut()
            .zip(&mut upper)
            .zip(values.as_flattened())
        {
            *low = byte(value + 128.5 - error);
            *high = byte(value + 128.5 + error);
        }
        // Bit `8y + x` where the bounds of sample (x, y) round apart.
        let mut mask = 0;
        for (low, high) in result.iter().zip(upper.as_chunks::<8>().0).rev() {
            let differs = u64::from_le_bytes(*low) ^ u64::from_le_bytes(*high);
            mask = mask << 8 | nonzero_bytes(differs);
        }
        if mask != 0 {
            let work = self.reference.refine(&self.scratch, mask, &mut result);
            bits.charge_refinement(work)?;
        }
        Ok(result)
    }
}

/// Bit `i` set where byte `i` (little-endian) of `word` is nonzero.
fn nonzero_bytes(word: u64) -> u64 {
    const LOW: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    let high = ((word & LOW).wrapping_add(LOW) | word) & !LOW;
    (high >> 7).wrapping_mul(0x0102_0408_1020_4080) >> 56
}

/// `block` with rows and columns exchanged.
fn transpose(block: [[u8; 8]; 8]) -> [[u8; 8]; 8] {
    let mut out = [[0; 8]; 8];
    for (y, row) in block.iter().enumerate() {
        for (x, &value) in row.iter().enumerate() {
            out[x][y] = value;
        }
    }
    out
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
        let shape = Shape {
            stride: 1,
            rows: 1,
            transposed: false,
        };
        let mut plane = Plane::new(shape, 255).unwrap();
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
