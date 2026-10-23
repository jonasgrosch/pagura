//! The cell arithmetic every coverage producer shares: signed area within
//! each cell and cover carried to the cells right of it.
//!
//! An edge with direction d (+1 downward, -1 upward) adds d times the part of
//! each cell right of it within the row, and d times its height to every
//! cell entirely right of it. The prefix sum from the page's left edge is
//! then each cell's integral of the winding number. Cells are indexed by
//! device column from 0, independent of the requested region; geometry left
//! of column 0 adds to cell 0's cover, and geometry right of the row's width
//! is dropped, since it cannot change cells left of it.

use crate::{FillRule, quantize_unit};
use pagura_core::{Error, Result};

/// Receives a row's coverage left to right as runs of equal value.
pub(crate) trait Sink {
    /// Pixels `x0..x1` have `value`.
    fn run(&mut self, x0: usize, x1: usize, value: u8);

    /// Pixels from `x`, which edges cross or touch, have `values`.
    #[inline]
    fn cells(&mut self, x: usize, values: &[u8]) {
        for (k, &value) in values.iter().enumerate() {
            self.run(x + k, x + k + 1, value);
        }
    }
}

/// Signed-area accumulation into one row of cells: an edge with direction
/// d adds d times the part of each cell right of it, and d times its height
/// to the cover of the cell after its last. Every producer shares this
/// arithmetic, so equal edges give equal bits in any cell store.
pub(crate) trait Accumulate {
    /// The device column of the first cell.
    fn base(&self) -> i64;
    /// The number of cells.
    fn width(&self) -> usize;
    /// Adds `value` to the area of cell `column`; outside the row it drops.
    fn area_at(&mut self, column: i64, value: f64);
    /// Adds `value` to the cover of cell `column`, or of the first cell for
    /// columns left of it; right of the row it drops.
    fn cover_at(&mut self, column: i64, value: f64);

    /// Preparation can measure primitive error before cancellation. The
    /// execution accumulator specializes this hook away.
    fn area_error(&mut self, _column: i64, _error: f64) {}

    /// Adds a straight piece within one row, from x `xa` to `xb`, of signed
    /// height `h`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "device coordinates are bounded by 10^12, within i64"
    )]
    fn piece(&mut self, xa: f64, xb: f64, h: f64) -> usize {
        let (l, r) = if xa < xb { (xa, xb) } else { (xb, xa) };
        let start = l.floor();
        let end = r.ceil();
        let first = start as i64;
        if end <= start + 1.0 {
            if end == start {
                // Vertical on a column boundary: every cell from it on lies
                // right of the piece.
                self.cover_at(first, h);
                return 1;
            }
            #[expect(clippy::manual_midpoint, reason = "bounded coordinates; one rounding")]
            let middle = (xa + xb) * 0.5;
            self.area_at(first, h * ((start + 1.0) - middle));
            self.area_error(
                first,
                64.0 * f64::EPSILON * h.abs() * (xa.abs() + xb.abs() + (start + 1.0).abs() + 1.0),
            );
            self.cover_at(first + 1, h);
            return 2;
        }
        let last = end as i64;
        let s = h / (r - l);
        let width = i64::try_from(self.width()).unwrap_or(i64::MAX);
        let from = first.max(self.base());
        let to = last.min(self.base().saturating_add(width));
        for column in from..to {
            let c = column as f64;
            let x0 = l.max(c);
            let x1 = r.min(c + 1.0);
            let near = (c + 1.0) - x0;
            let far = (c + 1.0) - x1;
            let right = s * ((x0 - l) + (near * near - far * far) * 0.5);
            self.area_at(column, right);
            self.area_error(
                column,
                64.0 * f64::EPSILON
                    * (h.abs()
                        + s.abs()
                            * (x0.abs()
                                + l.abs()
                                + near * near
                                + far * far
                                + (near.abs() + far.abs())
                                    * ((c + 1.0).abs() + x0.abs() + x1.abs()))),
            );
        }
        self.cover_at(last, h);
        usize::try_from(to - from).unwrap_or(0) + 1
    }
}

/// The coverage a winding integral gives under `rule`.
#[inline]
pub(crate) fn fold(value: f64, rule: FillRule) -> u8 {
    let area = match rule {
        FillRule::NonZero => value.abs(),
        FillRule::EvenOdd => {
            let t = value.abs() % 2.0;
            t.min(2.0 - t)
        }
    };
    quantize_unit(area)
}

/// The row width for a region ending at column `x1`.
pub(crate) fn width(x1: u32) -> Result<usize> {
    usize::try_from(x1).map_err(|_| Error::limit("row width exceeds usize"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_odd_folds_windings_of_two() {
        assert_eq!(fold(2.0, FillRule::EvenOdd), 0);
        assert_eq!(fold(-1.0, FillRule::EvenOdd), 255);
        assert_eq!(fold(1.5, FillRule::EvenOdd), 128);
        assert_eq!(fold(2.0, FillRule::NonZero), 255);
    }
}
