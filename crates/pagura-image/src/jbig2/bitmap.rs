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

    pub(super) fn combine_signed(&mut self, other: &Self, x: i64, y: i64, op: u8) {
        let x0 = x.saturating_neg().max(0).min(i64::from(other.width));
        let y0 = y.saturating_neg().max(0).min(i64::from(other.height));
        let x1 = (i64::from(self.width).saturating_sub(x)).clamp(x0, i64::from(other.width));
        let y1 = (i64::from(self.height).saturating_sub(y)).clamp(y0, i64::from(other.height));
        for dy in y0..y1 {
            for dx in x0..x1 {
                let a = self.pixel(x + dx, y + dy);
                let b = other.pixel(dx, dy);
                if let (Ok(px), Ok(py)) = (u32::try_from(x + dx), u32::try_from(y + dy)) {
                    self.set(
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
    }

    pub(super) fn combine(&mut self, other: &Self, x: u32, y: u32, op: u8) {
        let width = other.width.min(self.width.saturating_sub(x));
        let height = other.height.min(self.height.saturating_sub(y));
        for dy in 0..height {
            for dx in 0..width {
                let a = self.pixel(i64::from(x + dx), i64::from(y + dy));
                let b = other.pixel(i64::from(dx), i64::from(dy));
                self.set(
                    x + dx,
                    y + dy,
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
}
