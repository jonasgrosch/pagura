//! One-bit samples: packed footprints through phase-interned lookup tables,
//! popcount interiors, and exact two-by-two means.
use super::footprint::{Band, Tap};
use pagura_core::{Error, Limits, Result};

/// One footprint: the two source bytes from `byte` on, shifted right by
/// `shift` and masked to its taps, index table `table`.
struct Entry {
    byte: u32,
    shift: u8,
    mask: u8,
    table: u8,
}
/// Footprint lookup tables keyed by tap phase; at most 64 phases.
pub(super) struct Plan {
    entries: Vec<Entry>,
    tables: Vec<[f32; 256]>,
    maximum: usize,
    work: usize,
}
impl Plan {
    pub(super) fn new(
        bands: &[Band],
        taps: &[Tap],
        zero: f32,
        one: f32,
        limits: Limits,
    ) -> Result<Option<Self>> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(bands.len())
            .map_err(|_| Error::limit("binary plan allocation failed"))?;
        let mut tables = Vec::new();
        let mut phases = Vec::new();
        phases
            .try_reserve_exact(64)
            .map_err(|_| Error::limit("binary phase allocation failed"))?;
        let mut work = 0usize;
        let mut maximum = 0;
        for band in bands {
            let mut t = &taps[band.start..band.end];
            while t.len() > 1 && t[0].weight < 1e-7 {
                t = &t[1..];
            }
            while t.len() > 1 && t[t.len() - 1].weight < 1e-7 {
                t = &t[..t.len() - 1];
            }
            work = work
                .checked_add(phases.len() + 1)
                .filter(|&v| v <= limits.raster_work)
                .ok_or(Error::limit("binary phase work exceeded"))?;
            if t.len() > 8 {
                return Ok(None);
            }
            let first = t[0].sample;
            let n = t.len();
            let last = first + n - 1;
            maximum = maximum.max(last / 8 + 1);
            let key = (
                n,
                t[0].weight.to_bits(),
                if n > 2 { t[1].weight.to_bits() } else { 0 },
                t[n - 1].weight.to_bits(),
            );
            let table = if let Some(index) = phases.iter().position(|p| p == &key) {
                index
            } else {
                if phases.len() == 64 {
                    return Ok(None);
                }
                work = work
                    .checked_add(256 * t.len())
                    .filter(|&v| v <= limits.raster_work)
                    .ok_or(Error::limit("binary table work exceeded"))?;
                let size = (tables.len() + 1)
                    .checked_mul(size_of::<[f32; 256]>())
                    .and_then(|b| {
                        entries
                            .capacity()
                            .checked_mul(size_of::<Entry>())
                            .and_then(|e| b.checked_add(e))
                    })
                    .filter(|&b| b <= limits.decoded_bytes)
                    .ok_or(Error::limit("binary table budget exceeded"))?;
                let _ = size;
                tables
                    .try_reserve_exact(1)
                    .map_err(|_| Error::limit("binary table allocation failed"))?;
                let mask = (1usize << n) - 1;
                tables.push(std::array::from_fn(|i| {
                    let i = i & mask;
                    if i == 0 {
                        return zero;
                    }
                    if i == mask {
                        return one;
                    }
                    let mut sum = 0.0;
                    for (k, tap) in t.iter().enumerate() {
                        sum += f32::from(u8::from(i & (1 << (n - 1 - k)) != 0)) * tap.weight;
                    }
                    zero + (one - zero) * sum
                }));
                let index = tables.len() - 1;
                phases.push(key);
                index
            };
            // Phases are at most 64 and footprints at most eight taps.
            entries.push(Entry {
                byte: u32::try_from(first / 8)
                    .map_err(|_| Error::limit("binary row exceeds plan range"))?,
                shift: u8::try_from(16 - n - first % 8).unwrap_or(0),
                mask: u8::try_from((1usize << n) - 1).unwrap_or(0),
                table: u8::try_from(table).unwrap_or(0),
            });
        }
        Ok(Some(Self {
            entries,
            tables,
            maximum,
            work,
        }))
    }
    pub(super) fn work(&self) -> usize {
        self.work
    }
    pub(super) fn row(&self, source: &[u8], output: &mut [f32]) -> bool {
        if source.len() < self.maximum || output.len() != self.entries.len() {
            return false;
        }
        for (e, out) in self.entries.iter().zip(output) {
            let byte = e.byte as usize;
            let pair = u16::from_be_bytes([
                source.get(byte).copied().unwrap_or(0),
                source.get(byte + 1).copied().unwrap_or(0),
            ]);
            let index = pair.wrapping_shr(u32::from(e.shift)).to_le_bytes()[0] & e.mask;
            *out = self.tables[usize::from(e.table)][usize::from(index)];
        }
        true
    }
}

/// Footprints of unpacked one-bit codes (0 or 1) scaled to levels.
pub(super) fn byte_row(source: &[u8], bands: &[Band], taps: &[Tap], output: &mut [[f32; 1]]) {
    for (out, band) in output.iter_mut().zip(bands) {
        let t = &taps[band.start..band.end];
        let first = t[0];
        let last = t[t.len() - 1];
        if first.sample == last.sample {
            out[0] = f32::from(source[first.sample]) * 255.0;
            continue;
        }
        let n = last.sample - first.sample + 1;
        if n <= 8 && first.sample + 8 <= source.len() {
            let word = u64::from_le_bytes(
                source[first.sample..first.sample + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            let mask = u64::MAX >> (8 * (8 - n));
            let word = word & mask;
            if word == 0 {
                out[0] = 0.0;
                continue;
            }
            if word == 0x0101_0101_0101_0101 & mask {
                out[0] = 255.0;
                continue;
            }
            let middle = if n > 2 {
                (word >> 8) & (u64::MAX >> (8 * (10 - n)))
            } else {
                0
            };
            let sum = middle.count_ones() as f32;
            out[0] = (f32::from(source[first.sample]) * first.weight
                + sum * if n > 2 { t[1].weight } else { 0.0 }
                + f32::from(source[last.sample]) * last.weight)
                * 255.0;
        } else {
            let mut sum = 0.0;
            for tap in t {
                sum += f32::from(source[tap.sample]) * tap.weight;
            }
            out[0] = sum * 255.0;
        }
    }
}

/// Footprints of packed bits mapped to the levels `zero` and `one`.
pub(super) fn packed_row(
    source: &[u8],
    bands: &[Band],
    taps: &[Tap],
    zero: f32,
    one: f32,
    output: &mut [f32],
) {
    let bit = |x: usize| f32::from((source[x / 8] >> (7 - x % 8)) & 1);
    for (out, band) in output.iter_mut().zip(bands) {
        let t = &taps[band.start..band.end];
        let first = t[0];
        let last = t[t.len() - 1];
        let n = last.sample - first.sample + 1;
        if n == 1 {
            *out = zero + (one - zero) * bit(first.sample);
            continue;
        }
        if n <= 8 {
            let i = first.sample / 8;
            let pair =
                (u16::from(source[i]) << 8) | u16::from(source.get(i + 1).copied().unwrap_or(0));
            let mask = (1u16 << n) - 1;
            let bits = (pair >> (16 - n - first.sample % 8)) & mask;
            if bits == 0 {
                *out = zero;
                continue;
            }
            if bits == mask {
                *out = one;
                continue;
            }
            let interior = if n > 2 {
                (bits >> 1) & ((1u16 << (n - 2)) - 1)
            } else {
                0
            };
            *out = zero
                + (one - zero)
                    * (f32::from((bits >> (n - 1)) & 1) * first.weight
                        + interior.count_ones() as f32 * if n > 2 { t[1].weight } else { 0.0 }
                        + f32::from(bits & 1) * last.weight);
        } else {
            let mut sum = 0.0;
            for tap in t {
                sum += bit(tap.sample) * tap.weight;
            }
            *out = zero + (one - zero) * sum;
        }
    }
}

// Every input pair describes four independent cells. The table replaces
// per-bit extraction and rounding with one checked lookup and one word store.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the table index is sixteen bits and every output sample is a byte"
)]
static MEANS: [u32; 65536] = {
    let mut table = [0; 65536];
    let mut index = 0usize;
    while index < table.len() {
        let top = index >> 8;
        let bottom = index & 255;
        let mut word = 0u32;
        let mut cell = 0;
        while cell < 4 {
            let shift = 6 - cell * 2;
            let count = ((top >> shift) & 1)
                + ((top >> (shift + 1)) & 1)
                + ((bottom >> shift) & 1)
                + ((bottom >> (shift + 1)) & 1);
            let value = ((count * 255 + 2) / 4) as u32;
            word |= value << (cell * 8);
            cell += 1;
        }
        table[index] = word;
        index += 1;
    }
    table
};
/// Reduces two packed rows; returns false for incompatible extents.
pub(super) fn half(top: &[u8], bottom: &[u8], output: &mut [u8]) -> bool {
    let bytes = output.len().div_ceil(4);
    if top.len() < bytes || bottom.len() < bytes {
        return false;
    }
    let wide = output.len() / 4;
    for ((out, &a), &b) in output[..wide * 4]
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(top)
        .zip(bottom)
    {
        let index = usize::from(u16::from(a) << 8 | u16::from(b));
        *out = MEANS[index].to_le_bytes();
    }
    let tail = output.len() % 4;
    if tail != 0 {
        let index = usize::from(u16::from(top[wide]) << 8 | u16::from(bottom[wide]));
        output[wide * 4..].copy_from_slice(&MEANS[index].to_le_bytes()[..tail]);
    }
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_packed_pairs_and_padding_match_direct_binary_means() {
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let mut output = [0; 69];
                assert!(half(&[a; 18], &[b; 18], &mut output));
                for (x, &value) in output.iter().enumerate() {
                    let shift = 6 - (x % 4) * 2;
                    let n = ((a >> shift) & 1)
                        + ((a >> (shift + 1)) & 1)
                        + ((b >> shift) & 1)
                        + ((b >> (shift + 1)) & 1);
                    assert_eq!(u16::from(value), (u16::from(n) * 255 + 2) / 4);
                }
            }
        }
        assert!(!half(&[], &[], &mut [0; 1]));
    }
}
