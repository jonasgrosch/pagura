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
        let start = self.x + self.row.get(self.x..)?.iter().position(|&v| v != 0)?;
        let rest = &self.row[start..];
        let full = rest[0] == 255;
        let length = rest
            .iter()
            .position(|&v| v == 0 || (v == 255) != full)
            .unwrap_or(rest.len());
        self.x = start + length;
        Some((start, &self.row[start..self.x]))
    }
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
