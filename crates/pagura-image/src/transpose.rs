//! Decoded images with rows and columns exchanged, so that a painter of a
//! quarter-turned placement reads its source rows contiguously.

use crate::deferred::{Packed, Planar};
use crate::{DecodedImage, ImageAlpha, MaskKind, Samples};
use pagura_core::{Error, Limits, Result};

/// The side of the square tiles values are exchanged in, which keeps both
/// the rows read and the rows written in cache.
const TILE: usize = 16;

impl DecodedImage {
    /// The image with rows and columns exchanged: sample (`x`, `y`) of the
    /// result is sample (`y`, `x`) of `self`, with every component and the
    /// embedded opacity exchanged alike.
    ///
    /// # Errors
    ///
    /// A `Limit` error when the samples exceed `limits.decoded_bytes` or
    /// cannot be allocated.
    pub fn transposed(&self, limits: &Limits) -> Result<Self> {
        let size = (
            usize::try_from(self.width).map_err(|_| Error::limit("image width overflow"))?,
            usize::try_from(self.height).map_err(|_| Error::limit("image height overflow"))?,
        );
        let components = usize::from(self.components);
        let samples = transpose_samples(&self.samples, size, components, limits)?;
        let mask = match &self.mask {
            MaskKind::Embedded(alpha) => MaskKind::Embedded(ImageAlpha {
                bits_per_component: alpha.bits_per_component,
                samples: transpose_samples(&alpha.samples, size, 1, limits)?,
                premultiplied: alpha.premultiplied,
            }),
            mask => mask.clone(),
        };
        Ok(Self {
            width: self.height,
            height: self.width,
            bits_per_component: self.bits_per_component,
            components: self.components,
            samples,
            kind: self.kind.clone(),
            decode: self.decode.clone(),
            mask,
            interpolate: self.interpolate,
        })
    }
}

fn transpose_samples(
    samples: &Samples,
    size: (usize, usize),
    components: usize,
    limits: &Limits,
) -> Result<Samples> {
    Ok(match samples {
        Samples::Bytes(values) => {
            Samples::Bytes(values_transposed(values, size, components, limits)?)
        }
        Samples::Words(values) => {
            Samples::Words(values_transposed(values, size, components, limits)?)
        }
        Samples::Mixed { values, bits } => Samples::Mixed {
            values: values_transposed(values, size, components, limits)?,
            bits: bits.clone(),
        },
        Samples::Packed(packed) => Samples::Packed(packed_transposed(packed, limits)?),
        Samples::Planar(planar) => Samples::Planar(Box::new(planar_transposed(planar, limits)?)),
    })
}

/// Storage for `length` values of `T` within `limits`, filled with zeros.
fn storage<T: Copy + Default>(length: usize, limits: &Limits) -> Result<Vec<T>> {
    if length.saturating_mul(size_of::<T>()) > limits.decoded_bytes {
        return Err(Error::limit("transposed image exceeds the sample budget"));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| Error::limit("transposed image allocation failed"))?;
    values.resize(length, T::default());
    Ok(values)
}

/// `values` of `width` × `height` samples of `n` components, exchanged.
fn values_transposed<T: Copy + Default>(
    values: &[T],
    (width, height): (usize, usize),
    n: usize,
    limits: &Limits,
) -> Result<Vec<T>> {
    if width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(n))
        != Some(values.len())
    {
        return Err(Error::syntax("image sample count mismatch"));
    }
    let mut out = storage(values.len(), limits)?;
    match n {
        1 => exchange::<T, 1>(values, (width, height), &mut out),
        2 => exchange::<T, 2>(values, (width, height), &mut out),
        3 => exchange::<T, 3>(values, (width, height), &mut out),
        4 => exchange::<T, 4>(values, (width, height), &mut out),
        _ => {
            for (y, row) in values.chunks_exact(width * n).enumerate() {
                for (x, sample) in row.chunks_exact(n).enumerate() {
                    let at = (x * height + y) * n;
                    out[at..at + n].copy_from_slice(sample);
                }
            }
        }
    }
    Ok(out)
}

/// Exchanges rows and columns of `N`-component samples, tile by tile.
fn exchange<T: Copy, const N: usize>(values: &[T], (width, height): (usize, usize), out: &mut [T]) {
    let rows = values.as_chunks::<N>().0;
    let columns = out.as_chunks_mut::<N>().0;
    for y0 in (0..height).step_by(TILE) {
        let y1 = (y0 + TILE).min(height);
        for x0 in (0..width).step_by(TILE) {
            let x1 = (x0 + TILE).min(width);
            for x in x0..x1 {
                let column = &mut columns[x * height + y0..x * height + y1];
                for (y, sample) in (y0..y1).zip(column) {
                    *sample = rows[y * width + x];
                }
            }
        }
    }
}

/// One-bit rows exchanged eight by eight bits. The result starts out as
/// the more common of the all-zero and all-one bytes, so that the uniform
/// blocks of that value, most of a scanned page, need no stores; padding
/// bits end up zero.
fn packed_transposed(packed: &Packed, limits: &Limits) -> Result<Packed> {
    let (width, height) = (packed.width(), packed.height());
    let (stride, source) = (height.div_ceil(8), packed.stride);
    // The fill: the more common of all-one and all-zero words.
    let (ones, zeros) =
        packed
            .bytes
            .as_chunks::<8>()
            .0
            .iter()
            .fold((0usize, 0usize), |(ones, zeros), word| {
                let word = u64::from_ne_bytes(*word);
                (
                    ones + usize::from(word == u64::MAX),
                    zeros + usize::from(word == 0),
                )
            });
    let fill = if ones > zeros { u8::MAX } else { 0 };
    let uniform = u64::from_ne_bytes([fill; 8]);
    let mut out = storage::<u8>(stride.saturating_mul(width), limits)?;
    out.fill(fill);
    for (block_row, rows) in packed.bytes.chunks(source * 8).enumerate() {
        let mut column = 0;
        while column < source {
            // Eight columns of eight rows that all hold the fill need
            // nothing.
            let words = column + 8 <= source
                && rows.len() == source * 8
                && rows.chunks_exact(source).all(|row| {
                    row.get(column..column + 8)
                        .and_then(|bytes| bytes.first_chunk::<8>())
                        .is_some_and(|bytes| u64::from_ne_bytes(*bytes) == uniform)
                });
            if words {
                column += 8;
                continue;
            }
            let end = (column + 8).min(source);
            for column in column..end {
                transpose_block(
                    rows,
                    (source, column),
                    (fill, uniform),
                    (block_row, stride, width),
                    &mut out,
                );
            }
            column = end;
        }
    }
    // Rows past the image's height in the last block row are padding.
    let padding = stride * 8 - height;
    if padding > 0 {
        let mask = u8::MAX << padding;
        for row in out.chunks_exact_mut(stride.max(1)) {
            if let Some(last) = row.last_mut() {
                *last &= mask;
            }
        }
    }
    Packed::from_rows(height, width, out).ok_or(Error::syntax("packed image size mismatch"))
}

/// Exchanges the block of bit column `column` of the up to eight packed
/// `rows`, `source` bytes each, into `out`, rows `stride` bytes apart, of
/// which `width` exist; rows past the image read as `fill`.
fn transpose_block(
    rows: &[u8],
    (source, column): (usize, usize),
    (fill, uniform): (u8, u64),
    (block_row, stride, width): (usize, usize, usize),
    out: &mut [u8],
) {
    let mut block = uniform;
    for (i, row) in rows.chunks_exact(source).enumerate() {
        let byte = row.get(column).copied().unwrap_or(fill);
        let shift = 56 - 8 * i;
        block = block & !(0xff << shift) | u64::from(byte) << shift;
    }
    if block == uniform {
        return;
    }
    // Uniform blocks are their own transposes.
    let bytes = if block == 0 || block == u64::MAX {
        block.to_be_bytes()
    } else {
        transpose_bits(block).to_be_bytes()
    };
    let first = column * 8;
    for (x, &byte) in (first..width).zip(&bytes) {
        if let Some(out) = out.get_mut(x * stride + block_row) {
            *out = byte;
        }
    }
}

/// An eight-by-eight bit matrix, rows as bytes from the most significant
/// down and columns as bits from the most significant down, exchanged.
fn transpose_bits(mut x: u64) -> u64 {
    let t = (x ^ (x >> 7)) & 0x00aa_00aa_00aa_00aa;
    x ^= t ^ (t << 7);
    let t = (x ^ (x >> 14)) & 0x0000_cccc_0000_cccc;
    x ^= t ^ (t << 14);
    let t = (x ^ (x >> 28)) & 0x0000_0000_f0f0_f0f0;
    x ^ t ^ (t << 28)
}

/// Component planes exchanged with their blocks, sampling factors and
/// block flags.
fn planar_transposed(planar: &Planar, limits: &Limits) -> Result<Planar> {
    let mut planes: [Vec<u8>; 3] = Default::default();
    let mut dirty: [Vec<u8>; 3] = Default::default();
    let mut geometry = [[0; 3]; 3];
    for c in 0..3 {
        let [h, v, pitch] = planar.geometry[c];
        let plane = &planar.planes[c];
        let rows = plane.len().checked_div(pitch).unwrap_or(0);
        planes[c] = values_transposed(plane, (pitch, rows), 1, limits)?;
        geometry[c] = [v, h, rows];
        let (stride, block_rows) = (pitch / 8, rows / 8);
        let flags = &planar.dirty[c];
        let mut exchanged = storage::<u8>(flags.len(), limits)?;
        for row in 0..block_rows {
            for column in 0..stride {
                if let (Some(&flag), Some(out)) = (
                    flags.get(row * stride + column),
                    exchanged.get_mut(column * block_rows + row),
                ) {
                    *out = flag;
                }
            }
        }
        dirty[c] = exchanged;
    }
    Ok(Planar {
        width: planar.height,
        height: planar.width,
        maximum: [planar.maximum[1], planar.maximum[0]],
        geometry,
        planes,
        dirty,
        backgrounds: planar.backgrounds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, components: u8, samples: Samples) -> DecodedImage {
        DecodedImage {
            width,
            height,
            bits_per_component: 8,
            components,
            samples,
            kind: crate::ImageKind::Color { color_space: None },
            decode: None,
            mask: MaskKind::None,
            interpolate: false,
        }
    }

    #[test]
    fn bit_blocks_exchange_rows_and_columns() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..1000 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let bit = |m: u64, row: usize, column: usize| (m >> (63 - (row * 8 + column))) & 1;
            let exchanged = transpose_bits(state);
            for row in 0..8 {
                for column in 0..8 {
                    assert_eq!(bit(exchanged, row, column), bit(state, column, row));
                }
            }
        }
    }

    #[test]
    fn every_layout_exchanges_its_samples() {
        let limits = Limits::default();
        for (width, height) in [(1, 1), (5, 3), (17, 33), (40, 9)] {
            for n in [1u8, 3, 4] {
                let count = width * height * usize::from(n);
                let bytes: Vec<u8> = (0..count)
                    .map(|i| u8::try_from(i * 7 % 251).unwrap())
                    .collect();
                let original = image(
                    u32::try_from(width).unwrap(),
                    u32::try_from(height).unwrap(),
                    n,
                    Samples::Bytes(bytes),
                );
                let turned = original.transposed(&limits).unwrap();
                assert_eq!(
                    (turned.width, turned.height),
                    (original.height, original.width)
                );
                for y in 0..height {
                    for x in 0..width {
                        for k in 0..usize::from(n) {
                            let at =
                                |x: usize, y: usize, w: usize| (y * w + x) * usize::from(n) + k;
                            assert_eq!(
                                turned.samples.get(at(y, x, height)),
                                original.samples.get(at(x, y, width))
                            );
                        }
                    }
                }
                assert_eq!(turned.transposed(&limits).unwrap(), original);
            }
            let rows: Vec<u8> = (0..width.div_ceil(8) * height)
                .map(|i| u8::try_from(i * 37 % 256).unwrap())
                .collect();
            let packed = Packed::from_rows(width, height, rows).unwrap();
            let turned = packed_transposed(&packed, &limits).unwrap();
            for y in 0..height {
                for x in 0..width {
                    assert_eq!(turned.get(x * height + y), packed.get(y * width + x));
                }
            }
        }
    }

    #[test]
    fn planes_exchange_with_their_sampling_factors() {
        let limits = Limits::default();
        let luma: Vec<u8> = (0..32 * 16)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        let chroma = |seed: usize| -> Vec<u8> {
            (0..16 * 8)
                .map(|i| u8::try_from((i * seed) % 256).unwrap())
                .collect()
        };
        let planar = Planar::new(
            (29, 13),
            [2, 2],
            [[2, 2, 32], [1, 1, 16], [1, 1, 16]],
            [luma, chroma(3), chroma(5)],
        )
        .unwrap();
        let turned = planar_transposed(&planar, &limits).unwrap();
        assert_eq!((turned.width(), turned.height()), (13, 29));
        let (mut row, mut column) = (vec![0; 13 * 3], vec![0; 29 * 3]);
        for x in 0..29 {
            assert!(turned.rgb(x, &mut row));
            for y in 0..13 {
                assert!(planar.rgb(y, &mut column));
                assert_eq!(row[y * 3..y * 3 + 3], column[x * 3..x * 3 + 3]);
            }
        }
    }
}
