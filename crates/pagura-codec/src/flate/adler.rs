//! Adler-32 (RFC 1950 2.2) in a form compilers vectorise.
//!
//! For bytes `x[0..n]`, the sums advance as `a' = a + Σ x[i]` and
//! `b' = b + n·a + Σ (n - i)·x[i]`. Take the input in blocks of [`LANES`]
//! bytes and the blocks in groups of `m` (at most [`GROUP`]). Within a
//! group, lane `j` keeps `column[j]`, the running total of byte `j` of each
//! block, and `weighted[j]`, the running total of `column[j]` after each
//! block, which weights block `k` by `m - k`. The group then adds
//! `LANES·Σ weighted[j] - Σ j·column[j]` to `b`. Both lanes are 16-bit
//! vector additions with no dependence between lanes; the groups fold into
//! 32-bit lane totals, and those into the sums once per [`GROUPS`] groups.

const MODULUS: u64 = 65521;
const LANES: usize = 32;
/// `weighted[j] ≤ 255·m·(m + 1)/2` fits in 16 bits for `m ≤ 22`.
const GROUP: usize = 22;
/// Group totals per fold: the running total of the column totals stays
/// below 255·22·1024·1023/2 < 2^32.
const GROUPS: usize = 1024;

pub(super) struct Adler32 {
    a: u64,
    b: u64,
}

impl Adler32 {
    pub(super) fn new() -> Self {
        Self { a: 1, b: 0 }
    }

    pub(super) fn value(&self) -> u32 {
        // Both sums are reduced below the modulus after every update.
        #[expect(clippy::cast_possible_truncation, reason = "both sums are below 2^16")]
        let value = ((self.b << 16) | self.a) as u32;
        value
    }

    pub(super) fn update(&mut self, bytes: &[u8]) {
        // Local sums and indexed lanes keep the folds in registers.
        let (mut a, mut b) = (self.a, self.b);
        let (blocks, tail) = bytes.as_chunks::<LANES>();
        for chunk in blocks.chunks(GROUP * GROUPS) {
            let (groups, rest) = chunk.as_chunks::<GROUP>();
            let mut columns = [0_u32; LANES];
            let mut weights = [0_u32; LANES];
            let mut prior = [0_u32; LANES];
            for group in groups {
                let (column, weighted) = group_sums(group);
                for j in 0..LANES {
                    // No total wraps, by the GROUPS bound.
                    prior[j] = prior[j].wrapping_add(columns[j]);
                    columns[j] = columns[j].wrapping_add(u32::from(column[j]));
                    weights[j] = weights[j].wrapping_add(u32::from(weighted[j]));
                }
            }
            // Every full group starts from the `a` of all groups before it.
            let size = (GROUP * LANES) as u64;
            let count = groups.len() as u64;
            let before: u64 = prior.iter().copied().map(u64::from).sum();
            (a, b) = fold(a, b, &columns, &weights, size * (count * a + before));
            if !rest.is_empty() {
                let (column, weighted) = group_sums(rest);
                let (column, weighted) = (column.map(u32::from), weighted.map(u32::from));
                (a, b) = fold(a, b, &column, &weighted, (rest.len() * LANES) as u64 * a);
            }
        }
        for &byte in tail {
            a += u64::from(byte);
            b += a;
        }
        self.a = a % MODULUS;
        self.b = b % MODULUS;
    }
}

/// Adds groups' lane totals to the sums, with `shift` the `a` each group
/// starts from times its length, and reduces them.
fn fold(a: u64, b: u64, columns: &[u32; LANES], weights: &[u32; LANES], shift: u64) -> (u64, u64) {
    let (mut a, mut b) = (a, b + shift);
    for (j, (&column, &weighted)) in columns.iter().zip(weights).enumerate() {
        // Each weight includes its column, so b never goes negative.
        b = b + LANES as u64 * u64::from(weighted) - j as u64 * u64::from(column);
        a += u64::from(column);
    }
    (a % MODULUS, b % MODULUS)
}

/// One group's column and weighted totals.
fn group_sums(blocks: &[[u8; LANES]]) -> ([u16; LANES], [u16; LANES]) {
    let mut column = [0_u16; LANES];
    let mut weighted = [0_u16; LANES];
    for block in blocks {
        // Neither wraps, by the GROUP bound.
        for j in 0..LANES {
            column[j] = column[j].wrapping_add(u16::from(block[j]));
            weighted[j] = weighted[j].wrapping_add(column[j]);
        }
    }
    (column, weighted)
}

#[cfg(test)]
mod tests {
    use super::Adler32;

    fn reference(bytes: &[u8]) -> u32 {
        let (mut a, mut b) = (1_u32, 0_u32);
        for &byte in bytes {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    #[test]
    fn matches_the_definition_across_arbitrary_boundaries() {
        let mut state = 0x2545_f491_u32;
        let mut noise = vec![0_u8; 200_000];
        for byte in &mut noise {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state.to_le_bytes()[0];
        }
        // All-255 input is the worst case for every bound; 1.5 MB spans two
        // folds of whole groups.
        for bytes in [vec![255; 1_500_000], vec![0; 70_000], noise] {
            let sizes = [
                0,
                1,
                31,
                32,
                33,
                703,
                704,
                705,
                5552,
                65_535,
                720_897,
                bytes.len(),
            ];
            for size in sizes {
                let bytes = &bytes[..size.min(bytes.len())];
                for chunk in [1, 7, 32, 258, 5552, 65_536, usize::MAX] {
                    if chunk < 32 && bytes.len() > 70_000 {
                        continue;
                    }
                    let mut adler = Adler32::new();
                    for piece in bytes.chunks(chunk.min(bytes.len()).max(1)) {
                        adler.update(piece);
                    }
                    assert_eq!(adler.value(), reference(bytes));
                }
            }
        }
    }
}
