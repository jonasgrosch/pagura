//! Signed area differences for the explicitly approximate coverage policy.
//! The source polygons and glyph positions are unchanged. Integrating winding
//! before folding may differ from integrating the filled set in overlap cells.
use super::{Geometry, Shape, bytes, charge};
use crate::flatten::{CurveStack, Polyline, flatten_into_at_tolerance};
use crate::{FillRule, Point};
use pagura_core::{Error, Limits, Result};

#[derive(Debug, Default)]
pub(super) struct Cells {
    values: Vec<f32>,
    touched: Vec<u64>,
    stride: usize,
    words: usize,
    used_words: usize,
    contours: Vec<Polyline>,
    curves: CurveStack,
}

impl Cells {
    pub(super) fn retained_bytes(&self) -> usize {
        bytes(&self.values)
            .saturating_add(bytes(&self.touched))
            .saturating_add(bytes(&self.contours))
            .saturating_add(bytes(&self.curves))
            .saturating_add(
                self.contours
                    .iter()
                    .map(|c| bytes(&c.points))
                    .sum::<usize>(),
            )
    }

    pub(super) fn render(
        &mut self,
        shape: &Shape,
        output: &mut [u8],
        origin: (u32, u32),
        size: (u32, u32),
        limits: &Limits,
        tolerance: f64,
    ) -> Result<bool> {
        let width =
            usize::try_from(size.0).map_err(|_| Error::limit("cell width exceeds usize"))?;
        let height =
            usize::try_from(size.1).map_err(|_| Error::limit("cell height exceeds usize"))?;
        if width == 0 || height == 0 {
            return Ok(false);
        }
        if output.len()
            != width
                .checked_mul(height)
                .ok_or(Error::limit("mask size overflow"))?
        {
            return Err(Error::invalid_argument("mask output size mismatch"));
        }
        let flattened = match &shape.geometry {
            Geometry::Path(path) => flatten_into_at_tolerance(
                path,
                shape.transform,
                limits,
                &mut self.contours,
                &mut self.curves,
                tolerance,
            )?,
            Geometry::Glyph(_) => 0,
        };
        let contours = match &shape.geometry {
            Geometry::Path(_) => &self.contours[..flattened],
            Geometry::Glyph(glyph) => glyph.contours(),
        };
        let mut segments = 0usize;
        let mut leftmost = f64::INFINITY;
        for contour in contours {
            segments = segments
                .checked_add(contour.points.len())
                .filter(|&n| n <= limits.path_segments)
                .ok_or(Error::limit("cell segment budget exceeded"))?;
            // Retain the source-degenerate floor-cell policy on the exact path.
            if contour.degenerate {
                return Ok(false);
            }
            for &point in &contour.points {
                let point = point * shape.transform;
                crate::flatten::bounded(point)?;
                leftmost = leftmost.min(point.x);
            }
        }
        // Cells are indexed from a column that depends only on the geometry
        // (or the page's left edge), never on the tile, and every coordinate
        // stays global. Each row is evaluated on its own, so any tile or band
        // partition assembles into the same coverage.
        let first = first_column(leftmost, origin.0);
        let skip = usize::try_from(origin.0 - first)
            .map_err(|_| Error::limit("cell offset exceeds usize"))?;
        let stride = skip
            .checked_add(width)
            .and_then(|n| n.checked_add(1))
            .ok_or(Error::limit("cell stride overflow"))?;
        let Some(count) = stride
            .checked_mul(height)
            .filter(|&n| n <= limits.decoded_bytes / 16 && n <= 4 * 1024 * 1024)
        else {
            return Ok(false);
        };
        let mut work = count;
        charge(&mut work, 0, limits.raster_work)?;
        charge(&mut work, segments, limits.raster_work)?;
        self.prepare(stride, height, count, &mut work, limits)?;
        let view = View {
            stride,
            words: self.words,
            first: f64::from(first),
            top: f64::from(origin.1),
            rows: height,
        };
        let contours = match &shape.geometry {
            Geometry::Path(_) => &self.contours[..flattened],
            Geometry::Glyph(glyph) => glyph.contours(),
        };
        for contour in contours {
            if contour.points.len() < 2 {
                continue;
            }
            for index in 0..contour.points.len() {
                let a = contour.points[index] * shape.transform;
                let b = contour.points[(index + 1) % contour.points.len()] * shape.transform;
                edge(
                    &mut self.values,
                    &mut self.touched,
                    &view,
                    a,
                    b,
                    &mut work,
                    limits.raster_work,
                )?;
            }
        }
        self.write(output, width, skip, shape.rule);
        Ok(true)
    }
    fn prepare(
        &mut self,
        stride: usize,
        height: usize,
        count: usize,
        work: &mut usize,
        limits: &Limits,
    ) -> Result<()> {
        if self.words != 0 {
            for (y, row) in self.touched[..self.used_words]
                .chunks_exact_mut(self.words)
                .enumerate()
            {
                for (word, bits) in row.iter_mut().enumerate() {
                    charge(work, bits.count_ones() as usize + 1, limits.raster_work)?;
                    let mut marked = std::mem::take(bits);
                    while marked != 0 {
                        let x = word * 64 + marked.trailing_zeros() as usize;
                        self.values[y * self.stride + x] = 0.0;
                        marked &= marked - 1;
                    }
                }
            }
        }
        self.values
            .try_reserve_exact(count.saturating_sub(self.values.len()))
            .map_err(|_| Error::limit("cell allocation failed"))?;
        if count > self.values.len() {
            self.values.resize(count, 0.0);
        }
        let words_per_row = stride.div_ceil(64);
        let words = words_per_row
            .checked_mul(height)
            .ok_or(Error::limit("cell bitmap size overflow"))?;
        self.touched
            .try_reserve_exact(words.saturating_sub(self.touched.len()))
            .map_err(|_| Error::limit("cell bitmap allocation failed"))?;
        if words > self.touched.len() {
            self.touched.resize(words, 0);
        }
        self.stride = stride;
        self.words = words_per_row;
        self.used_words = words;
        Ok(())
    }
    /// Folds each row's running winding into the output, whose first column
    /// is `skip` cells to the right of the first cell.
    fn write(&self, output: &mut [u8], width: usize, skip: usize, rule: FillRule) {
        let end = skip + width;
        for ((row, bits), out) in self
            .values
            .chunks_exact(self.stride)
            .zip(self.touched.chunks_exact(self.words))
            .zip(output.chunks_exact_mut(width))
        {
            let mut winding = 0.0f32;
            let mut left = skip;
            'row: for (word, &marked) in bits.iter().enumerate() {
                let mut marked = marked;
                while marked != 0 {
                    let x = word * 64 + marked.trailing_zeros() as usize;
                    marked &= marked - 1;
                    if x >= end {
                        break 'row;
                    }
                    if x >= skip {
                        let value = fold(winding, rule);
                        out[left - skip..x - skip].fill(value);
                    }
                    winding += row[x];
                    if x >= skip {
                        out[x - skip] = fold(winding, rule);
                        left = x + 1;
                    }
                }
            }
            let value = fold(winding, rule);
            out[left - skip..].fill(value);
        }
    }
}

/// The device column of the first cell: the geometry's leftmost column, or
/// the page's left edge, but never right of the mask.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=origin, a u32"
)]
fn first_column(leftmost: f64, origin: u32) -> u32 {
    if leftmost.is_finite() {
        leftmost.floor().clamp(0.0, f64::from(origin)) as u32
    } else {
        origin
    }
}

/// The cell rows and columns one mask covers, in global device coordinates.
struct View {
    stride: usize,
    words: usize,
    /// The device column of the first cell.
    first: f64,
    /// The device row of the first cell row.
    top: f64,
    rows: usize,
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "row and column indices are clamped to the allocated viewport"
)]
fn edge(
    cells: &mut [f32],
    touched: &mut [u64],
    view: &View,
    mut a: Point,
    mut b: Point,
    work: &mut usize,
    maximum: usize,
) -> Result<()> {
    if a.y == b.y {
        return Ok(());
    }
    let sign = if a.y < b.y {
        1.0
    } else {
        std::mem::swap(&mut a, &mut b);
        -1.0
    };
    let bottom = view.top + view.rows as f64;
    let first = (a.y.floor().clamp(view.top, bottom) - view.top) as usize;
    let end = (b.y.ceil().clamp(view.top, bottom) - view.top) as usize;
    charge(work, end - first, maximum)?;
    let dx = (b.x - a.x) / (b.y - a.y);
    if !dx.is_finite() {
        return Err(Error::limit("cell slope exceeds numeric range"));
    }
    // Columns are global device columns; `view.first` maps them to cells.
    let width = view.stride - 1;
    let right_edge = view.first + width as f64;
    for y in first..end {
        let row_top = view.top + y as f64;
        let low = a.y.max(row_top);
        let high = b.y.min(row_top + 1.0);
        if high <= low {
            continue;
        }
        let ax = a.x + (low - a.y) * dx;
        let bx = a.x + (high - a.y) * dx;
        let row = &mut cells[y * view.stride..(y + 1) * view.stride];
        let bits = &mut touched[y * view.words..(y + 1) * view.words];
        if ax == bx {
            if ax < view.first {
                delta(row, bits, 0, ((high - low) * sign) as f32);
            } else if ax < right_edge {
                let column = ax.floor();
                let x = (column - view.first) as usize;
                let area = (high - low) * sign;
                let fraction = ax - column;
                delta(row, bits, x, (area * (1.0 - fraction)) as f32);
                delta(row, bits, x + 1, (area * fraction) as f32);
            }
            continue;
        }
        let (left, right) = (ax.min(bx), ax.max(bx));
        if left < view.first {
            let covered = if right <= view.first {
                high - low
            } else {
                (view.first - left) / (right - left) * (high - low)
            };
            delta(row, bits, 0, (covered * sign) as f32);
        }
        let from = left.floor().clamp(view.first, right_edge);
        let to = right.ceil().clamp(view.first, right_edge);
        charge(work, (to - from) as usize, maximum)?;
        let rate = (high - low) / (right - left) * sign;
        let mut column = from;
        while column < to {
            let x = (column - view.first) as usize;
            let l = left.max(column);
            let r = right.min(column + 1.0);
            let cover = if rate.is_finite() {
                (r - l) * rate
            } else {
                (r - l) / (right - left) * (high - low) * sign
            };
            let area = cover * (1.0 - f64::midpoint(l - column, r - column));
            delta(row, bits, x, area as f32);
            delta(row, bits, x + 1, (cover - area) as f32);
            column += 1.0;
        }
    }
    Ok(())
}

fn delta(row: &mut [f32], bits: &mut [u64], x: usize, value: f32) {
    if value != 0.0 {
        row[x] += value;
        bits[x / 64] |= 1 << (x % 64);
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "folded area is clamped to 0..1 before byte rounding"
)]
fn fold(winding: f32, rule: FillRule) -> u8 {
    let area = match rule {
        FillRule::NonZero => winding.abs().min(1.0),
        FillRule::EvenOdd => {
            let t = winding.abs() % 2.0;
            t.min(2.0 - t)
        }
    };
    (area * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PathBuilder, Transform};
    use std::sync::Arc;

    #[test]
    fn triangle_areas_match_rational_pixel_intersections_in_both_directions() {
        for reverse in [false, true] {
            let mut path = PathBuilder::new();
            path.move_to(Point::new(0.0, 0.0));
            let vertices = if reverse {
                [[0.0, 2.0], [2.0, 0.0]]
            } else {
                [[2.0, 0.0], [0.0, 2.0]]
            };
            for [x, y] in vertices {
                path.line_to(Point::new(x, y)).unwrap();
            }
            path.close();
            let shape = Shape {
                geometry: Geometry::Path(Arc::new(path.finish())),
                transform: Transform::IDENTITY,
                rule: FillRule::NonZero,
            };
            let mut cells = Cells::default();
            let mut output = [0; 4];
            assert!(
                cells
                    .render(
                        &shape,
                        &mut output,
                        (0, 0),
                        (2, 2),
                        &Limits::default(),
                        1.0 / 256.0
                    )
                    .unwrap()
            );
            assert_eq!(output, [255, 128, 128, 0]);
            let mut part = [0];
            assert!(
                cells
                    .render(
                        &shape,
                        &mut part,
                        (1, 0),
                        (1, 1),
                        &Limits::default(),
                        1.0 / 256.0
                    )
                    .unwrap()
            );
            assert_eq!(part, [128]);
        }
    }

    #[test]
    fn cell_storage_and_work_are_bounded_before_scattering() {
        let mut path = PathBuilder::new();
        path.rect(0.25, 0.25, 1.0, 1.0);
        let shape = Shape {
            geometry: Geometry::Path(Arc::new(path.finish())),
            transform: Transform::IDENTITY,
            rule: FillRule::EvenOdd,
        };
        let mut cells = Cells::default();
        assert!(
            !cells
                .render(
                    &shape,
                    &mut [0; 4],
                    (0, 0),
                    (2, 2),
                    &Limits {
                        decoded_bytes: 1,
                        ..Limits::default()
                    },
                    1.0 / 256.0
                )
                .unwrap()
        );
        assert!(
            cells
                .render(
                    &shape,
                    &mut [0; 4],
                    (0, 0),
                    (2, 2),
                    &Limits {
                        raster_work: 1,
                        ..Limits::default()
                    },
                    1.0 / 256.0
                )
                .is_err()
        );
    }
}

#[cfg(test)]
mod reuse_tests {
    use super::*;
    use crate::{PathBuilder, Transform};
    use std::sync::Arc;
    #[test]
    fn sparse_rows_reuse_storage_without_leaking_prior_shapes() {
        let mut cells = Cells::default();
        for (width, height, x0, y0, x1, y1) in [
            (132, 57, 299, 11, 313, 61),
            (2, 4, 1, 2, 6, 14),
            (79, 9, 17, 5, 29, 21),
            (1, 1, 1, 1, 3, 3),
            (132, 57, 17, 11, 43, 89),
        ] {
            let mut path = PathBuilder::new();
            path.rect(
                f64::from(x0) / 4.0,
                f64::from(y0) / 4.0,
                f64::from(x1 - x0) / 4.0,
                f64::from(y1 - y0) / 4.0,
            );
            let shape = Shape {
                geometry: Geometry::Path(Arc::new(path.finish())),
                transform: Transform::IDENTITY,
                rule: FillRule::NonZero,
            };
            let mut output = vec![170; usize::try_from(width * height).unwrap()];
            assert!(
                cells
                    .render(
                        &shape,
                        &mut output,
                        (0, 0),
                        (width, height),
                        &Limits::default(),
                        1.0 / 256.0
                    )
                    .unwrap()
            );
            for y in 0..height {
                for x in 0..width {
                    let dx = x1.min((x + 1) * 4).saturating_sub(x0.max(x * 4));
                    let dy = y1.min((y + 1) * 4).saturating_sub(y0.max(y * 4));
                    let expected = u8::try_from((dx * dy * 255 + 8) / 16).unwrap();
                    assert_eq!(output[usize::try_from(y * width + x).unwrap()], expected);
                }
            }
        }
    }
}
