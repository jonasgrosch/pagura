//! One dense row of cells: signed area within each cell and cover carried to
//! the cells right of it, with a bitmap of touched cells, so that nothing is
//! sorted and untouched runs cost one bit each.
//!
//! An edge with direction d (+1 downward, -1 upward) adds d times the part of
//! each cell right of it within the row, and d times its height to every
//! cell entirely right of it. The prefix sum from the page's left edge is
//! then each cell's integral of the winding number. Cells are indexed by
//! device column from 0, independent of the requested region; geometry left
//! of column 0 adds to cell 0's cover, and geometry right of the row's width
//! is dropped, since it cannot change cells left of it.

use super::curve::Quadratic;
use super::edges::Line;
use crate::{FillRule, quantize_unit};
use pagura_core::{Error, Meter, Result};

/// Receives a row's coverage left to right as runs of equal value.
pub(crate) trait Sink {
    /// Pixels `x0..x1` have `value`.
    fn run(&mut self, x0: usize, x1: usize, value: u8);
}

#[derive(Debug, Default)]
pub(crate) struct Row {
    /// The device column of the first cell; geometry left of it adds to the
    /// first cell's cover.
    base: i64,
    area: Vec<f64>,
    cover: Vec<f64>,
    touched: Vec<u64>,
}

impl Row {
    /// A row of `width` cells from column 0, charged to `bytes`.
    pub(crate) fn new(width: usize, bytes: &mut Meter) -> Result<Self> {
        Self::from(0, width, bytes)
    }

    /// A row of `width` cells from column `base`, charged to `bytes`.
    pub(crate) fn from(base: usize, width: usize, bytes: &mut Meter) -> Result<Self> {
        let mut row = Self {
            base: i64::try_from(base).map_err(|_| Error::limit("row base exceeds i64"))?,
            ..Self::default()
        };
        let words = width.div_ceil(64) + 1;
        bytes.reserve(&mut row.area, width)?;
        bytes.reserve(&mut row.cover, width)?;
        bytes.reserve(&mut row.touched, words)?;
        row.area.resize(width, 0.0);
        row.cover.resize(width, 0.0);
        row.touched.resize(words, 0);
        Ok(row)
    }

    /// One past the device column of the last cell.
    pub(crate) fn end(&self) -> usize {
        usize::try_from(self.base).unwrap_or(0) + self.area.len()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        (self.area.capacity() + self.cover.capacity())
            .saturating_mul(size_of::<f64>())
            .saturating_add(self.touched.capacity().saturating_mul(size_of::<u64>()))
    }

    #[inline]
    fn mark(&mut self, x: usize) {
        if let Some(word) = self.touched.get_mut(x / 64) {
            *word |= 1 << (x % 64);
        }
    }

    #[inline]
    fn add_area(&mut self, column: i64, value: f64) {
        if let Ok(x) = usize::try_from(column.wrapping_sub(self.base))
            && let Some(cell) = self.area.get_mut(x)
        {
            *cell += value;
            self.mark(x);
        }
    }

    #[inline]
    fn add_cover(&mut self, column: i64, value: f64) {
        let x = usize::try_from(column.wrapping_sub(self.base).max(0)).unwrap_or(usize::MAX);
        if let Some(cell) = self.cover.get_mut(x) {
            *cell += value;
            self.mark(x);
        }
    }

    /// Adds the part of `line` within row `top..top + 1`. Returns the cells
    /// visited.
    #[inline]
    pub(crate) fn line(&mut self, line: Line, top: f64) -> usize {
        let Line { a, b } = line;
        if a.y == b.y {
            return 0;
        }
        let (upper, lower, d) = if a.y < b.y {
            (a, b, 1.0)
        } else {
            (b, a, -1.0)
        };
        let lo = upper.y.max(top);
        let hi = lower.y.min(top + 1.0);
        if lo >= hi {
            return 0;
        }
        self.piece(line.x(lo), line.x(hi), (hi - lo) * d)
    }

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
                self.add_cover(first, h);
                return 1;
            }
            self.add_area(first, h * ((start + 1.0) - (xa + xb) * 0.5));
            self.add_cover(first + 1, h);
            return 2;
        }
        let last = end as i64;
        let s = h / (r - l);
        let width = i64::try_from(self.area.len()).unwrap_or(i64::MAX);
        let from = first.max(self.base);
        let to = last.min(self.base.saturating_add(width));
        for column in from..to {
            let c = column as f64;
            let x0 = l.max(c);
            let x1 = r.min(c + 1.0);
            let near = (c + 1.0) - x0;
            let far = (c + 1.0) - x1;
            let right = s * ((x0 - l) + (near * near - far * far) * 0.5);
            self.add_area(column, right);
        }
        self.add_cover(last, h);
        usize::try_from(to - from).unwrap_or(0) + 1
    }

    /// Adds the part of a monotone quadratic within row `top..top + 1`,
    /// integrated under the curve. Returns the cells visited.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "device coordinates are bounded by 10^12, within i64"
    )]
    pub(crate) fn curve(&mut self, curve: Quadratic, top: f64) -> usize {
        let (curve, d) = if curve.points[0].y < curve.points[2].y {
            (curve, 1.0)
        } else {
            let mut points = curve.points;
            points.reverse();
            (Quadratic { points }, -1.0)
        };
        let lo = curve.points[0].y.max(top);
        let hi = curve.points[2].y.min(top + 1.0);
        if lo >= hi {
            return 0;
        }
        let t0 = curve.parameter(lo, false);
        // Rounding can invert the parameters of nearly equal heights.
        let t1 = curve.parameter(hi, false).max(t0);
        let xa = curve.at(t0).x;
        let xb = curve.at(t1).x;
        let first = xa.min(xb).floor() as i64;
        let last = xa.max(xb).ceil() as i64;
        let width = i64::try_from(self.area.len()).unwrap_or(i64::MAX);
        let from = first.max(self.base);
        let to = last.min(self.base.saturating_add(width));
        let height = hi - lo;
        for column in from..to {
            let left = curve.cell(t0, t1, column as f64);
            self.add_area(column, d * (height - left));
        }
        self.add_cover(last, d * height);
        usize::try_from(to - from).unwrap_or(0) + 1
    }

    /// Adds signed area `value` to cell `column` without cover, and `cover`
    /// to every cell from `column + 1` on; for analysis-free producers.
    pub(crate) fn add_cell(&mut self, column: i64, value: f64, cover: f64) {
        self.add_area(column, value);
        if cover != 0.0 {
            self.add_cover(column + 1, cover);
        }
    }

    /// The winding integral of cells `from..to` left to right through
    /// `sink`, folded by `rule` and quantized; clears every touched cell of
    /// the range. Returns the touched cells.
    pub(crate) fn emit(
        &mut self,
        (from, to): (usize, usize),
        rule: FillRule,
        sink: &mut impl Sink,
    ) -> usize {
        let base = usize::try_from(self.base).unwrap_or(0);
        let to = to.min(self.end()).saturating_sub(base);
        let from = from.saturating_sub(base);
        if from >= to {
            return 0;
        }
        let mut cover = 0.0;
        let mut position = from;
        let mut count = 0;
        let first_word = from / 64;
        let last_word = (to - 1) / 64;
        for word in first_word..=last_word {
            let mut bits = std::mem::take(&mut self.touched[word]);
            while bits != 0 {
                let x = word * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if x < from || x >= to {
                    // Outside the range only the prefix of cells in it
                    // can matter: cover left of it, nothing right of it.
                    let carried = std::mem::take(&mut self.cover[x]);
                    self.area[x] = 0.0;
                    if x < from {
                        cover += carried;
                    }
                    continue;
                }
                if position < x {
                    sink.run(base + position, base + x, fold(cover, rule));
                }
                cover += std::mem::take(&mut self.cover[x]);
                let value = std::mem::take(&mut self.area[x]) + cover;
                sink.run(base + x, base + x + 1, fold(value, rule));
                position = x + 1;
                count += 1;
            }
        }
        if position < to {
            sink.run(base + position, base + to, fold(cover, rule));
        }
        count
    }

    /// The prefix integral at every touched cell, in order, for analysis;
    /// `visit` receives the column, its area and the cover up to and
    /// including it. Clears the touched cells of `from..to`.
    pub(crate) fn drain(&mut self, (from, to): (usize, usize), mut visit: impl FnMut(usize, f64, f64)) {
        let base = usize::try_from(self.base).unwrap_or(0);
        let to = to.min(self.end()).saturating_sub(base);
        let from = from.saturating_sub(base);
        if from >= to {
            return;
        }
        let mut cover = 0.0;
        for word in from / 64..=(to - 1) / 64 {
            let mut bits = std::mem::take(&mut self.touched[word]);
            while bits != 0 {
                let x = word * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                cover += std::mem::take(&mut self.cover[x]);
                visit(base + x, std::mem::take(&mut self.area[x]), cover);
            }
        }
    }

    /// Clears every cell; used after an interrupted analysis.
    pub(crate) fn reset(&mut self) {
        self.area.fill(0.0);
        self.cover.fill(0.0);
        self.touched.fill(0);
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
    use crate::Point;

    /// Merges runs of equal value.
    struct Collect(Vec<(usize, usize, u8)>);
    impl Sink for Collect {
        fn run(&mut self, x0: usize, x1: usize, value: u8) {
            if let Some(last) = self.0.last_mut()
                && last.1 == x0
                && last.2 == value
            {
                last.1 = x1;
            } else {
                self.0.push((x0, x1, value));
            }
        }
    }

    fn row(width: usize) -> Row {
        Row::new(width, &mut Meter::new(usize::MAX, "budget")).unwrap()
    }

    #[test]
    fn a_rectangle_row_has_exact_edges_and_a_full_interior() {
        let mut r = row(8);
        let left = Line {
            a: Point::new(1.25, 0.0),
            b: Point::new(1.25, 1.0),
        };
        let right = Line {
            a: Point::new(5.5, 1.0),
            b: Point::new(5.5, 0.0),
        };
        r.line(left, 0.0);
        r.line(right, 0.0);
        let mut out = Collect(Vec::new());
        r.emit((0, 8), FillRule::NonZero, &mut out);
        assert_eq!(
            out.0,
            [
                (0, 1, 0),
                (1, 2, 191),
                (2, 5, 255),
                (5, 6, 128),
                (6, 8, 0)
            ]
        );
        assert!(r.touched.iter().all(|&w| w == 0));
        assert!(r.area.iter().chain(&r.cover).all(|&v| v == 0.0));
    }

    #[test]
    fn slanted_pieces_split_their_area_across_columns() {
        // A triangle from (0, 0) to (4, 1) and back down the left edge.
        let mut r = row(6);
        r.line(
            Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(4.0, 1.0),
            },
            0.0,
        );
        r.line(
            Line {
                a: Point::new(0.0, 1.0),
                b: Point::new(0.0, 0.0),
            },
            0.0,
        );
        r.line(
            Line {
                a: Point::new(4.0, 1.0),
                b: Point::new(0.0, 1.0),
            },
            0.0,
        );
        let mut sums = Vec::new();
        r.drain((0, 6), |x, area, cover| sums.push((x, area + cover)));
        let total: f64 = sums.iter().map(|&(_, v)| v.abs()).sum();
        assert!((total - 2.0).abs() < 1e-12, "{sums:?}");
        // Column 0 holds 1 - 1/8 of its area below the hypotenuse.
        assert!((sums[0].1.abs() - 0.875).abs() < 1e-12);
    }

    #[test]
    fn geometry_left_of_the_page_carries_and_right_of_the_row_drops() {
        let mut r = row(4);
        r.line(
            Line {
                a: Point::new(-10.0, 0.0),
                b: Point::new(-9.0, 1.0),
            },
            0.0,
        );
        r.line(
            Line {
                a: Point::new(20.0, 1.0),
                b: Point::new(20.0, 0.0),
            },
            0.0,
        );
        let mut out = Collect(Vec::new());
        r.emit((0, 4), FillRule::NonZero, &mut out);
        assert_eq!(out.0, [(0, 4, 255)]);
    }

    #[test]
    fn even_odd_folds_windings_of_two() {
        assert_eq!(fold(2.0, FillRule::EvenOdd), 0);
        assert_eq!(fold(-1.0, FillRule::EvenOdd), 255);
        assert_eq!(fold(1.5, FillRule::EvenOdd), 128);
        assert_eq!(fold(2.0, FillRule::NonZero), 255);
    }
}
