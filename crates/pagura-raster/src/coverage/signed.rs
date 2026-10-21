//! Signed-area coverage for glyphs with a certified winding (ADR 0004).
//!
//! A certified glyph polygon has winding 0 or one common sign almost
//! everywhere, so a cell's signed area is its nonzero area. Edges are placed
//! in the glyph's own integer frame and clipped to integer rows. Each row piece
//! adds its exact trapezoid areas to the cells it crosses and its cover to the
//! next cell; one prefix sum per row then gives every cell's area, which is
//! quantized once. Each cell is a function of the geometry, the exact origin
//! and the cell alone, so viewports and tiles cut the result exactly.

use super::quadratic::rectangular_clips;
use super::rectangle::Rectangle;
use super::{Geometry, Shape, bytes, charge};
use crate::quantize_unit;
use crate::{GlyphGeometry, Point};
use pagura_core::{Error, Limits, Result};

/// Cells in one band of rows; wider glyphs process a single row per band.
const BAND_CELLS: usize = 1 << 16;

/// The certified glyph and its exact device origin, when this kernel applies.
/// Admission depends on the glyph and the retained clip stack, never on tiles.
pub(super) fn admit(shapes: &[Shape]) -> Option<(&GlyphGeometry, Point)> {
    let shape = shapes.first()?;
    let Geometry::Glyph(glyph) = &shape.geometry else {
        return None;
    };
    let t = shape.transform;
    (glyph.signed_area()
        && [t.a, t.b, t.c, t.d] == [1.0, 0.0, 0.0, 1.0]
        && t.is_finite()
        && rectangular_clips(&shapes[1..]))
    .then_some((glyph.as_ref(), Point::new(t.e, t.f)))
}

/// Reused row-band storage.
#[derive(Debug, Default)]
pub(super) struct Signed {
    cells: Vec<f64>,
}

// The glyph's integer frame: local cell (0, 0) is global cell `origin`.
#[derive(Debug, Clone, Copy)]
struct Frame {
    shift: Point,
    floor: Point,
    origin: (i64, i64),
    columns: usize,
    rows: usize,
}

impl Signed {
    pub(super) fn retained_bytes(&self) -> usize {
        bytes(&self.cells)
    }

    /// Writes the coverage of `glyph` placed at `origin` into the viewport.
    /// Cells outside `clip` become zero. Returns whether a fractional clip edge
    /// cuts a cell, which the caller resolves geometrically.
    pub(super) fn render(
        &mut self,
        (glyph, origin): (&GlyphGeometry, Point),
        out: &mut [u8],
        viewport: ((u32, u32), (u32, u32)),
        clip: Rectangle,
        limits: &Limits,
        work: &mut usize,
    ) -> Result<bool> {
        let ((x, y), (width, height)) = viewport;
        let stride =
            usize::try_from(width).map_err(|_| Error::limit("mask width exceeds usize"))?;
        out.fill(0);
        let Some(frame) = frame(glyph, origin)? else {
            return Ok(false);
        };
        // Viewport cell (0, 0) in local cells.
        let left = i64::from(x) - frame.origin.0;
        let top = i64::from(y) - frame.origin.1;
        let first = clamp(top, frame.rows);
        let end = clamp(top + i64::from(height), frame.rows);
        // Viewport rows above the glyph's first row.
        let above = clamp(-top, usize::MAX);
        let width = frame.columns + 2;
        let band = (BAND_CELLS / width).max(1);
        let mut start = first;
        while start < end {
            let stop = end.min(start + band);
            let count = (stop - start) * width;
            if self.cells.len() < count {
                self.cells
                    .try_reserve_exact(count - self.cells.len())
                    .map_err(|_| Error::limit("glyph cell allocation failed"))?;
                self.cells.resize(count, 0.0);
            }
            let cells = &mut self.cells[..count];
            if let Err(error) = accumulate(glyph, frame, (start, stop), cells, limits, work)
                .and_then(|()| charge(work, count, limits.raster_work))
            {
                // A budget error leaves partial sums behind.
                self.cells.clear();
                return Err(error);
            }
            for (row, sums) in (start..stop).zip(cells.chunks_exact_mut(width)) {
                let target = above + (row - first);
                emit(sums, left, &mut out[target * stride..(target + 1) * stride]);
            }
            start = stop;
        }
        Ok(restrict(out, stride, (x, y), clip))
    }
}

fn frame(glyph: &GlyphGeometry, origin: Point) -> Result<Option<Frame>> {
    let Some([lo, hi]) = glyph.polygon_bounds() else {
        return Ok(None);
    };
    let whole = Point::new(origin.x.floor(), origin.y.floor());
    let shift = Point::new(origin.x - whole.x, origin.y - whole.y);
    // Rounding is monotone, so every placed vertex lies within these cells.
    let floor = Point::new((lo.x + shift.x).floor(), (lo.y + shift.y).floor());
    let ceil = Point::new((hi.x + shift.x).ceil(), (hi.y + shift.y).ceil());
    let cells = |count: f64| {
        usize::try_from(integer(count)?).map_err(|_| Error::limit("glyph extent exceeds usize"))
    };
    Ok(Some(Frame {
        shift,
        floor,
        origin: (integer(whole.x + floor.x)?, integer(whole.y + floor.y)?),
        columns: cells(ceil.x - floor.x)?,
        rows: cells(ceil.y - floor.y)?,
    }))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "range-checked integral values convert exactly"
)]
fn integer(value: f64) -> Result<i64> {
    if value.abs() <= 2f64.powi(52) && value.fract() == 0.0 {
        Ok(value as i64)
    } else {
        Err(Error::limit("glyph frame exceeds integer range"))
    }
}

fn clamp(value: i64, maximum: usize) -> usize {
    usize::try_from(value.max(0)).map_or(maximum, |value| value.min(maximum))
}

// Adds every edge's row pieces within rows `first..end` to `cells`, which hold
// one row of columns + 2 accumulators per row and must be zero on entry.
fn accumulate(
    glyph: &GlyphGeometry,
    frame: Frame,
    (first, end): (usize, usize),
    cells: &mut [f64],
    limits: &Limits,
    work: &mut usize,
) -> Result<()> {
    let width = frame.columns + 2;
    let place = |p: Point| {
        Point::new(
            (p.x + frame.shift.x) - frame.floor.x,
            (p.y + frame.shift.y) - frame.floor.y,
        )
    };
    let lo = first as f64;
    let hi = end as f64;
    for contour in glyph.contours() {
        let points = contour.points();
        let Some(&last) = points.last() else {
            continue;
        };
        charge(work, points.len(), limits.raster_work)?;
        let mut previous = place(last);
        for &point in points {
            let current = place(point);
            let (a, b, sign) = if previous.y < current.y {
                (previous, current, 1.0)
            } else {
                (current, previous, -1.0)
            };
            previous = current;
            if a.y == b.y || b.y <= lo || a.y >= hi {
                continue;
            }
            let row = a.y.floor().max(lo);
            if a.y >= row && b.y <= row + 1.0 {
                charge(work, span(a.x, b.x), limits.raster_work)?;
                let offset = (index(row) - first) * width;
                piece(
                    &mut cells[offset..offset + width],
                    a.x,
                    b.x,
                    (b.y - a.y) * sign,
                );
                continue;
            }
            let slope = (b.x - a.x) / (b.y - a.y);
            let (left, right) = (a.x.min(b.x), a.x.max(b.x));
            let at = |y: f64| {
                if y == a.y {
                    a.x
                } else if y == b.y {
                    b.x
                } else {
                    (a.x + (y - a.y) * slope).clamp(left, right)
                }
            };
            let stop = b.y.ceil().min(hi);
            charge(
                work,
                span(a.x, b.x).saturating_add(index(stop - row)),
                limits.raster_work,
            )?;
            let mut top = row;
            while top < stop {
                let lower = a.y.max(top);
                let upper = b.y.min(top + 1.0);
                let offset = (index(top) - first) * width;
                piece(
                    &mut cells[offset..offset + width],
                    at(lower),
                    at(upper),
                    (upper - lower) * sign,
                );
                top += 1.0;
            }
        }
    }
    Ok(())
}

fn span(a: f64, b: f64) -> usize {
    index((a - b).abs().ceil()) + 2
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "local cell coordinates are nonnegative integers within the frame"
)]
fn index(value: f64) -> usize {
    value.max(0.0) as usize
}

// One line piece within a row band, from x = a to x = b, with signed height d.
// Cell i holds the cover change at column i: the prefix sum at a column is the
// band area right of the piece within that column, or d past the piece.
fn piece(row: &mut [f64], a: f64, b: f64, d: f64) {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    let start = lo.floor();
    let end = hi.ceil();
    let i = index(start);
    let j = index(end);
    if j <= i + 1 {
        let middle = a.midpoint(b) - start;
        row[i] += d - d * middle;
        row[i + 1] += d * middle;
        return;
    }
    let s = 1.0 / (hi - lo);
    let head = 1.0 - (lo - start);
    let tail = hi - (end - 1.0);
    let first = 0.5 * s * head * head;
    let last = 0.5 * s * tail * tail;
    row[i] += d * first;
    if j == i + 2 {
        row[i + 1] += d * (1.0 - first - last);
    } else {
        let second = s * (head + 0.5);
        row[i + 1] += d * (second - first);
        let step = d * s;
        for cell in &mut row[i + 2..j - 1] {
            *cell += step;
        }
        let before = second + (j - i - 3) as f64 * s;
        row[j - 1] += d * (1.0 - before - last);
    }
    row[j] += d * last;
}

// Prefix-sums one row from the glyph's first column, so cells left of the
// viewport still contribute, and writes the viewport's columns. Clears the row.
fn emit(sums: &mut [f64], left: i64, output: &mut [u8]) {
    let mut area = 0.0;
    let skip = usize::try_from(left.max(0)).unwrap_or(usize::MAX);
    let shift = usize::try_from((-left).max(0)).unwrap_or(usize::MAX);
    let columns = sums.len() - 2;
    for (column, sum) in sums.iter_mut().enumerate() {
        area += std::mem::take(sum);
        if column < columns
            && column >= skip
            && let Some(cell) = output.get_mut(column - skip + shift)
        {
            *cell = quantize_unit(area.abs());
        }
    }
}

// Clears cells outside the rectangular clip intersection. Returns whether a
// fractional clip edge passes through a viewport cell.
fn restrict(out: &mut [u8], stride: usize, origin: (u32, u32), clip: Rectangle) -> bool {
    let rows = out.len() / stride.max(1);
    let viewport = Rectangle::viewport(
        origin,
        (
            u32::try_from(stride).unwrap_or(u32::MAX),
            u32::try_from(rows).unwrap_or(u32::MAX),
        ),
    );
    if clip.left <= viewport.left
        && clip.top <= viewport.top
        && clip.right >= viewport.right
        && clip.bottom >= viewport.bottom
    {
        return false;
    }
    let mut partial = false;
    for (y, row) in out.chunks_exact_mut(stride).enumerate() {
        let top = viewport.top + y as f64;
        for (x, cell) in row.iter_mut().enumerate() {
            let left = viewport.left + x as f64;
            if left + 1.0 <= clip.left
                || left >= clip.right
                || top + 1.0 <= clip.top
                || top >= clip.bottom
            {
                *cell = 0;
            } else if !interior(clip, left, top) {
                partial = true;
            }
        }
    }
    partial
}

fn interior(clip: Rectangle, left: f64, top: f64) -> bool {
    clip.left <= left && left + 1.0 <= clip.right && clip.top <= top && top + 1.0 <= clip.bottom
}

/// Replaces cells cut by a fractional clip edge with `reference`, which holds
/// the geometric clip of the same viewport. Interior cells keep the kernel's
/// value in every tiling.
pub(super) fn patch(
    out: &mut [u8],
    reference: &[u8],
    stride: usize,
    origin: (u32, u32),
    clip: Rectangle,
) {
    for (y, (row, source)) in out
        .chunks_exact_mut(stride)
        .zip(reference.chunks_exact(stride))
        .enumerate()
    {
        let top = f64::from(origin.1) + y as f64;
        for (x, (cell, &value)) in row.iter_mut().zip(source).enumerate() {
            if !interior(clip, f64::from(origin.0) + x as f64, top) {
                *cell = value;
            }
        }
    }
}

#[cfg(test)]
mod tests;
