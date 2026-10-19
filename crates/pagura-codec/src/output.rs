//! Bounded decoder output and incremental predictor reversal (7.4.4.4).
//! PNG arithmetic: PNG Second Edition 9.2, Table 9.1, 9.3 and 9.4. TIFF sample
//! arithmetic: TIFF 6.0 section 14, pp.64-65; PDF supplies high-bit-first packing.
use crate::bounded::{BoundedBytes, DECODE_BUDGET, decode_limit};
use crate::flate::WINDOW;
use crate::predictor::PngFilter;
use crate::report::syntax;
use crate::{Predictor, PredictorKind};
use pagura_core::{Error, Position, Result, ResultExt};

pub(crate) struct Output {
    bytes: BoundedBytes,
    predictor: Predictor,
    row_bytes: usize,
    row_samples: usize,
    pixel_bytes: usize,
    row_offset: usize,
    png_filter: Option<PngFilter>,
}

impl Output {
    pub(crate) fn new(cap: usize, predictor: Predictor) -> Result<Self> {
        let colors = usize::from(predictor.colors());
        let bits = usize::from(predictor.bits_per_component());
        // With Predictor=1 the dimensions do not participate in decoding.
        let columns = if predictor.kind() == PredictorKind::None {
            0
        } else {
            predictor.columns()
        };
        let row_samples = usize::try_from(columns)
            .ok()
            .and_then(|columns| columns.checked_mul(colors))
            .ok_or(Error::limit(DECODE_BUDGET))?;
        let row_bytes = row_samples
            .checked_mul(bits)
            .map(|bits| bits.div_ceil(8))
            .ok_or(Error::limit(DECODE_BUDGET))?;
        Ok(Self {
            bytes: BoundedBytes::new(cap, DECODE_BUDGET),
            predictor,
            row_bytes,
            row_samples,
            pixel_bytes: (colors * bits).div_ceil(8),
            row_offset: 0,
            png_filter: None,
        })
    }

    pub(crate) fn buffer(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    // These two operations are selected only when prediction is absent.
    pub(crate) fn literal(&mut self, byte: u8, offset: usize) -> Result<()> {
        self.bytes.push(byte).at(Position::Buffer(offset))
    }

    pub(crate) fn copy(&mut self, distance: usize, length: usize, offset: usize) -> Result<()> {
        let start = self
            .bytes
            .as_slice()
            .len()
            .checked_sub(distance)
            .filter(|_| distance != 0)
            .ok_or_else(|| syntax("DEFLATE distance exceeds history", offset))?;
        self.bytes
            .repeat(start, length)
            .at(Position::Buffer(offset))
    }

    // PNG tags belong to pre-predictor history, but not to the decoded cap.
    pub(crate) fn history_capacity(&self) -> usize {
        let cap = self.bytes.max();
        match self.predictor.kind() {
            PredictorKind::None => 0,
            PredictorKind::Tiff => cap.min(WINDOW),
            PredictorKind::Png => cap
                .saturating_add(cap / self.row_bytes)
                .saturating_add(1)
                .min(WINDOW),
        }
    }

    pub(crate) fn write(&mut self, bytes: &[u8], offset: usize) -> Result<()> {
        self.check(bytes.len(), offset)?;
        let at = Position::Buffer(offset);
        match self.predictor.kind() {
            PredictorKind::None => self.bytes.extend(bytes).at(at),
            PredictorKind::Tiff => {
                let mut rest = bytes;
                while !rest.is_empty() {
                    let (row, tail) =
                        rest.split_at(rest.len().min(self.row_bytes - self.row_offset));
                    self.bytes.extend(row).at(at)?;
                    self.row_offset += row.len();
                    if self.row_offset == self.row_bytes {
                        self.tiff_row();
                        self.row_offset = 0;
                    }
                    rest = tail;
                }
                Ok(())
            }
            PredictorKind::Png => {
                for &byte in bytes {
                    let Some(filter) = self.png_filter else {
                        self.png_filter = Some(PngFilter::try_from(byte).at(at)?);
                        continue;
                    };
                    let byte = byte.wrapping_add(self.png_prediction(filter));
                    self.bytes.push(byte).at(at)?;
                    self.row_offset += 1;
                    if self.row_offset == self.row_bytes {
                        self.row_offset = 0;
                        self.png_filter = None;
                    }
                }
                Ok(())
            }
        }
    }

    // Account for PNG tag bytes without charging them to the decoded cap.
    pub(crate) fn check(&self, raw: usize, offset: usize) -> Result<()> {
        let tags = if self.predictor.kind() == PredictorKind::Png {
            let stride = self
                .row_bytes
                .checked_add(1)
                .ok_or_else(|| decode_limit(offset))?;
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
        if self.bytes.fits(raw - tags) {
            Ok(())
        } else {
            Err(decode_limit(offset))
        }
    }

    fn png_prediction(&self, filter: PngFilter) -> u8 {
        let bytes = self.bytes.as_slice();
        let end = bytes.len();
        let left = if self.row_offset >= self.pixel_bytes {
            bytes.get(end - self.pixel_bytes).copied().unwrap_or(0)
        } else {
            0
        };
        let above = end
            .checked_sub(self.row_bytes)
            .and_then(|i| bytes.get(i))
            .copied()
            .unwrap_or(0);
        let upper_left = if self.row_offset >= self.pixel_bytes {
            end.checked_sub(self.row_bytes)
                .and_then(|i| i.checked_sub(self.pixel_bytes))
                .and_then(|i| bytes.get(i))
                .copied()
                .unwrap_or(0)
        } else {
            0
        };
        filter.predict(left, above, upper_left)
    }

    fn tiff_row(&mut self) {
        let bits = usize::from(self.predictor.bits_per_component());
        let colors = usize::from(self.predictor.colors());
        let row_samples = self.row_samples;
        let bytes = self.bytes.as_mut_slice();
        let start = bytes.len() - self.row_bytes;
        // The checked row layout guarantees this range; do not index input offsets.
        let Some(row) = bytes.get_mut(start..) else {
            return;
        };
        for sample in colors..row_samples {
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
        Ok(self.bytes.into_vec())
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
