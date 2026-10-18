//! Bounded output and incremental predictor reversal (ISO 32000-2 7.4.4.4).
//! PNG arithmetic: Second Edition 9.2, Table 9.1, 9.3 and 9.4. TIFF sample
//! arithmetic: TIFF 6.0 section 14, pp.64-65; PDF supplies high-bit-first packing.
use crate::report::syntax;
use crate::{Predictor, PredictorKind};
use pagura_core::Result;

pub(crate) struct Output {
    bytes: Vec<u8>,
    cap: usize,
    predictor: Predictor,
    row_bytes: usize,
    row_samples: usize,
    pixel_bytes: usize,
    row_offset: usize,
    png_filter: Option<u8>,
}

impl Output {
    pub(crate) fn new(cap: usize, predictor: Predictor) -> Result<Self> {
        if predictor.colors == 0
            || predictor.columns == 0
            || !matches!(predictor.bits_per_component, 1 | 2 | 4 | 8 | 16)
        {
            return Err(syntax("invalid predictor parameters", 0));
        }
        let pixel_bits = usize::from(predictor.colors) * usize::from(predictor.bits_per_component);
        // With Predictor=1 the dimensions do not participate in decoding.
        let columns = if predictor.kind == PredictorKind::None {
            0
        } else {
            predictor.columns
        };
        let row_samples = usize::try_from(columns)
            .ok()
            .and_then(|columns| columns.checked_mul(usize::from(predictor.colors)))
            .ok_or_else(|| crate::report::limit(0))?;
        let row_bytes = row_samples
            .checked_mul(usize::from(predictor.bits_per_component))
            .map(|bits| bits.div_ceil(8))
            .ok_or_else(|| crate::report::limit(0))?;
        Ok(Self {
            bytes: Vec::new(),
            cap,
            predictor,
            row_bytes,
            row_samples,
            pixel_bytes: pixel_bits.div_ceil(8),
            row_offset: 0,
            png_filter: None,
        })
    }

    pub(crate) fn buffer(&self) -> &[u8] {
        &self.bytes
    }

    // These two operations are selected only when prediction is absent.
    pub(crate) fn literal(&mut self, byte: u8, offset: usize) -> Result<()> {
        self.reserve(1, offset)?;
        self.bytes.push(byte);
        Ok(())
    }

    pub(crate) fn copy(&mut self, distance: usize, length: usize, offset: usize) -> Result<()> {
        let start = self
            .bytes
            .len()
            .checked_sub(distance)
            .filter(|_| distance != 0)
            .ok_or_else(|| syntax("DEFLATE distance exceeds history", offset))?;
        self.reserve(length, offset)?;
        let mut remaining = length;
        while remaining != 0 {
            let count = remaining.min(self.bytes.len() - start);
            self.bytes.extend_from_within(start..start + count);
            remaining -= count;
        }
        Ok(())
    }

    // PNG tags belong to pre-predictor history, but not to the decoded cap.
    pub(crate) fn history_capacity(&self) -> usize {
        match self.predictor.kind {
            PredictorKind::None => 0,
            PredictorKind::Tiff => self.cap.min(32768),
            PredictorKind::Png => self
                .cap
                .saturating_add(self.cap / self.row_bytes)
                .saturating_add(1)
                .min(32768),
        }
    }

    pub(crate) fn write(&mut self, bytes: &[u8], offset: usize) -> Result<()> {
        self.check(bytes.len(), offset)?;
        if self.predictor.kind == PredictorKind::None {
            self.reserve(bytes.len(), offset)?;
            self.bytes.extend_from_slice(bytes);
            return Ok(());
        }
        for &byte in bytes {
            if self.predictor.kind == PredictorKind::Png && self.png_filter.is_none() {
                if byte > 4 {
                    return Err(syntax("invalid PNG predictor tag", offset));
                }
                self.png_filter = Some(byte);
                continue;
            }
            self.reserve(1, offset)?;
            let byte = if self.predictor.kind == PredictorKind::Png {
                self.png_byte(byte)
            } else {
                byte
            };
            self.bytes.push(byte);
            self.row_offset += 1;
            if self.row_offset == self.row_bytes {
                if self.predictor.kind == PredictorKind::Tiff {
                    self.tiff_row();
                }
                self.row_offset = 0;
                self.png_filter = None;
            }
        }
        Ok(())
    }

    // Account for PNG tag bytes without charging them to the decoded cap.
    pub(crate) fn check(&self, raw: usize, offset: usize) -> Result<()> {
        let tags = if self.predictor.kind == PredictorKind::Png {
            let stride = self
                .row_bytes
                .checked_add(1)
                .ok_or_else(|| crate::report::limit(offset))?;
            let until_tag = if self.png_filter.is_none() {
                0
            } else {
                self.row_bytes - self.row_offset
            };
            if raw > until_tag {
                1 + (raw - until_tag - 1) / stride
            } else {
                0
            }
        } else {
            0
        };
        self.bytes
            .len()
            .checked_add(raw - tags)
            .filter(|&n| n <= self.cap)
            .ok_or_else(|| crate::report::limit(offset))?;
        Ok(())
    }

    fn reserve(&mut self, count: usize, offset: usize) -> Result<()> {
        let required = self
            .bytes
            .len()
            .checked_add(count)
            .filter(|&n| n <= self.cap)
            .ok_or_else(|| crate::report::limit(offset))?;
        if required > self.bytes.capacity() {
            let target = required
                .max(self.bytes.capacity().saturating_mul(2).max(64))
                .min(self.cap);
            self.bytes
                .try_reserve_exact(target - self.bytes.len())
                .map_err(|_| crate::report::limit(offset))?;
        }
        Ok(())
    }

    fn png_byte(&self, byte: u8) -> u8 {
        let end = self.bytes.len();
        let left = if self.row_offset >= self.pixel_bytes {
            self.bytes.get(end - self.pixel_bytes).copied().unwrap_or(0)
        } else {
            0
        };
        let above = end
            .checked_sub(self.row_bytes)
            .and_then(|i| self.bytes.get(i))
            .copied()
            .unwrap_or(0);
        let upper_left = if self.row_offset >= self.pixel_bytes {
            end.checked_sub(self.row_bytes)
                .and_then(|i| i.checked_sub(self.pixel_bytes))
                .and_then(|i| self.bytes.get(i))
                .copied()
                .unwrap_or(0)
        } else {
            0
        };
        let prediction = match self.png_filter {
            Some(1) => left,
            Some(2) => above,
            Some(3) => (left & above) + ((left ^ above) >> 1),
            Some(4) => paeth(left, above, upper_left),
            _ => 0,
        };
        byte.wrapping_add(prediction)
    }

    fn tiff_row(&mut self) {
        let start = self.bytes.len() - self.row_bytes;
        // The checked row layout guarantees this range; do not index input offsets.
        let Some(row) = self.bytes.get_mut(start..) else {
            return;
        };
        let bits = usize::from(self.predictor.bits_per_component);
        let colors = usize::from(self.predictor.colors);
        for sample in colors..self.row_samples {
            let value = read_sample(row, sample, bits).wrapping_add(read_sample(
                row,
                sample - colors,
                bits,
            ));
            write_sample(row, sample, bits, value);
        }
    }

    pub(crate) fn finish(self, offset: usize) -> Result<Vec<u8>> {
        if self.row_offset != 0 || self.png_filter.is_some() {
            return Err(syntax("incomplete predictor row", offset));
        }
        Ok(self.bytes)
    }
}

pub(crate) fn paeth(left: u8, above: u8, upper_left: u8) -> u8 {
    // PNG Second Edition 9.4: exact arithmetic; ties prefer left, then above.
    let p = i16::from(left) + i16::from(above) - i16::from(upper_left);
    let a = (p - i16::from(left)).abs();
    let b = (p - i16::from(above)).abs();
    let c = (p - i16::from(upper_left)).abs();
    if a <= b && a <= c {
        left
    } else if b <= c {
        above
    } else {
        upper_left
    }
}

fn read_sample(row: &[u8], sample: usize, bits: usize) -> u16 {
    // Constructor checked row_samples * bits; sample is smaller than row_samples.
    let bit = sample * bits;
    let byte = row.get(bit / 8).copied().unwrap_or(0);
    if bits == 16 {
        u16::from_be_bytes([byte, row.get(bit / 8 + 1).copied().unwrap_or(0)])
    } else {
        (u16::from(byte) >> (8 - bits - bit % 8)) & ((1 << bits) - 1)
    }
}

fn write_sample(row: &mut [u8], sample: usize, bits: usize, value: u16) {
    // Constructor checked row_samples * bits; sample is smaller than row_samples.
    let bit = sample * bits;
    let [high, low] = value.to_be_bytes();
    if bits == 16 {
        if let Some(byte) = row.get_mut(bit / 8) {
            *byte = high;
        }
        if let Some(byte) = row.get_mut(bit / 8 + 1) {
            *byte = low;
        }
    } else {
        let shift = 8 - bits - bit % 8;
        let mask = u8::MAX >> (8 - bits);
        if let Some(byte) = row.get_mut(bit / 8) {
            *byte = (*byte & !(mask << shift)) | ((low & mask) << shift);
        }
    }
}
