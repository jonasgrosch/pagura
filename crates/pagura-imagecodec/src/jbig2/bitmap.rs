//! Packed bitmaps and region combination (T.88 6.2 and 7.4).
use super::Bitmap;
use crate::budget::Budget;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};
use std::rc::Rc;

/// The work charged per pixel decoded, refined or composited: the pixel,
/// its write and up to sixteen template samples.
pub(super) const PIXEL_WORK: usize = 18;

/// The heap block of an `Rc<Bitmap>`: the bitmap and its two reference
/// counts.
pub(super) const SHARED_BYTES: usize = size_of::<Bitmap>() + 2 * size_of::<usize>();

/// Charges the work of `pixels` decoded, refined or composited pixels.
pub(super) fn charge_pixels(budget: &mut Budget, pixels: u64) -> Result<()> {
    let pixels = usize::try_from(pixels).map_err(|_| Error::limit("JBIG2 pixel work overflow"))?;
    budget.work_product(pixels, PIXEL_WORK)
}

/// Shares a bitmap that later segments or symbols refer to, charging its
/// heap block.
pub(super) fn share(bitmap: Bitmap, budget: &mut Budget) -> Result<Rc<Bitmap>> {
    budget.memory::<u8>(SHARED_BYTES)?;
    Ok(Rc::new(bitmap))
}

/// Drops a shared bitmap, releasing its memory if this was its last
/// reference.
pub(super) fn unshare(bitmap: Rc<Bitmap>, budget: &mut Budget) {
    if let Ok(bitmap) = Rc::try_unwrap(bitmap) {
        budget.release::<u8>(SHARED_BYTES);
        bitmap.release(budget);
    }
}

/// The row stride and the size in bytes of a `width × height` bitmap, whose
/// pixels must fit `Limits::pixels`.
pub(super) fn layout(width: u32, height: u32, limits: &Limits) -> Result<(usize, usize)> {
    let (width, height) = (u32_to_usize(width), u32_to_usize(height));
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
    if pixels > limits.pixels {
        return Err(Error::limit("JBIG2 bitmap exceeds the pixel budget"));
    }
    Ok((stride, bytes))
}

/// How a region's pixels combine with the pixels under them (T.88
/// 7.4.1.5 and 7.4.8.5).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CombinationOp {
    Or = 0,
    And = 1,
    Xor = 2,
    Xnor = 3,
    Replace = 4,
}

impl TryFrom<u8> for CombinationOp {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            0 => Self::Or,
            1 => Self::And,
            2 => Self::Xor,
            3 => Self::Xnor,
            4 => Self::Replace,
            _ => return Err(Error::syntax("invalid JBIG2 combination operator")),
        })
    }
}

impl CombinationOp {
    /// The operator a two-bit field selects: OR, AND, XOR or XNOR, as in
    /// text region and page information flags (T.88 7.4.3.1.1, 7.4.8.5).
    pub(super) const fn from_two_bits(bits: u16) -> Self {
        match bits & 3 {
            0 => Self::Or,
            1 => Self::And,
            2 => Self::Xor,
            _ => Self::Xnor,
        }
    }

    /// Combines the pixel values `old` and `new`, each 0 or 1.
    pub(super) const fn apply(self, old: u8, new: u8) -> u8 {
        match self {
            Self::Or => old | new,
            Self::And => old & new,
            Self::Xor => old ^ new,
            Self::Xnor => 1 ^ old ^ new,
            Self::Replace => new,
        }
    }

    /// Whether combining the pixel value `new` leaves every old pixel as it
    /// is: OR and XOR with 0, AND and XNOR with 1.
    pub(super) const fn keeps(self, new: u8) -> bool {
        matches!(
            (self, new),
            (Self::Or | Self::Xor, 0) | (Self::And | Self::Xnor, 1)
        )
    }
}

/// The value of a bitmap's pixels before anything is drawn on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fill {
    White,
    Black,
}

impl Fill {
    pub(super) const fn from_flag(black: bool) -> Self {
        if black { Self::Black } else { Self::White }
    }

    const fn byte(self) -> u8 {
        match self {
            Self::White => 0,
            Self::Black => u8::MAX,
        }
    }
}

/// One packed row of a bitmap. Pixels outside it read as white, as T.88
/// 6.2.5.2 has template pixels outside a bitmap read.
#[derive(Debug, Clone, Copy)]
pub(super) struct Row<'a> {
    bytes: &'a [u8],
    width: usize,
}

impl<'a> Row<'a> {
    /// A row outside a bitmap.
    pub(super) const EMPTY: Self = Self {
        bytes: &[],
        width: 0,
    };

    /// Row `y` of the rows of `width` pixels packed in `data` with
    /// `stride`, or an empty row outside them.
    pub(super) fn of(data: &'a [u8], stride: usize, width: u32, y: i64) -> Self {
        let Ok(y) = usize::try_from(y) else {
            return Self::EMPTY;
        };
        let start = y.saturating_mul(stride);
        match data.get(start..start.saturating_add(stride)) {
            Some(bytes) => Self::new(bytes, width),
            None => Self::EMPTY,
        }
    }

    pub(super) const fn new(bytes: &'a [u8], width: u32) -> Self {
        Self {
            bytes,
            width: u32_to_usize(width),
        }
    }

    /// Pixel `x`, 0 outside the row.
    pub(super) fn get(self, x: i64) -> u8 {
        match usize::try_from(x) {
            Ok(x) if x < self.width => get(self.bytes, x),
            _ => 0,
        }
    }
}

/// Pixel `x` of a packed row, 0 past its end.
fn get(row: &[u8], x: usize) -> u8 {
    row.get(x / 8).map_or(0, |byte| (byte >> (7 - x % 8)) & 1)
}

/// Sets pixel `x` of a packed row to `bit`, 0 or 1; nothing past its end.
pub(super) fn set(row: &mut [u8], x: usize, bit: u8) {
    if let Some(byte) = row.get_mut(x / 8) {
        let mask = 0x80 >> (x % 8);
        *byte = (*byte & !mask) | if bit == 0 { 0 } else { mask };
    }
}

impl Bitmap {
    /// A working bitmap, charged to `decode_bytes`.
    pub(super) fn new(
        width: u32,
        height: u32,
        fill: Fill,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Self> {
        let (stride, bytes) = layout(width, height, limits)?;
        let mut bitmap = Self {
            width,
            height,
            stride,
            data: budget.filled(bytes, fill.byte())?,
        };
        bitmap.clear_padding();
        Ok(bitmap)
    }

    /// The page bitmap, the output of the decode, charged to
    /// `decoded_bytes`.
    pub(super) fn page(
        width: u32,
        height: u32,
        fill: Fill,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Self> {
        let (stride, bytes) = layout(width, height, limits)?;
        let mut bitmap = Self {
            width,
            height,
            stride,
            data: budget.output(bytes, fill.byte())?,
        };
        bitmap.clear_padding();
        Ok(bitmap)
    }

    /// Drops a working bitmap, releasing its memory.
    pub(super) fn release(self, budget: &mut Budget) {
        budget.release::<u8>(self.data.len());
    }

    /// Grows a page of unknown height to `height` rows of `fill`, charging
    /// the new pixels and bytes.
    pub(super) fn grow(
        &mut self,
        height: u32,
        fill: Fill,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<()> {
        let (_, bytes) = layout(self.width, height, limits)?;
        let rows = height.saturating_sub(self.height);
        charge_pixels(budget, u64::from(self.width) * u64::from(rows))?;
        let extra = bytes.saturating_sub(self.data.len());
        budget.extend_output(&mut self.data, extra, fill.byte())?;
        self.height = height;
        self.clear_padding();
        Ok(())
    }

    /// Row `y`, or an empty row outside the bitmap.
    pub(super) fn row(&self, y: i64) -> Row<'_> {
        Row::of(&self.data, self.stride, self.width, y)
    }

    /// Row `y` for writing, if it is inside the bitmap.
    pub(super) fn row_mut(&mut self, y: i64) -> Option<&mut [u8]> {
        let start = usize::try_from(y).ok()?.checked_mul(self.stride)?;
        self.data.get_mut(start..start.checked_add(self.stride)?)
    }

    /// The rows above row `y`, and row `y` for writing.
    pub(super) fn split_row(&mut self, y: u32) -> Option<(&[u8], &mut [u8])> {
        let start = u32_to_usize(y).checked_mul(self.stride)?;
        let (above, rest) = self.data.split_at_mut_checked(start)?;
        Some((above, rest.get_mut(..self.stride)?))
    }

    /// Sets the pixel at `(x, y)`, which must be inside the bitmap.
    pub(super) fn set(&mut self, x: u32, y: u32, bit: u8) {
        debug_assert!(x < self.width && y < self.height, "pixel outside bitmap");
        if let Some(row) = self.row_mut(i64::from(y)) {
            set(row, u32_to_usize(x), bit);
        }
    }

    fn clear_padding(&mut self) {
        if !self.width.is_multiple_of(8) {
            let mask = u8::MAX << (8 - self.width % 8);
            for row in self.data.chunks_exact_mut(self.stride) {
                if let Some(last) = row.last_mut() {
                    *last &= mask;
                }
            }
        }
    }

    /// Combines `other` with its top left corner at `(x, y)`, which may lie
    /// outside this bitmap; what falls outside is clipped.
    pub(super) fn combine_signed(&mut self, other: &Self, x: i64, y: i64, op: CombinationOp) {
        let x0 = x.saturating_neg().max(0).min(i64::from(other.width));
        let y0 = y.saturating_neg().max(0).min(i64::from(other.height));
        let x1 = (i64::from(self.width).saturating_sub(x)).clamp(x0, i64::from(other.width));
        let y1 = (i64::from(self.height).saturating_sub(y)).clamp(y0, i64::from(other.height));
        for dy in y0..y1 {
            let source = other.row(dy);
            let Some(target) = self.row_mut(y + dy) else {
                continue;
            };
            for dx in x0..x1 {
                if let Ok(tx) = usize::try_from(x + dx) {
                    set(target, tx, op.apply(get(target, tx), source.get(dx)));
                }
            }
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
        op: CombinationOp,
        completed: Option<u32>,
    ) -> Result<Option<u32>> {
        let mut bottom = None;
        let width = other.width.min(self.width.saturating_sub(x));
        let height = other.height.min(self.height.saturating_sub(y));
        for dy in 0..height {
            let source = other.row(i64::from(dy));
            let Some(target) = self.row_mut(i64::from(y + dy)) else {
                continue;
            };
            for dx in 0..width {
                let new = source.get(i64::from(dx));
                if op.keeps(new) {
                    continue;
                }
                if completed.is_some_and(|line| y + dy <= line) {
                    return Err(Error::syntax("JBIG2 region revisits a completed stripe"));
                }
                bottom = Some(y + dy + 1);
                let tx = u32_to_usize(x + dx);
                set(target, tx, op.apply(get(target, tx), new));
            }
        }
        Ok(bottom)
    }
}
