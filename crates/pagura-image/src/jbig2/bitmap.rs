//! Packed bitmaps and region combination (T.88 clauses 6.2 and 7.4).
use super::Bitmap;
use pagura_core::{Error, Limits, Result};

pub(super) fn layout(width: u32, height: u32, limits: &Limits) -> Result<(usize, usize)> {
    let width = usize::try_from(width).map_err(|_| Error::limit("JBIG2 width overflow"))?;
    let height = usize::try_from(height).map_err(|_| Error::limit("JBIG2 height overflow"))?;
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| Error::limit("JBIG2 pixel count overflow"))?;
    let stride = width
        .checked_add(7)
        .ok_or_else(|| Error::limit("JBIG2 row size overflow"))?
        / 8;
    let bytes = stride
        .checked_mul(height)
        .ok_or_else(|| Error::limit("JBIG2 bitmap size overflow"))?;
    if pixels > limits.pixels || bytes > limits.decoded_bytes {
        return Err(Error::limit("JBIG2 bitmap exceeds limits"));
    }
    Ok((stride, bytes))
}

impl Bitmap {
    pub(super) fn new(width: u32, height: u32, black: bool, limits: &Limits) -> Result<Self> {
        let (_, bytes) = layout(width, height, limits)?;
        let mut bitmap = Self {
            width,
            height,
            data: vec![if black { 255 } else { 0 }; bytes],
        };
        bitmap.clear_padding();
        Ok(bitmap)
    }

    pub(super) fn stride(&self) -> usize {
        // The constructor has proved this fits, including on wasm32.
        self.data
            .len()
            .checked_div(self.height as usize)
            .unwrap_or(0)
    }

    pub(super) fn pixel(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return 0;
        }
        let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
            return 0;
        };
        self.data
            .get(y * self.stride() + x / 8)
            .map_or(0, |b| (b >> (7 - x % 8)) & 1)
    }

    pub(super) fn set(&mut self, x: u32, y: u32, bit: u8) {
        let stride = self.stride();
        if x < self.width
            && y < self.height
            && let Some(byte) = self.data.get_mut(y as usize * stride + x as usize / 8)
        {
            let mask = 0x80 >> (x % 8);
            *byte = (*byte & !mask) | if bit == 0 { 0 } else { mask };
        }
    }

    pub(super) fn clear_padding(&mut self) {
        if !self.width.is_multiple_of(8) {
            let mask = 255 << (8 - self.width % 8);
            let stride = self.stride();
            for row in self.data.chunks_mut(stride.max(1)) {
                if let Some(last) = row.last_mut() {
                    *last &= mask;
                }
            }
        }
    }

    /// Packed row `y`.
    fn row(&self, y: usize) -> &[u8] {
        let stride = self.stride();
        self.data.get(y * stride..(y + 1) * stride).unwrap_or(&[])
    }

    fn row_mut(&mut self, y: usize) -> &mut [u8] {
        let stride = self.stride();
        self.data
            .get_mut(y * stride..(y + 1) * stride)
            .unwrap_or(&mut [])
    }

    /// Combines `other` placed with its top left at (`x`, `y`) with `op`
    /// (T.88 7.4.10: 0 OR, 1 AND, 2 XOR, 3 XNOR, otherwise REPLACE), where
    /// it lies on the bitmap.
    pub(super) fn combine_signed(&mut self, other: &Self, x: i64, y: i64, op: u8) {
        let x0 = x.saturating_neg().max(0).min(i64::from(other.width));
        let y0 = y.saturating_neg().max(0).min(i64::from(other.height));
        let x1 = (i64::from(self.width).saturating_sub(x)).clamp(x0, i64::from(other.width));
        let y1 = (i64::from(self.height).saturating_sub(y)).clamp(y0, i64::from(other.height));
        let index = |v: i64| usize::try_from(v).unwrap_or(0);
        let (from, to, len) = (index(x0), index(x + x0), index(x1 - x0));
        for dy in y0..y1 {
            combine_row(
                self.row_mut(index(y + dy)),
                to,
                other.row(index(dy)),
                from,
                len,
                op,
            );
        }
    }

    /// T.88 7.4.10 constrains drawing, rather than the region rectangle.
    /// Identity operands (OR/XOR zero, AND/XNOR one) do not write pixels;
    /// padding containing them may cross a completed stripe boundary.
    /// The returned exclusive bottom row covers every non-identity operand.
    pub(super) fn combine_page(
        &mut self,
        other: &Self,
        x: u32,
        y: u32,
        op: u8,
        completed: Option<u32>,
    ) -> Result<Option<u32>> {
        let width = other.width.min(self.width.saturating_sub(x)) as usize;
        let height = other.height.min(self.height.saturating_sub(y));
        let writing = |dy: u32| writes(other.row(dy as usize), width, op);
        let Some(first) = (0..height).find(|&dy| writing(dy)) else {
            return Ok(None);
        };
        if completed.is_some_and(|line| y + first <= line) {
            return Err(Error::syntax("JBIG2 region revisits a completed stripe"));
        }
        let last = (first..height)
            .rev()
            .find(|&dy| writing(dy))
            .unwrap_or(first);
        for dy in first..=last {
            combine_row(
                self.row_mut((y + dy) as usize),
                x as usize,
                other.row(dy as usize),
                0,
                width,
                op,
            );
        }
        Ok(Some(y + last + 1))
    }
}

/// The eight bits of `row` from bit `at` on, most significant first; bits
/// after the row read as zero.
fn bits_at(row: &[u8], at: usize) -> u8 {
    let (index, shift) = (at / 8, at % 8);
    let high = row.get(index).copied().unwrap_or(0);
    if shift == 0 {
        return high;
    }
    let low = row.get(index + 1).copied().unwrap_or(0);
    (high << shift) | (low >> (8 - shift))
}

/// Combines bits `from..from + len` of `source` into `target` from bit `to`
/// on with `op`, a byte of `target` at a time.
fn combine_row(target: &mut [u8], to: usize, source: &[u8], from: usize, len: usize, op: u8) {
    let end = to + len;
    // Bit coordinates can exceed isize::MAX on wasm32 even when the
    // packed allocation fits. Keep their displacement unsigned.
    let before = to.saturating_sub(from);
    let after = from.saturating_sub(to);
    for (index, a) in target
        .iter_mut()
        .enumerate()
        .take(end.div_ceil(8))
        .skip(to / 8)
    {
        let start = 8 * index;
        let mask =
            (0xff_u8 >> (to.max(start) - start)) & (0xff_u8 << (start + 8 - end.min(start + 8)));
        let b = match start.checked_sub(before) {
            Some(at) => bits_at(source, at + after),
            // Only the first target byte can reach before source bit zero,
            // by at most seven bits; those leading bits are masked off.
            None => bits_at(source, 0) >> (before - start),
        };
        let value = match op {
            0 => *a | b,
            1 => *a & b,
            2 => *a ^ b,
            3 => !(*a ^ b),
            _ => b,
        };
        *a = (*a & !mask) | (value & mask);
    }
}

/// Whether the first `len` bits of `row` hold an operand of `op` that
/// writes: a one for OR and XOR, a zero for AND and XNOR, any for REPLACE.
fn writes(row: &[u8], len: usize, op: u8) -> bool {
    let ones = match op {
        0 | 2 => true,
        1 | 3 => false,
        _ => return len > 0,
    };
    row.iter()
        .take(len.div_ceil(8))
        .enumerate()
        .any(|(index, &byte)| {
            let mask = 0xff_u8 << (8 * index + 8 - len.min(8 * index + 8));
            (if ones { byte } else { !byte }) & mask != 0
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit offsets may cross the signed address boundary while their
    /// packed storage still fits. This exercises that boundary on wasm32.
    #[test]
    #[ignore = "allocates a 256 MiB packed row; run separately on native and wasm32"]
    fn wide_packed_row_keeps_unsigned_bit_offsets() {
        let x = (1_u32 << 31) + 3;
        let width = x + 9;
        let limits = Limits {
            pixels: width as usize,
            decoded_bytes: width.div_ceil(8) as usize,
            ..Limits::default()
        };
        let mut target = Bitmap::new(width, 1, false, &limits).unwrap();
        let mut source = Bitmap::new(9, 1, false, &limits).unwrap();
        source.data.copy_from_slice(&[0xa6, 0x80]);
        target.combine_signed(&source, i64::from(x), 0, 4);
        for column in 0..9 {
            assert_eq!(
                target.pixel(i64::from(x) + column, 0),
                source.pixel(column, 0)
            );
        }
        assert_eq!(target.pixel(i64::from(x) - 1, 0), 0);
        assert_eq!(target.data.last().copied().unwrap() & 0x0f, 0);
    }

    /// Pixel by pixel, as T.88 7.4.10 states it.
    fn combine_pixels(target: &mut Bitmap, other: &Bitmap, x: i64, y: i64, op: u8) {
        for dy in 0..i64::from(other.height) {
            for dx in 0..i64::from(other.width) {
                let (Ok(px), Ok(py)) = (u32::try_from(x + dx), u32::try_from(y + dy)) else {
                    continue;
                };
                let (a, b) = (target.pixel(x + dx, y + dy), other.pixel(dx, dy));
                target.set(
                    px,
                    py,
                    match op {
                        0 => a | b,
                        1 => a & b,
                        2 => a ^ b,
                        3 => 1 ^ a ^ b,
                        _ => b,
                    },
                );
            }
        }
    }

    fn noise(width: u32, height: u32, seed: &mut u32) -> Bitmap {
        let mut bitmap = Bitmap::new(width, height, false, &Limits::default()).unwrap();
        for byte in &mut bitmap.data {
            *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *byte = (*seed >> 16).to_le_bytes()[0];
        }
        bitmap.clear_padding();
        bitmap
    }

    /// Row combination equals combining pixel by pixel for every operator,
    /// alignment and clipping, and a page combination writes, refuses and
    /// reports the bottom exactly where a non-identity operand lies.
    #[test]
    fn row_combination_matches_pixel_combination() {
        let mut seed = 7;
        for case in 0..600_u32 {
            let (w, h) = (1 + case % 37, 1 + case % 5);
            let other = noise(w, h, &mut seed);
            let page = noise(29 + case % 11, 9, &mut seed);
            let (x, y) = (i64::from(case % 47) - 12, i64::from(case % 13) - 3);
            let op = u8::try_from(case % 5).unwrap();
            let mut rows = Bitmap {
                data: page.data.clone(),
                ..page
            };
            let mut pixels = Bitmap {
                data: page.data.clone(),
                ..page
            };
            rows.combine_signed(&other, x, y, op);
            combine_pixels(&mut pixels, &other, x, y, op);
            assert_eq!(rows.data, pixels.data, "case {case}");
            let (Ok(px), Ok(py)) = (u32::try_from(x), u32::try_from(y)) else {
                continue;
            };
            let mut sparse = noise(w, h, &mut seed);
            // Mostly identity operands, so that bottoms and refusals vary.
            for (index, byte) in sparse.data.iter_mut().enumerate() {
                *byte = if index % 3 == 0 { *byte & 0x11 } else { 0 };
                if op % 2 == 1 {
                    *byte = !*byte;
                }
            }
            sparse.clear_padding();
            let mut expected = Bitmap {
                data: page.data.clone(),
                ..page
            };
            let width = sparse.width.min(page.width.saturating_sub(px));
            let height = sparse.height.min(page.height.saturating_sub(py));
            let mut bottom = None;
            for dy in 0..height {
                for dx in 0..width {
                    let b = sparse.pixel(i64::from(dx), i64::from(dy));
                    if !matches!((op, b), (0 | 2, 0) | (1 | 3, 1)) {
                        bottom = Some(py + dy + 1);
                    }
                }
            }
            combine_pixels(&mut expected, &sparse, x, y, op);
            for completed in [None, Some(py), bottom.map(|b| b - 1)] {
                let mut combined = Bitmap {
                    data: page.data.clone(),
                    ..page
                };
                let result = combined.combine_page(&sparse, px, py, op, completed);
                let first = (0..height).find(|&dy| {
                    (0..width).any(|dx| {
                        let b = sparse.pixel(i64::from(dx), i64::from(dy));
                        !matches!((op, b), (0 | 2, 0) | (1 | 3, 1))
                    })
                });
                if first.zip(completed).is_some_and(|(f, line)| py + f <= line) {
                    assert!(result.is_err(), "case {case}");
                    assert_eq!(
                        combined.data, page.data,
                        "case {case}: refused before writing"
                    );
                } else {
                    assert_eq!(result.unwrap(), bottom, "case {case}");
                    assert_eq!(combined.data, expected.data, "case {case}");
                }
            }
        }
    }
}
