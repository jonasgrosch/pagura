//! One-bit samples in their packed PDF rows.

/// One-bit, one-component samples, most significant bit first, with each
/// row padded to whole bytes as in the image data (8.9.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packed {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride: usize,
    pub(crate) bytes: Vec<u8>,
}

impl Packed {
    /// Width in samples.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in samples.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Packed row `y`, including its trailing padding bits.
    #[must_use]
    pub fn row(&self, y: usize) -> Option<&[u8]> {
        self.bytes
            .get(y.checked_mul(self.stride)?..y.checked_add(1)?.checked_mul(self.stride)?)
    }

    /// The sample code at `index` in row-major order, excluding padding.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<u16> {
        if index >= self.width * self.height {
            return None;
        }
        let y = index / self.width;
        let x = index % self.width;
        Some(u16::from(
            (self.bytes[y * self.stride + x / 8] >> (7 - x % 8)) & 1,
        ))
    }

    pub(crate) fn storage_bytes(&self) -> usize {
        self.bytes.capacity()
    }
}
