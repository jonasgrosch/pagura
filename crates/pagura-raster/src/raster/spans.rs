//! Nonzero row runs, separating solid interiors from fractional edge samples.

use crate::IntRect;
use pagura_core::convert::u32_to_usize;

/// A horizontal run of nonzero coverage within one row of a
/// [`Mask`](crate::Mask). Its samples are either all 255 or all fractional
/// (1..=254), so a solid run can be painted as one fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span<'a> {
    row: usize,
    column: usize,
    x: u32,
    y: u32,
    coverage: &'a [u8],
}

impl<'a> Span<'a> {
    /// The row, counted from the mask's top edge.
    pub const fn row(&self) -> usize {
        self.row
    }

    /// The first column, counted from the mask's left edge.
    pub const fn column(&self) -> usize {
        self.column
    }

    /// The first column in global device space.
    pub const fn x(&self) -> u32 {
        self.x
    }

    /// The row in global device space.
    pub const fn y(&self) -> u32 {
        self.y
    }

    /// The samples, at least one; they borrow the mask's coverage.
    pub const fn coverage(&self) -> &'a [u8] {
        self.coverage
    }

    /// Whether every sample is 255.
    pub fn is_solid(&self) -> bool {
        self.coverage.first() == Some(&255)
    }
}

/// The spans of row-major `coverage` over `area`.
pub(super) fn rows(coverage: &[u8], area: IntRect) -> impl Iterator<Item = Span<'_>> {
    // Rows and columns stay inside `area`, whose far edges fit u32.
    coverage
        .chunks_exact(u32_to_usize(area.width()))
        .zip(area.y()..)
        .enumerate()
        .flat_map(move |(row, (samples, y))| {
            Runs { row: samples, x: 0 }.map(move |(column, coverage)| Span {
                row,
                column,
                x: area
                    .x()
                    .saturating_add(u32::try_from(column).unwrap_or(u32::MAX)),
                y,
                coverage,
            })
        })
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
        let area = IntRect::new(10, 20, 8, 2).unwrap();
        let actual: Vec<_> = rows(&input, area).collect();
        let span = |row, column: usize, coverage| Span {
            row,
            column,
            x: 10 + u32::try_from(column).unwrap(),
            y: 20 + u32::try_from(row).unwrap(),
            coverage,
        };
        assert_eq!(
            actual,
            [
                span(0, 1, &[32, 64][..]),
                span(0, 3, &[255, 255][..]),
                span(0, 6, &[128][..]),
                span(1, 0, &[255, 255][..]),
                span(1, 4, &[7, 9][..]),
                span(1, 6, &[255][..])
            ]
        );
        assert_eq!(
            actual.iter().map(Span::is_solid).collect::<Vec<_>>(),
            [false, true, false, true, false, true]
        );
        let mut restored = [0; 16];
        for span in actual {
            let start = span.row() * 8 + span.column();
            restored[start..start + span.coverage().len()].copy_from_slice(span.coverage());
        }
        assert_eq!(restored, input);
        assert!(rows(&[0; 16], area).next().is_none());
    }
}
