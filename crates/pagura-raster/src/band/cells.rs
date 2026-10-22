//! The coverage kernel's cells: one strip of a band, at most [`STRIP`] rows
//! of a mark's columns, with a touched bitmap per row.
//!
//! Every item (a straight edge or a monotone quadratic) adds to the rows of
//! the window it crosses, item by item, with the row's arithmetic
//! ([`Accumulate`]), so each cell receives its additions in the items' own
//! order whatever the band height or region. Each row then leaves left to
//! right: a touched cell folds its winding integral (its area plus the
//! cover carried from the left) into a partial cell, and the cells between
//! touched ones take the carried cover once, as one run.

use super::curve::Quadratic;
use super::edges::Line;
use super::row::{Accumulate, Sink, fold};
use crate::FillRule;
use pagura_core::{Error, Meter, Result};

/// Rows per strip: strips lie on a global grid of this many rows, and a
/// band is rasterized strip by strip, so a window never holds more rows.
pub(crate) const STRIP: u32 = 16;

/// The widest window that still gets whole strips: wider ones take fewer
/// rows at a time, down to one, so that storage stays near a strip of this
/// width, or one row of the region.
const WIDE: usize = 4096;

/// The most rows of one window: a mark without strip lists (a glyph) takes
/// up to this many rows of a band at once.
pub(crate) const WINDOW: u32 = 32;

/// Strip-sized cells, reused for every mark and band of a render.
#[derive(Debug, Default)]
pub(crate) struct Cells {
    area: Vec<f64>,
    cover: Vec<f64>,
    touched: Vec<u64>,
    /// One row's folded touched cells, handed on together.
    bytes: Vec<u8>,
    /// The window: device column of its first cell, its width, bitmap words
    /// per row, first device row and row count.
    base: i64,
    width: usize,
    words: usize,
    y0: u32,
    rows: usize,
    /// Window rows (bits from the first) that items skip.
    skip: u32,
}

impl Cells {
    /// Cells for windows of up to [`STRIP`] rows of columns `0..width`,
    /// charged to `bytes`.
    pub(crate) fn new(width: usize, bytes: &mut Meter) -> Result<Self> {
        let (cells, words) = Self::sizes(width)?;
        let mut storage = Self::default();
        bytes.reserve(&mut storage.area, cells)?;
        bytes.reserve(&mut storage.cover, cells)?;
        bytes.reserve(&mut storage.touched, words)?;
        bytes.reserve(&mut storage.bytes, width)?;
        storage.area.resize(cells, 0.0);
        storage.cover.resize(cells, 0.0);
        storage.touched.resize(words, 0);
        storage.bytes.resize(width, 0);
        Ok(storage)
    }

    /// Charges `bytes` with what [`Self::new`] reserves.
    pub(crate) fn charge(width: usize, bytes: &mut Meter) -> Result<()> {
        let (cells, words) = Self::sizes(width)?;
        bytes.charge_items::<f64>(cells)?;
        bytes.charge_items::<f64>(cells)?;
        bytes.charge_items::<u64>(words)?;
        bytes.charge_items::<u8>(width)
    }

    /// Cells and bitmap words for windows up to `width` wide: a strip of
    /// windows up to [`WIDE`] columns, or one row of `width`.
    fn sizes(width: usize) -> Result<(usize, usize)> {
        let rows = STRIP as usize;
        let narrow = width.min(WIDE);
        let cells = (narrow * rows).max(width);
        let words = (narrow / 64 + 1) * rows;
        let words = words.max(width / 64 + 1);
        if cells.checked_mul(2 * size_of::<f64>()).is_none() {
            return Err(Error::limit("raster cells exceed usize"));
        }
        Ok((cells, words))
    }

    /// The rows of a window `width` wide the storage holds at once, up to
    /// [`WINDOW`]: whole strips up to [`WIDE`] columns, fewer beyond, at
    /// least one.
    pub(crate) fn rows_fit(&self, width: usize) -> u32 {
        let words = width / 64 + 1;
        let fit = (self.area.len() / width.max(1)).min(self.touched.len() / words);
        u32::try_from(fit).unwrap_or(WINDOW).clamp(1, WINDOW)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        (self.area.capacity() + self.cover.capacity())
            .saturating_mul(size_of::<f64>())
            .saturating_add(self.touched.capacity().saturating_mul(size_of::<u64>()))
            .saturating_add(self.bytes.capacity())
    }

    /// Opens a window of columns `from..to` and device rows `rows`, at most
    /// [`WINDOW`] of them; `false` when it does not fit the storage, which
    /// preparation rules out.
    pub(crate) fn open(&mut self, (from, to): (usize, usize), rows: std::ops::Range<u32>) -> bool {
        let width = to.saturating_sub(from);
        let words = width / 64 + 1;
        let count = rows.len();
        if count > WINDOW as usize
            || width > self.bytes.len()
            || width.saturating_mul(count) > self.area.len()
            || words.saturating_mul(count) > self.touched.len()
        {
            return false;
        }
        self.base = i64::try_from(from).unwrap_or(i64::MAX);
        self.width = width;
        self.words = words;
        self.y0 = rows.start;
        self.rows = count;
        self.skip = 0;
        true
    }

    /// Makes items skip the window rows whose bits `mask` sets, from the
    /// first; rows resolved at preparation take their sides instead.
    pub(crate) fn skip(&mut self, mask: u32) {
        self.skip = mask;
    }

    /// Whether window row `r` is skipped.
    #[inline]
    fn skipped(&self, r: usize) -> bool {
        u32::try_from(r)
            .ok()
            .and_then(|r| self.skip.checked_shr(r))
            .is_some_and(|bits| bits & 1 != 0)
    }

    /// Whether an item whose leftmost point lies at `x` can change a cell of
    /// the window: nothing right of it can.
    #[inline]
    pub(crate) fn reaches(&self, x: f64) -> bool {
        #[expect(
            clippy::cast_precision_loss,
            reason = "window columns are far below 2^52"
        )]
        let end = (self.base as f64) + (self.width as f64);
        x < end
    }

    /// The window's rows an item from height `ya` to `yb` crosses.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the window's rows, at most STRIP"
    )]
    #[inline]
    fn span(&self, ya: f64, yb: f64) -> std::ops::Range<usize> {
        let first = f64::from(self.y0);
        let end = first + self.rows as f64;
        let lo = ya.min(yb).floor().max(first);
        let hi = ya.max(yb).ceil().min(end);
        if lo < hi {
            (lo - first) as usize..(hi - first) as usize
        } else {
            0..0
        }
    }

    /// The cells and bitmap words of window row `r`, as a view whose
    /// slices the accumulation keeps in registers.
    #[inline]
    fn row(&mut self, r: usize) -> Row<'_> {
        let cells = r.wrapping_mul(self.width);
        let words = r.wrapping_mul(self.words);
        Row {
            base: self.base,
            area: self
                .area
                .get_mut(cells..cells.wrapping_add(self.width))
                .unwrap_or_default(),
            cover: self
                .cover
                .get_mut(cells..cells.wrapping_add(self.width))
                .unwrap_or_default(),
            touched: self
                .touched
                .get_mut(words..words.wrapping_add(self.words))
                .unwrap_or_default(),
        }
    }

    /// Adds a straight edge to the rows it crosses. Returns the cells and
    /// rows visited.
    #[inline]
    pub(crate) fn line(&mut self, line: Line) -> usize {
        let Line { a, b } = line;
        if a.y == b.y {
            return 0;
        }
        let (upper, lower, d) = if a.y < b.y { (a, b, 1.0) } else { (b, a, -1.0) };
        let mut visited = 0;
        // The crossing of a row's lower side is the next row's upper one:
        // computed once, the same value either way.
        let mut carried: Option<(f64, f64)> = None;
        for r in self.span(a.y, b.y) {
            if self.skipped(r) {
                continue;
            }
            let top = f64::from(self.y0) + f64::from(u32::try_from(r).unwrap_or(u32::MAX));
            let lo = upper.y.max(top);
            let hi = lower.y.min(top + 1.0);
            if lo >= hi {
                continue;
            }
            let xa = match carried {
                Some((y, x)) if y == lo => x,
                _ => line.x(lo),
            };
            let xb = line.x(hi);
            carried = Some((hi, xb));
            visited += 1 + self.row(r).piece(xa, xb, (hi - lo) * d);
        }
        visited
    }

    /// Adds a monotone quadratic to the rows it crosses. Returns the cells
    /// and rows visited.
    pub(crate) fn curve(&mut self, curve: Quadratic) -> usize {
        let mut visited = 0;
        for r in self.span(curve.points[0].y, curve.points[2].y) {
            if self.skipped(r) {
                continue;
            }
            let top = f64::from(self.y0) + f64::from(u32::try_from(r).unwrap_or(u32::MAX));
            visited += 1 + self.row(r).curve(curve, top);
        }
        visited
    }

    /// Writes window row `r` to `sink`, folded by `rule`, and clears its
    /// cells: consecutive touched cells together, and each run between them
    /// once with the cover carried into it. Returns the touched cells.
    pub(crate) fn emit(&mut self, r: usize, rule: FillRule, sink: &mut impl Sink) -> usize {
        let base = usize::try_from(self.base).unwrap_or(0);
        let cells = r.wrapping_mul(self.width);
        let words = r.wrapping_mul(self.words)..(r + 1).wrapping_mul(self.words);
        let mut cover = 0.0;
        let mut position = 0;
        let mut touched = 0;
        // Folded touched cells waiting from column `start`.
        let (mut start, mut pending) = (0, 0);
        for (w, word) in (0_usize..).zip(words) {
            let Some(slot) = self.touched.get_mut(word) else {
                break;
            };
            let mut bits = std::mem::take(slot);
            while bits != 0 {
                let x = w * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let i = cells + x;
                let (Some(a), Some(c)) = (self.area.get_mut(i), self.cover.get_mut(i)) else {
                    continue;
                };
                if position < x {
                    if let Some(values) = self.bytes.get(..pending) {
                        sink.cells(base + start, values);
                    }
                    pending = 0;
                    sink.run(base + position, base + x, fold(cover, rule));
                }
                cover += std::mem::take(c);
                let value = std::mem::take(a) + cover;
                if pending == 0 {
                    start = x;
                }
                if let Some(slot) = self.bytes.get_mut(pending) {
                    *slot = fold(value, rule);
                    pending += 1;
                }
                position = x + 1;
                touched += 1;
            }
        }
        if let Some(values) = self.bytes.get(..pending) {
            sink.cells(base + start, values);
        }
        if position < self.width {
            sink.run(base + position, base + self.width, fold(cover, rule));
        }
        touched
    }

    /// The window's first device row and row count.
    pub(crate) fn rows(&self) -> std::ops::Range<u32> {
        let count = u32::try_from(self.rows).unwrap_or(0);
        self.y0..self.y0 + count
    }
}

/// One row of a window's cells, for [`Accumulate`].
struct Row<'a> {
    base: i64,
    area: &'a mut [f64],
    cover: &'a mut [f64],
    touched: &'a mut [u64],
}

impl Row<'_> {
    #[inline]
    fn mark(&mut self, x: usize) {
        if let Some(word) = self.touched.get_mut(x / 64) {
            *word |= 1 << (x % 64);
        }
    }
}

impl Accumulate for Row<'_> {
    #[inline]
    fn base(&self) -> i64 {
        self.base
    }

    #[inline]
    fn width(&self) -> usize {
        self.area.len()
    }

    #[inline]
    fn area_at(&mut self, column: i64, value: f64) {
        if let Ok(x) = usize::try_from(column.wrapping_sub(self.base))
            && let Some(cell) = self.area.get_mut(x)
        {
            *cell += value;
            self.mark(x);
        }
    }

    #[inline]
    fn cover_at(&mut self, column: i64, value: f64) {
        let x = usize::try_from(column.wrapping_sub(self.base).max(0)).unwrap_or(usize::MAX);
        if let Some(cell) = self.cover.get_mut(x) {
            *cell += value;
            self.mark(x);
        }
    }
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

    fn cells(width: usize) -> Cells {
        Cells::new(width, &mut Meter::new(usize::MAX, "budget")).unwrap()
    }

    fn line(a: (f64, f64), b: (f64, f64)) -> Line {
        Line {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
        }
    }

    #[test]
    fn a_rectangle_row_has_exact_edges_and_a_full_interior() {
        let mut c = cells(8);
        assert!(c.open((0, 8), 0..1));
        c.line(line((1.25, 0.0), (1.25, 1.0)));
        c.line(line((5.5, 1.0), (5.5, 0.0)));
        let mut out = Collect(Vec::new());
        assert_eq!(c.emit(0, FillRule::NonZero, &mut out), 4);
        assert_eq!(
            out.0,
            [(0, 1, 0), (1, 2, 191), (2, 5, 255), (5, 6, 128), (6, 8, 0)]
        );
        assert!(c.touched.iter().all(|&w| w == 0));
        assert!(c.area.iter().chain(&c.cover).all(|&v| v == 0.0));
    }

    #[test]
    fn slanted_edges_split_their_area_across_rows_and_columns() {
        // A triangle from (0, 0) to (4, 2) and back up the left side.
        let mut c = cells(6);
        assert!(c.open((0, 6), 0..2));
        c.line(line((0.0, 0.0), (4.0, 2.0)));
        c.line(line((4.0, 2.0), (0.0, 2.0)));
        c.line(line((0.0, 2.0), (0.0, 0.0)));
        let mut total = 0_u32;
        for r in 0..2 {
            let mut out = Collect(Vec::new());
            c.emit(r, FillRule::NonZero, &mut out);
            total += out
                .0
                .iter()
                .map(|&(a, b, v)| u32::try_from(b - a).unwrap() * u32::from(v))
                .sum::<u32>();
        }
        // An area of 4 pixels, quantized per cell.
        assert!((i64::from(total) - 4 * 255).abs() <= 4, "{total}");
    }

    #[test]
    fn geometry_left_of_the_page_carries_and_right_of_the_window_drops() {
        let mut c = cells(4);
        assert!(c.open((0, 4), 3..4));
        c.line(line((-10.0, 3.0), (-9.0, 4.0)));
        c.line(line((20.0, 4.0), (20.0, 3.0)));
        let mut out = Collect(Vec::new());
        c.emit(0, FillRule::NonZero, &mut out);
        assert_eq!(out.0, [(0, 4, 255)]);
    }

    #[test]
    fn windows_beyond_the_storage_are_refused() {
        let mut c = cells(8);
        assert!(!c.open((0, 9), 0..STRIP));
        assert!(!c.open((0, 8), 0..STRIP + 1));
        assert!(c.open((0, 8), 0..STRIP));
    }
}
