//! Bounded output and incremental predictor reversal (ISO 32000-2 7.4.4.4).
//! PNG arithmetic: Second Edition 9.2, Table 9.1, 9.3 and 9.4. TIFF sample
//! arithmetic: TIFF 6.0 section 14, pp.64-65; PDF supplies high-bit-first packing.
use crate::{Predictor, PredictorKind, syntax};
use pagura_core::{Diagnostics, Result};

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
            .ok_or_else(|| crate::limit(0))?;
        let row_bytes = row_samples
            .checked_mul(usize::from(predictor.bits_per_component))
            .map(|bits| bits.div_ceil(8))
            .ok_or_else(|| crate::limit(0))?;
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

    /// Output bytes the cap still allows.
    pub(crate) fn room(&self) -> usize {
        self.cap.saturating_sub(self.bytes.len())
    }

    /// Whether `bytes`, written next, carry only valid PNG row tags.
    pub(crate) fn valid_tags(&self, bytes: &[u8]) -> bool {
        if self.predictor.kind != PredictorKind::Png {
            return true;
        }
        let first = if self.png_filter.is_none() {
            0
        } else {
            self.row_bytes - self.row_offset
        };
        bytes
            .iter()
            .skip(first)
            .step_by(self.row_bytes.saturating_add(1))
            .all(|&tag| tag <= 4)
    }

    pub(crate) fn write(&mut self, mut bytes: &[u8], offset: usize) -> Result<()> {
        self.check(bytes.len(), offset)?;
        if self.predictor.kind == PredictorKind::None {
            self.reserve(bytes.len(), offset)?;
            self.bytes.extend_from_slice(bytes);
            return Ok(());
        }
        // A row at a time: tags first, then the row's bytes in one segment.
        while let Some((&tag, rest)) = bytes.split_first() {
            if self.predictor.kind == PredictorKind::Png && self.png_filter.is_none() {
                if tag > 4 {
                    return Err(syntax("invalid PNG predictor tag", offset));
                }
                self.png_filter = Some(tag);
                bytes = rest;
                continue;
            }
            let (segment, rest) = bytes.split_at(bytes.len().min(self.row_bytes - self.row_offset));
            self.grow(segment.len(), offset)?;
            if self.predictor.kind == PredictorKind::Png {
                self.png_segment(segment);
            } else {
                self.bytes.extend_from_slice(segment);
            }
            self.row_offset += segment.len();
            if self.row_offset == self.row_bytes {
                if self.predictor.kind == PredictorKind::Tiff {
                    self.tiff_row();
                }
                self.row_offset = 0;
                self.png_filter = None;
            }
            bytes = rest;
        }
        Ok(())
    }

    // Account for PNG tag bytes without charging them to the decoded cap.
    pub(crate) fn check(&self, raw: usize, offset: usize) -> Result<()> {
        let tags = if self.predictor.kind == PredictorKind::Png {
            let stride = self
                .row_bytes
                .checked_add(1)
                .ok_or_else(|| crate::limit(offset))?;
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
            .ok_or_else(|| crate::limit(offset))?;
        Ok(())
    }

    fn reserve(&mut self, count: usize, offset: usize) -> Result<()> {
        let required = self
            .bytes
            .len()
            .checked_add(count)
            .filter(|&n| n <= self.cap)
            .ok_or_else(|| crate::limit(offset))?;
        if required > self.bytes.capacity() {
            let target = required
                .max(self.bytes.capacity().saturating_mul(2).max(64))
                .min(self.cap);
            self.bytes
                .try_reserve_exact(target - self.bytes.len())
                .map_err(|_| crate::limit(offset))?;
        }
        Ok(())
    }

    /// Reserves `count` more bytes, to the capacity reserving them one at a
    /// time would reach; callers count retained capacity.
    fn grow(&mut self, count: usize, offset: usize) -> Result<()> {
        let needed = self
            .bytes
            .len()
            .checked_add(count)
            .filter(|&n| n <= self.cap)
            .ok_or_else(|| crate::limit(offset))?;
        let mut target = self.bytes.capacity();
        while target < needed {
            target = target.saturating_mul(2).max(64).min(self.cap);
        }
        if target > self.bytes.capacity() {
            self.bytes
                .try_reserve_exact(target - self.bytes.len())
                .map_err(|_| crate::limit(offset))?;
        }
        Ok(())
    }

    /// Reverses the current row's PNG filter for the row's next bytes.
    fn png_segment(&mut self, segment: &[u8]) {
        let start = self.bytes.len();
        let row = start - self.row_offset;
        let (stride, step) = (self.row_bytes, self.pixel_bytes);
        let filter = self.png_filter.unwrap_or(0);
        if filter == 0 || (row == 0 && filter == 2) {
            self.bytes.extend_from_slice(segment);
            return;
        }
        if filter == 2 {
            // Up: no dependence within the row.
            self.bytes
                .extend_from_within(start - stride..start - stride + segment.len());
            for (byte, &raw) in self.bytes[start..].iter_mut().zip(segment) {
                *byte = byte.wrapping_add(raw);
            }
            return;
        }
        self.bytes.extend_from_slice(segment);
        let (from, to) = (start - row, self.bytes.len() - row);
        let (before, current) = self.bytes.split_at_mut(row);
        let previous = before
            .get(row.saturating_sub(stride)..)
            .filter(|_| row != 0);
        // Byte by byte up to the second whole pixel, then whole pixels of
        // common widths, then the remaining bytes.
        let head = from.max(step).next_multiple_of(step).min(to);
        unfilter_bytes(filter, current, previous, step, from, head);
        let body = head + (to - head) / step * step;
        let done = match step {
            1 => unfilter_pixels::<1>(filter, current, previous, head, body),
            2 => unfilter_pixels::<2>(filter, current, previous, head, body),
            3 => unfilter_pixels::<3>(filter, current, previous, head, body),
            4 => unfilter_pixels::<4>(filter, current, previous, head, body),
            6 => unfilter_pixels::<6>(filter, current, previous, head, body),
            8 => unfilter_pixels::<8>(filter, current, previous, head, body),
            _ => head,
        };
        unfilter_bytes(filter, current, previous, step, done, to);
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

    /// The output, whose last predictor row must be complete; an incomplete
    /// one is damaged data (`filter.corrupt`) and is discarded.
    pub(crate) fn finish(self, offset: usize, diagnostics: &mut Diagnostics) -> Result<Vec<u8>> {
        if self.row_offset != 0 || self.png_filter.is_some() {
            return crate::damaged(
                syntax("incomplete predictor row", offset),
                diagnostics,
                || self.complete(),
            );
        }
        Ok(self.bytes)
    }

    /// The whole predictor rows written so far.
    pub(crate) fn complete(mut self) -> Vec<u8> {
        let whole = self.bytes.len() - self.row_offset;
        self.bytes.truncate(whole);
        self.bytes
    }
}

/// Reverses PNG filter 1, 3 or 4 (PNG Second Edition 9.2) for `current[from..to]`,
/// one byte at a time; `previous` is the row above, absent for the first row.
fn unfilter_bytes(
    filter: u8,
    current: &mut [u8],
    previous: Option<&[u8]>,
    step: usize,
    from: usize,
    to: usize,
) {
    for index in from..to {
        let left = index
            .checked_sub(step)
            .and_then(|i| current.get(i))
            .copied()
            .unwrap_or(0);
        let above = previous
            .and_then(|row| row.get(index))
            .copied()
            .unwrap_or(0);
        let upper_left = index
            .checked_sub(step)
            .and_then(|i| previous.and_then(|row| row.get(i)))
            .copied()
            .unwrap_or(0);
        if let Some(byte) = current.get_mut(index) {
            *byte = byte.wrapping_add(predict(filter, left, above, upper_left));
        }
    }
}

/// [`unfilter_bytes`] for whole `STEP`-byte pixels from `from` (a multiple
/// of `STEP`, at least one pixel in) to `to`, keeping the left pixel in
/// registers. Returns where it stopped.
fn unfilter_pixels<const STEP: usize>(
    filter: u8,
    current: &mut [u8],
    previous: Option<&[u8]>,
    from: usize,
    to: usize,
) -> usize {
    let (done, rest) = current.split_at_mut(from.min(current.len()));
    let (Some(&first), Some(pixels)) = (
        done.last_chunk::<STEP>(),
        rest.get_mut(..to.saturating_sub(from)),
    ) else {
        return from;
    };
    let mut left = first;
    let (pixels, _) = pixels.as_chunks_mut::<STEP>();
    match previous.and_then(|row| row.get(from - STEP..to)) {
        Some(row) => {
            let above = row.get(STEP..).unwrap_or_default().as_chunks::<STEP>().0;
            let corner = row.as_chunks::<STEP>().0;
            for ((pixel, above), corner) in pixels.iter_mut().zip(above).zip(corner) {
                // Built in registers, so the next pixel need not reload it.
                left = std::array::from_fn(|channel| {
                    pixel[channel].wrapping_add(predict(
                        filter,
                        left[channel],
                        above[channel],
                        corner[channel],
                    ))
                });
                *pixel = left;
            }
        }
        None => {
            for pixel in pixels {
                left = std::array::from_fn(|channel| {
                    pixel[channel].wrapping_add(predict(filter, left[channel], 0, 0))
                });
                *pixel = left;
            }
        }
    }
    to
}

#[inline]
fn predict(filter: u8, left: u8, above: u8, upper_left: u8) -> u8 {
    match filter {
        1 => left,
        2 => above,
        3 => (left & above) + ((left ^ above) >> 1),
        4 => paeth(left, above, upper_left),
        _ => 0,
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
