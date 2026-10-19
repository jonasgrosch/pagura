//! Nonzero row runs, separating solid interiors from fractional edge samples.

pub(super) fn rows(coverage: &[u8], width: usize) -> impl Iterator<Item = (usize, usize, &[u8])> {
    coverage
        .chunks_exact(width)
        .enumerate()
        .flat_map(|(y, row)| Runs::new(row).map(move |(x, samples)| (y, x, samples)))
}

/// One row's nonzero runs as `(column, samples)`.
pub(crate) struct Runs<'a> {
    row: &'a [u8],
    x: usize,
}
impl<'a> Runs<'a> {
    pub(crate) fn new(row: &'a [u8]) -> Self {
        Self { row, x: 0 }
    }
}
impl<'a> Iterator for Runs<'a> {
    type Item = (usize, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.x + position(self.row.get(self.x..)?, |v| v)?;
        let rest = &self.row[start..];
        // Gaps and solid interiors can be long; fractional edge runs are short.
        let length = if rest[0] == 255 {
            position(rest, |v| v ^ 255)
        } else {
            rest.iter().position(|&v| v == 0 || v == 255)
        }
        .unwrap_or(rest.len());
        self.x = start + length;
        Some((start, &self.row[start..self.x]))
    }
}

/// The index of the first byte that `differs` maps to a nonzero value.
///
/// Each 16-byte block reduces to one bitwise OR, which the compiler can
/// vectorize; only the block holding the boundary is searched byte by byte.
fn position(row: &[u8], differs: impl Fn(u8) -> u8) -> Option<usize> {
    let (blocks, tail) = row.as_chunks::<16>();
    for (index, block) in blocks.iter().enumerate() {
        if block.iter().fold(0, |any, &v| any | differs(v)) != 0 {
            return block
                .iter()
                .position(|&v| differs(v) != 0)
                .map(|n| index * 16 + n);
        }
    }
    tail.iter()
        .position(|&v| differs(v) != 0)
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

    /// Byte-at-a-time reference for `Runs`.
    fn reference(row: &[u8]) -> Vec<(usize, &[u8])> {
        let mut runs = Vec::new();
        let mut x = 0;
        while let Some(offset) = row[x..].iter().position(|&v| v != 0) {
            let start = x + offset;
            let full = row[start] == 255;
            let length = row[start..]
                .iter()
                .position(|&v| v == 0 || (v == 255) != full)
                .unwrap_or(row.len() - start);
            x = start + length;
            runs.push((start, &row[start..x]));
        }
        runs
    }

    #[test]
    fn block_search_matches_bytewise_runs_at_every_offset_and_tail() {
        for length in 1..98 {
            for fill in [0, 37, 255] {
                for edge in [0, 37, 255] {
                    for index in 0..length {
                        for other in [0, 128, 255] {
                            let mut row = vec![fill; length];
                            row[index] = edge;
                            row[(index * 7 + 3) % length] = other;
                            let actual: Vec<_> =
                                rows(&row, length).map(|(_, x, run)| (x, run)).collect();
                            assert_eq!(actual, reference(&row));
                        }
                    }
                }
            }
        }
    }
}
