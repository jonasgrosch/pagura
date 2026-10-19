//! Nonzero row runs, separating solid interiors from fractional edge samples.

pub(super) fn rows(coverage: &[u8], width: usize) -> impl Iterator<Item = (usize, usize, &[u8])> {
    coverage
        .chunks_exact(width)
        .enumerate()
        .flat_map(|(y, row)| Runs { row, x: 0 }.map(move |(x, samples)| (y, x, samples)))
}

struct Runs<'a> {
    row: &'a [u8],
    x: usize,
}
impl<'a> Iterator for Runs<'a> {
    type Item = (usize, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.x + find(self.row.get(self.x..)?, |v| v)?;
        let rest = &self.row[start..];
        let full = rest[0] == 255;
        let length = if full {
            find(rest, |v| v ^ 255)
        } else {
            find(rest, |v| u8::from(v == 0 || v == 255))
        }
        .unwrap_or(rest.len());
        self.x = start + length;
        Some((start, &self.row[start..self.x]))
    }
}

// Fixed-width byte reductions let LLVM use the same vector mechanism on
// NEON, x86 and SIMD128. Only a block containing a boundary needs a byte search.
fn find(row: &[u8], mismatch: impl Fn(u8) -> u8) -> Option<usize> {
    let (blocks, tail) = row.as_chunks::<16>();
    for (index, block) in blocks.iter().enumerate() {
        let mut changed = 0u8;
        for &value in block {
            changed |= mismatch(value);
        }
        if changed != 0 {
            return block
                .iter()
                .position(|&v| mismatch(v) != 0)
                .map(|n| index * 16 + n);
        }
    }
    tail.iter()
        .position(|&v| mismatch(v) != 0)
        .map(|n| blocks.len() * 16 + n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_exclude_zeroes_and_separate_solid_interiors() {
        let input = [0, 32, 64, 255, 255, 0, 128, 0, 255, 255, 0, 0, 7, 9, 255, 0];
        let actual: Vec<_> = rows(&input, 8).collect();
        assert_eq!(
            actual,
            [
                (0, 1, &[32, 64][..]),
                (0, 3, &[255, 255][..]),
                (0, 6, &[128][..]),
                (1, 0, &[255, 255][..]),
                (1, 4, &[7, 9][..]),
                (1, 6, &[255][..])
            ]
        );
        let mut restored = [0; 16];
        for (y, x, samples) in actual {
            restored[y * 8 + x..y * 8 + x + samples.len()].copy_from_slice(samples);
        }
        assert_eq!(restored, input);
        assert!(rows(&[0; 16], 8).next().is_none());
    }
}

#[cfg(test)]
mod vector_tests {
    use super::*;
    #[test]
    fn vector_search_finds_boundaries_in_every_lane_and_tail() {
        for length in 1..98 {
            assert!(rows(&vec![0; length], length).next().is_none());
            let full = vec![255; length];
            assert_eq!(
                rows(&full, length).collect::<Vec<_>>(),
                [(0, 0, full.as_slice())]
            );
            for index in 0..length {
                let mut pixels = vec![0; length];
                pixels[index] = 37;
                assert_eq!(
                    rows(&pixels, length).collect::<Vec<_>>(),
                    [(0, index, &[37][..])]
                );
                let mut pixels = vec![255; length];
                pixels[index] = 37;
                let runs = rows(&pixels, length).collect::<Vec<_>>();
                assert!(
                    runs.iter()
                        .all(|(_, _, v)| v.iter().all(|&n| (n == 255) == (v[0] == 255)))
                );
                let mut restored = vec![0; length];
                for (_, x, values) in runs {
                    restored[x..x + values.len()].copy_from_slice(values);
                }
                assert_eq!(restored, pixels);
            }
        }
    }
}
