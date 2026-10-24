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

use super::curve::Mono;
use super::edges::Line;
use super::row::{Accumulate, Sink, fold, piece_into, single};
use crate::{FillRule, Point};
use pagura_core::{Error, Meter, Result};

/// Rows per strip: strips lie on a global grid of this many rows, and a
/// band is rasterized strip by strip, so a window never holds more rows.
pub(crate) const STRIP: u32 = 16;

/// The widest window that still gets whole strips: wider ones take fewer
/// rows at a time, down to one, so that storage stays near a strip of this
/// width, or one row of the region.
pub(crate) const WIDE: usize = 4096;

/// The most rows of one window: a mark without strip lists (a glyph) takes
/// up to this many rows of a band at once.
pub(crate) const WINDOW: u32 = 32;

/// The widest window emitted densely: every cell of each row in one pass,
/// without a touched bitmap. Narrow windows (glyphs, small marks) touch a
/// large share of their cells, so the bitmap costs more than it skips.
pub(crate) const DENSE: usize = 256;

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
    /// Whether the window keeps no touched bitmap and emits every cell.
    dense: bool,
    /// Used only by the preparation-only measured entry points. The
    /// ordinary kernel specializes measurement away at compile time.
    rounding: Rounding,
}

#[derive(Debug, Default)]
struct Rounding {
    absolute: f64,
    primitive: f64,
    additions: usize,
}

impl Rounding {
    fn add(&mut self, before: f64, value: f64, sum: f64) {
        if value == 0.0 {
            return;
        }
        // TwoSum recovers this actual addition's exact rounding tail.
        // Many off-window fragments can add exactly: their count alone
        // must not impose an irreducible local precision floor.
        let back = sum - before;
        let tail = (before - (sum - back)) + (value - back);
        self.primitive += tail.abs();
        self.absolute += value.abs();
        self.additions = self.additions.saturating_add(2);
    }

    fn primitive(&mut self, error: f64) {
        if error == 0.0 {
            return;
        }
        self.primitive += error;
        self.additions = self.additions.saturating_add(1);
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "inflated before use; enormous counts refuse precision"
    )]
    fn bound(&self, width: usize) -> f64 {
        let positive = (self.additions as f64 + 4.0) * (1.0 + 4.0 * f64::EPSILON);
        let prefix = (width as f64 + 4.0) * (1.0 + 4.0 * f64::EPSILON);
        let (positive, prefix) = (positive * f64::EPSILON, prefix * f64::EPSILON);
        if positive >= 0.25 || prefix >= 0.25 {
            return f64::INFINITY;
        }
        // Outward sums of primitive errors, exact addition tails and
        // absolute inputs. Stored cell magnitudes total at most S+E. Only
        // emission remains unmeasured: at most width cover additions and
        // one area addition per pixel, bounded by gamma_(width+4).
        let error = self.primitive / (1.0 - positive);
        let absolute = self.absolute / (1.0 - positive);
        (error
            + (8.0 * prefix / (1.0 - prefix)) * (absolute + error)
            + 64.0 * f64::EPSILON * absolute)
            * (1.0 + 8.0 * f64::EPSILON)
            + f64::MIN_POSITIVE
    }
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
        self.dense = false;
        self.rounding = Rounding::default();
        true
    }

    /// Opens a window as [`Self::open`] does for execution: one at most
    /// [`DENSE`] columns wide keeps no touched bitmap and [`Self::emit`]
    /// visits every cell of its rows. The cells and their arithmetic are the
    /// same, so the emitted bytes are too; inspection keeps [`Self::open`],
    /// whose touched cells classify clip boundaries.
    pub(crate) fn open_dense(&mut self, scan: (usize, usize), rows: std::ops::Range<u32>) -> bool {
        let opened = self.open(scan, rows);
        self.dense = opened && self.width <= DENSE;
        opened
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
    fn row<const MEASURE: bool>(&mut self, r: usize) -> Row<'_, MEASURE> {
        let cells = r.wrapping_mul(self.width);
        // A dense window marks nothing: an empty bitmap drops every mark.
        let words = if self.dense {
            0..0
        } else {
            let first = r.wrapping_mul(self.words);
            first..first.wrapping_add(self.words)
        };
        Row {
            rounding: &mut self.rounding,
            base: self.base,
            area: self
                .area
                .get_mut(cells..cells.wrapping_add(self.width))
                .unwrap_or_default(),
            cover: self
                .cover
                .get_mut(cells..cells.wrapping_add(self.width))
                .unwrap_or_default(),
            touched: self.touched.get_mut(words).unwrap_or_default(),
        }
    }

    /// Adds a piece to window row `r`, as [`Row::piece`] does. A dense
    /// execution window adds a piece inside one cell, with a cell after it,
    /// at its flat index: the same operations, without the row's view.
    #[expect(
        clippy::inline_always,
        reason = "every flattened glyph edge takes this path inside the edge loop"
    )]
    #[inline(always)]
    fn piece<const MEASURE: bool>(&mut self, r: usize, xa: f64, xb: f64, h: f64) -> usize {
        if !MEASURE && self.dense {
            let (l, right) = if xa < xb { (xa, xb) } else { (xb, xa) };
            let start = l.floor();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "device coordinates are bounded by 10^12, within i64"
            )]
            let first = start as i64;
            if right.ceil() == start + 1.0
                && let Ok(x) = usize::try_from(first.wrapping_sub(self.base))
                && x.wrapping_add(1) < self.width
            {
                let k = r.wrapping_mul(self.width).wrapping_add(x);
                if let (Some(area), Some(cover)) =
                    (self.area.get_mut(k), self.cover.get_mut(k.wrapping_add(1)))
                {
                    *area += single(xa, xb, h, start);
                    *cover += h;
                    return 2;
                }
            }
        }
        self.row::<MEASURE>(r).piece(xa, xb, h)
    }

    /// Adds a straight edge to the rows it crosses. Returns the cells and
    /// rows visited.
    #[inline]
    pub(crate) fn line(&mut self, line: Line) -> usize {
        self.line_with::<false>(line)
    }

    pub(crate) fn line_measured(&mut self, line: Line) -> usize {
        self.line_with::<true>(line)
    }

    pub(crate) fn arithmetic_error(&self) -> f64 {
        self.rounding.bound(self.width)
    }

    #[inline]
    fn line_with<const MEASURE: bool>(&mut self, line: Line) -> usize {
        let Line { a, b } = line;
        if a.y == b.y {
            return 0;
        }
        let (upper, lower, d) = if a.y < b.y { (a, b, 1.0) } else { (b, a, -1.0) };
        // An edge within one row is one piece between its endpoints, which
        // interpolation returns exactly: most flattened curve edges.
        let top = upper.y.floor();
        if lower.y <= top + 1.0 {
            let first = f64::from(self.y0);
            if top < first || top >= first + self.rows as f64 {
                return 0;
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a row of the window, checked above"
            )]
            let r = (top - first) as usize;
            if self.skipped(r) {
                return 0;
            }
            return 1 + self.piece::<MEASURE>(r, upper.x, lower.x, (lower.y - upper.y) * d);
        }
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
                _ => line.kernel_x(lo),
            };
            let xb = line.kernel_x(hi);
            carried = Some((hi, xb));
            visited += 1 + self.piece::<MEASURE>(r, xa, xb, (hi - lo) * d);
        }
        visited
    }

    /// Adds monotone quadratic `curve` moved by `origin` to the rows it
    /// crosses, integrated under the curve: per row one root for the row's
    /// lower side (its upper one is the row above's) and one per column
    /// side crossed, and each cell's area from the closed-form integral of
    /// x dy between them. Returns the cells and rows visited.
    pub(crate) fn curve(&mut self, curve: &Mono, origin: Point) -> usize {
        self.curve_with::<false>(curve, origin)
    }

    pub(crate) fn curve_measured(&mut self, curve: &Mono, origin: Point) -> usize {
        self.curve_with::<true>(curve, origin)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded device coordinates are within i64"
    )]
    fn curve_with<const MEASURE: bool>(&mut self, curve: &Mono, origin: Point) -> usize {
        let m = curve;
        let (x0, y0) = (m.x0 + origin.x, m.y0 + origin.y);
        let (x1, y1) = (m.x1 + origin.x, m.y1 + origin.y);
        let increasing = x1 >= x0;
        let mut visited = 0;
        let mut carried = None;
        for r in self.span(y0, y1) {
            let top = f64::from(self.y0) + f64::from(u32::try_from(r).unwrap_or(u32::MAX));
            let lo = y0.max(top);
            let hi = y1.min(top + 1.0);
            if lo >= hi || self.skipped(r) {
                carried = None;
                continue;
            }
            let ta = if lo == y0 {
                0.0
            } else {
                carried.unwrap_or_else(|| m.ty(lo - y0))
            };
            let tb = if hi == y1 { 1.0 } else { m.ty(hi - y0).max(ta) };
            carried = Some(tb);
            let xa = if ta == 0.0 { x0 } else { x0 + m.dx(ta) };
            let xb = if tb == 1.0 { x1 } else { x0 + m.dx(tb) };
            let height = (hi - lo) * m.direction;
            let (l, right) = if xa < xb { (xa, xb) } else { (xb, xa) };
            let start = l.floor();
            let end = right.ceil();
            let first = start as i64;
            let mut row = self.row::<MEASURE>(r);
            if MEASURE {
                // Approximate row roots replace the exact truncation only
                // inside horizontal residual strips. Endpoint x rounding
                // can also change floor/ceil and skip an entire column,
                // even when no column root is evaluated. Each quadratic
                // coordinate has at most two monotone intervals; eight
                // includes both ends and the opposite-strip cross terms.
                let ey = m.residual(ta, lo - y0, false) + m.residual(tb, hi - y0, false);
                let ex = m.endpoint_error([ta, tb], [x0, x1]);
                row.primitive(8.0 * (ex + ey + ex * ey));
            }
            visited += 2;
            if end <= start + 1.0 {
                if end == start {
                    row.cover_at(first, height);
                    continue;
                }
                let area = (start + 1.0 - x0) * (hi - lo) - (m.h(tb) - m.h(ta));
                row.area_at(first, area * m.direction);
                if MEASURE {
                    row.primitive(m.area_error([ta, tb], start + 1.0 - x0, hi - lo));
                }
                row.cover_at(first + 1, height);
                continue;
            }
            let last = end as i64;
            let total = hi - lo;
            let (mut t, mut yt, mut ht) = (ta, m.dy(ta), m.h(ta));
            let mut done = 0.0;
            let mut done_error = 0.0;
            let columns = last.wrapping_sub(first);
            for k in 0..columns {
                let c = if increasing {
                    first.wrapping_add(k)
                } else {
                    last.wrapping_sub(1).wrapping_sub(k)
                };
                let tn = if k + 1 == columns {
                    tb
                } else {
                    let side = if increasing { c.wrapping_add(1) } else { c };
                    m.tx(side as f64 - x0).clamp(t, tb)
                };
                if MEASURE && k + 1 != columns {
                    let side = if increasing { c.wrapping_add(1) } else { c };
                    let ex = m.residual(tn, side as f64 - x0, true);
                    let ey = m.residual(ta, lo - y0, false) + m.residual(tb, hi - y0, false);
                    // Moving a column transition changes coverage only in
                    // its vertical residual strip, including clamped roots.
                    row.primitive(8.0 * ex * (1.0 + ey));
                }
                let (yn, hn) = (m.dy(tn), m.h(tn));
                let dy = yn - yt;
                // The cell's part right of the curve where the curve is in
                // it, and its full width where the curve is left of it.
                let inside = (c.wrapping_add(1) as f64 - x0) * dy - (hn - ht);
                let left = if increasing { done } else { total - done - dy };
                row.area_at(c, (left + inside) * m.direction);
                if MEASURE {
                    let dy_error = m.height_error([t, tn]);
                    let left_error = done_error
                        + if increasing {
                            0.0
                        } else {
                            dy_error + 8.0 * f64::EPSILON * (total.abs() + done.abs() + dy.abs())
                        };
                    row.primitive(
                        left_error
                            + (c.wrapping_add(1) as f64 - x0).abs() * dy_error
                            + m.area_error([t, tn], c.wrapping_add(1) as f64 - x0, dy)
                            + 32.0 * f64::EPSILON * (left.abs() + inside.abs()),
                    );
                    done_error += dy_error + 8.0 * f64::EPSILON * (done.abs() + dy.abs());
                }
                done += dy;
                (t, yt, ht) = (tn, yn, hn);
                visited += 1;
            }
            row.cover_at(last, height);
        }
        visited
    }

    /// Writes window row `r` to `sink`, folded by `rule`, and clears its
    /// cells: consecutive touched cells together, and each run between them
    /// once with the cover carried into it. Returns the touched cells.
    pub(crate) fn emit(&mut self, r: usize, rule: FillRule, sink: &mut impl Sink) -> usize {
        self.emit_with(r, sink, |_, _, value, _| fold(value, rule))
    }

    /// The identical prefix sum before quantization, for preparation's
    /// coverage-byte certificate. Inspection clears the cells as emission
    /// does, and introduces no retained execution representation.
    pub(crate) fn inspect(&mut self, r: usize, mut visit: impl FnMut(usize, usize, f64)) -> usize {
        self.inspect_parts(r, |x0, x1, value, _| visit(x0, x1, value))
    }

    /// The same inspection with the distinction between touched boundary
    /// cells and untouched carried-cover runs. Clip classification needs
    /// geometry support even when its provisional coverage byte is zero.
    pub(crate) fn inspect_parts(
        &mut self,
        r: usize,
        mut visit: impl FnMut(usize, usize, f64, bool),
    ) -> usize {
        struct Ignore;
        impl Sink for Ignore {
            fn run(&mut self, _: usize, _: usize, _: u8) {}
        }
        self.emit_with(r, &mut Ignore, |x0, x1, value, touched| {
            visit(x0, x1, value, touched);
            0
        })
    }

    fn emit_with(
        &mut self,
        r: usize,
        sink: &mut impl Sink,
        mut map: impl FnMut(usize, usize, f64, bool) -> u8,
    ) -> usize {
        let base = usize::try_from(self.base).unwrap_or(0);
        let cells = r.wrapping_mul(self.width);
        if self.dense {
            return self.emit_dense(cells, base, sink, map);
        }
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
                    let value = map(base + position, base + x, cover, false);
                    sink.run(base + position, base + x, value);
                }
                cover += std::mem::take(c);
                let value = std::mem::take(a) + cover;
                if pending == 0 {
                    start = x;
                }
                if let Some(slot) = self.bytes.get_mut(pending) {
                    *slot = map(base + x, base + x + 1, value, true);
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
            let value = map(base + position, base + self.width, cover, false);
            sink.run(base + position, base + self.width, value);
        }
        touched
    }

    /// [`Self::emit_with`] for a dense window: every cell of the row from the
    /// left, with the same prefix sum, handed on as one run of bytes. Cells
    /// that received nothing count as untouched.
    fn emit_dense(
        &mut self,
        cells: usize,
        base: usize,
        sink: &mut impl Sink,
        mut map: impl FnMut(usize, usize, f64, bool) -> u8,
    ) -> usize {
        let end = cells.wrapping_add(self.width);
        let (Some(area), Some(cover), Some(bytes)) = (
            self.area.get_mut(cells..end),
            self.cover.get_mut(cells..end),
            self.bytes.get_mut(..self.width),
        ) else {
            return 0;
        };
        let mut sum = 0.0;
        let mut touched = 0;
        for (x, ((a, c), byte)) in area
            .iter_mut()
            .zip(cover.iter_mut())
            .zip(bytes.iter_mut())
            .enumerate()
        {
            let (part, step) = (std::mem::take(a), std::mem::take(c));
            let hit = part != 0.0 || step != 0.0;
            touched += usize::from(hit);
            sum += step;
            *byte = map(base + x, base + x + 1, part + sum, hit);
        }
        sink.cells(base, bytes);
        touched
    }

    /// The window's first device row and row count.
    pub(crate) fn rows(&self) -> std::ops::Range<u32> {
        let count = u32::try_from(self.rows).unwrap_or(0);
        self.y0..self.y0 + count
    }
}

/// One row of a window's cells, for [`Accumulate`].
struct Row<'a, const MEASURE: bool> {
    base: i64,
    area: &'a mut [f64],
    cover: &'a mut [f64],
    touched: &'a mut [u64],
    rounding: &'a mut Rounding,
}

impl<const MEASURE: bool> Row<'_, MEASURE> {
    #[inline]
    fn primitive(&mut self, error: f64) {
        if MEASURE {
            self.rounding.primitive(error);
        }
    }

    #[inline]
    fn mark(&mut self, x: usize) {
        if let Some(word) = self.touched.get_mut(x / 64) {
            *word |= 1 << (x % 64);
        }
    }
}

impl<const MEASURE: bool> Accumulate for Row<'_, MEASURE> {
    #[inline]
    fn base(&self) -> i64 {
        self.base
    }

    #[inline]
    fn width(&self) -> usize {
        self.area.len()
    }

    #[inline]
    fn area_error(&mut self, column: i64, error: f64) {
        if let Ok(x) = usize::try_from(column.wrapping_sub(self.base))
            && x < self.area.len()
        {
            self.primitive(error);
        }
    }

    #[inline]
    fn area_at(&mut self, column: i64, value: f64) {
        if let Ok(x) = usize::try_from(column.wrapping_sub(self.base))
            && let Some(cell) = self.area.get_mut(x)
        {
            let before = *cell;
            *cell += value;
            if MEASURE {
                self.rounding.add(before, value, *cell);
            }
            self.mark(x);
        }
    }

    #[inline]
    fn cover_at(&mut self, column: i64, value: f64) {
        let x = usize::try_from(column.wrapping_sub(self.base).max(0)).unwrap_or(usize::MAX);
        if let Some(cell) = self.cover.get_mut(x) {
            let before = *cell;
            *cell += value;
            if MEASURE {
                self.rounding.add(before, value, *cell);
            }
            self.mark(x);
        }
    }

    /// [`piece_into`] with its most common case, a piece inside one cell
    /// with a cell after it in the row, done in place: the same operations
    /// in the same order, located once.
    #[inline]
    fn piece(&mut self, xa: f64, xb: f64, h: f64) -> usize {
        let (l, r) = if xa < xb { (xa, xb) } else { (xb, xa) };
        let start = l.floor();
        let end = r.ceil();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "device coordinates are bounded by 10^12, within i64"
        )]
        let first = start as i64;
        if end == start + 1.0
            && let Ok(x) = usize::try_from(first.wrapping_sub(self.base))
            && x.wrapping_add(1) < self.area.len()
            && x.wrapping_add(1) < self.cover.len()
        {
            let value = single(xa, xb, h, start);
            let before = self.area[x];
            self.area[x] += value;
            if MEASURE {
                self.rounding.add(before, value, self.area[x]);
                self.primitive(
                    64.0 * f64::EPSILON
                        * h.abs()
                        * (xa.abs() + xb.abs() + (start + 1.0).abs() + 1.0),
                );
            }
            self.mark(x);
            let before = self.cover[x + 1];
            self.cover[x + 1] += h;
            if MEASURE {
                self.rounding.add(before, h, self.cover[x + 1]);
            }
            self.mark(x + 1);
            return 2;
        }
        piece_into(self, xa, xb, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A row's bytes, unemitted pixels zero.
    struct Bytes(Vec<u8>);
    impl Sink for Bytes {
        fn run(&mut self, x0: usize, x1: usize, value: u8) {
            if let Some(pixels) = self.0.get_mut(x0..x1) {
                pixels.fill(value);
            }
        }
    }

    #[test]
    fn dense_windows_emit_the_bytes_of_marked_windows() {
        let mut state = 0x853c_49e6_748f_ea9b_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            #[expect(
                clippy::cast_precision_loss,
                reason = "53 random bits map to a uniform fraction"
            )]
            let unit = (state >> 11) as f64 / (1u64 << 53) as f64;
            unit
        };
        let (width, rows) = (40_usize, 0..STRIP);
        let (mut marked, mut dense) = (cells(width), cells(width));
        for case in 0..200 {
            assert!(marked.open((3, width), rows.clone()));
            assert!(dense.open_dense((3, width), rows.clone()));
            // Short flattened-curve edges, long ones across rows and
            // columns, and some reaching outside the window.
            let count = 1 + case % 24;
            let mut point = (3.0 + 34.0 * next(), 16.0 * next());
            for _ in 0..count {
                let reach = if next() < 0.7 { 0.6 } else { 20.0 };
                let to = (
                    point.0 + reach * (next() - 0.5),
                    point.1 + reach * (next() - 0.5),
                );
                marked.line(line(point, to));
                dense.line(line(point, to));
                point = to;
            }
            for r in 0..rows.len() {
                let (mut a, mut b) = (Bytes(vec![0; width]), Bytes(vec![0; width]));
                marked.emit(r, FillRule::NonZero, &mut a);
                dense.emit(r, FillRule::NonZero, &mut b);
                assert_eq!(a.0, b.0, "{case} row {r}");
            }
        }
    }

    #[test]
    fn exact_fragment_additions_do_not_create_a_count_precision_floor() {
        let mut measured = cells(1);
        assert!(measured.open((0, 1), 0..1));
        // Every nonzero height is an exact binary fraction. Excluding
        // zero additions alone cannot tighten the old global gamma bound.
        for index in 0..8_192_u32 {
            let a = f64::from(index) / 16_384.0;
            let b = f64::from(index + 1) / 16_384.0;
            measured.line_measured(line((-2.0, a), (-2.0, b)));
        }
        let error = measured.arithmetic_error();
        assert_eq!(measured.rounding.primitive, 0.0);
        assert_eq!(measured.rounding.additions, 16_384);
        assert!(error < 1e-12, "exact fragment allowance {error}");
        measured.inspect(0, |_, _, value| {
            assert_eq!(value, 0.5);
            assert_eq!(crate::quantize_unit(value - error), 128);
            assert_eq!(crate::quantize_unit(value + error), 128);
        });

        // A small term lost between cancelling large values is an actual
        // residual and must survive in the measured interval.
        let mut measured = cells(1);
        assert!(measured.open((0, 1), 0..1));
        {
            let mut row = measured.row::<true>(0);
            row.area_at(0, 2.0_f64.powi(40));
            row.area_at(0, 2.0_f64.powi(-20));
            row.area_at(0, -2.0_f64.powi(40));
        }
        let error = measured.arithmetic_error();
        assert_eq!(measured.rounding.primitive, 2.0_f64.powi(-20));
        measured.inspect(0, |_, _, value| {
            assert!((value - 2.0_f64.powi(-20)).abs() <= error);
        });
    }

    #[test]
    fn measured_quadratics_enclose_flat_roots_and_discarded_column_prefixes() {
        use crate::band::curve::Quadratic;
        for decreasing in [false, true] {
            for (width, offset, first) in [(32.0, 0.0, 0), (32.0, -16.0, 8)] {
                let (a, b, c) = if decreasing {
                    (
                        Point::new(width, 0.0),
                        Point::new(0.0, 0.5),
                        Point::new(0.0, 1.0),
                    )
                } else {
                    (
                        Point::new(0.0, 0.0),
                        Point::new(0.0, 0.5),
                        Point::new(width, 1.0),
                    )
                };
                let m = Mono::new(Quadratic { points: [a, b, c] });
                let mut measured = cells(40);
                assert!(measured.open((first, first + 8), 0..1));
                measured.curve_measured(&m, Point::new(offset, 0.0));
                let error = measured.arithmetic_error();
                measured.inspect(0, |x0, x1, area| {
                    for column in x0..x1 {
                        let c = f64::from(u32::try_from(column).unwrap()) - offset;
                        let a = (c / width).clamp(0.0, 1.0).sqrt();
                        let b = ((c + 1.0) / width).clamp(0.0, 1.0).sqrt();
                        // x=w*t², y=t, or its t-reflection. Integrate the
                        // pixel-local width with factored cubic difference.
                        let exact = a + (b - a) * (c + 1.0 - width * (b * b + b * a + a * a) / 3.0);
                        assert!(
                            (area - exact).abs() <= error + 16.0 * f64::EPSILON,
                            "descending{decreasing}, column{column}: {area} ±{error}, exact{exact}"
                        );
                    }
                });
            }
        }
        for (x, expected) in [(0.0, 1.0), (0.375, 0.625), (-2.0, 1.0)] {
            let m = Mono::new(Quadratic {
                points: [Point::new(x, 0.0), Point::new(x, 0.5), Point::new(x, 1.0)],
            });
            let mut measured = cells(1);
            assert!(measured.open((0, 1), 0..1));
            measured.curve_measured(&m, Point::default());
            let error = measured.arithmetic_error();
            measured.inspect(0, |_, _, area| assert!((area - expected).abs() <= error));
        }
        // At the horizontal tangent y=t², the row root has arbitrarily
        // small derivative. Its residual strip still bounds coverage.
        let m = Mono::new(Quadratic {
            points: [Point::default(), Point::new(0.5, 0.0), Point::new(1.0, 1.0)],
        });
        for height in [1.0, 2.0_f64.powi(-20), 2.0_f64.powi(-40)] {
            let mut measured = cells(1);
            assert!(measured.open((0, 1), 0..1));
            measured.curve_measured(&m, Point::new(0.0, 1.0 - height));
            let error = measured.arithmetic_error();
            let exact = height * (1.0 - 2.0 * height.sqrt() / 3.0);
            measured.inspect(0, |_, _, area| {
                assert!((area - exact).abs() <= error + 16.0 * f64::EPSILON);
            });
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
    fn inspection_and_emission_share_the_unquantized_prefix_sum() {
        let edges = [
            line((0.25, 0.125), (5.75, 1.875)),
            line((5.75, 1.875), (0.25, 1.875)),
            line((0.25, 1.875), (0.25, 0.125)),
        ];
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let mut a = cells(8);
            let mut b = cells(8);
            assert!(a.open((0, 8), 0..2));
            assert!(b.open((0, 8), 0..2));
            for edge in edges {
                a.line(edge);
                b.line(edge);
            }
            for row in 0..2 {
                let mut expected = Collect(Vec::new());
                a.emit(row, rule, &mut expected);
                let mut emitted = [0; 8];
                for (x0, x1, value) in expected.0 {
                    emitted[x0..x1].fill(value);
                }
                let mut inspected = [0; 8];
                b.inspect(row, |x0, x1, area| inspected[x0..x1].fill(fold(area, rule)));
                assert_eq!(inspected, emitted);
            }
            assert!(b.area.iter().chain(&b.cover).all(|&v| v == 0.0));
            assert!(b.touched.iter().all(|&v| v == 0));
        }
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
