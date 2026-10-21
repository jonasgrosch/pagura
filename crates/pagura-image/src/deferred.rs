//! Deferred sample layouts for painters that convert and filter only the
//! rows they need.
//!
//! [`SampleLayout::Deferred`] keeps a baseline YCbCr JPEG in its original
//! component planes ([`Planar`]) and one-bit, one-component samples in their
//! packed rows ([`Packed`]). Every other image decodes exactly as with
//! [`SampleLayout::Interleaved`].
//!
//! Deferred samples are the decoders' samples: a deferred JPEG consumes every
//! coefficient, and each component sample is the reference's byte (T.81
//! A.3.3 in binary64, then rounding and clamping). Only the layout differs.
mod colour;
mod packed;
mod planar;

pub use colour::{Constant, deficits, floor_byte, uniform, ycbcr, ycbcr8};
pub use packed::Packed;
pub use planar::Planar;

/// Sample storage kept between decodes, so that the planes of the next
/// deferred image reuse memory that is already mapped instead of faulting
/// in fresh pages. It keeps the planes of one image at a time.
#[derive(Debug, Default)]
pub struct SampleBuffers {
    free: std::sync::Mutex<Vec<Vec<u8>>>,
}

impl SampleBuffers {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps `planar`'s component planes for later decodes, replacing any
    /// kept before, unless they would exceed `max_bytes` together.
    pub fn recycle(&self, planar: Planar, max_bytes: usize) {
        let planes = planar.planes;
        let bytes = planes
            .iter()
            .fold(0usize, |n, plane| n.saturating_add(plane.capacity()));
        let mut free = self.lock();
        free.clear();
        if bytes <= max_bytes {
            free.extend(planes);
        }
    }

    /// Heap bytes currently kept.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.lock()
            .iter()
            .fold(0usize, |n, buffer| n.saturating_add(buffer.capacity()))
    }

    /// `length` copies of `value`, in a kept buffer that is large enough but
    /// at most twice as large, or in a new allocation.
    pub(crate) fn filled(&self, length: usize, value: u8) -> Option<Vec<u8>> {
        let mut free = self.lock();
        let fit = free
            .iter()
            .enumerate()
            .filter(|(_, buffer)| (length..=length.saturating_mul(2)).contains(&buffer.capacity()))
            .min_by_key(|(_, buffer)| buffer.capacity())
            .map(|(index, _)| index);
        let mut buffer = if let Some(index) = fit {
            free.swap_remove(index)
        } else {
            drop(free);
            let mut buffer = Vec::new();
            buffer.try_reserve_exact(length).ok()?;
            buffer
        };
        buffer.clear();
        buffer.resize(length, value);
        Some(buffer)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Vec<u8>>> {
        self.free
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// How an image's samples may be represented after decoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SampleLayout {
    /// Interleaved samples, byte for byte as the decoders define them.
    #[default]
    Interleaved,
    /// Baseline YCbCr JPEG frames as [`Samples::Planar`](crate::Samples::Planar)
    /// and one-bit, one-component samples as
    /// [`Samples::Packed`](crate::Samples::Packed); other images are
    /// interleaved.
    Deferred,
    /// Interleaved samples, with an eight-bit DCTDecode image reconstructed
    /// at this many cells per eight-sample block axis (1, 2, 3, 4 or 6; see
    /// [`dct::decode_cells`](crate::dct::decode_cells)) and its
    /// [`Reduction`](crate::Reduction) recorded: an approximate path for
    /// images drawn smaller than their samples (ADR 0004). Other images,
    /// and eight cells, are interleaved exactly.
    Cells(u8),
}

impl SampleLayout {
    /// JPEG samples kept per eight-sample block axis in this layout.
    pub(crate) fn cells(self) -> u8 {
        match self {
            Self::Cells(cells) => cells,
            Self::Interleaved | Self::Deferred => 8,
        }
    }
}

/// Heap bytes retained by `samples`, including padded component planes,
/// block flags and row padding.
#[must_use]
pub fn storage_bytes(samples: &crate::Samples) -> usize {
    match samples {
        crate::Samples::Bytes(values) => values.capacity(),
        crate::Samples::Words(values) => values.capacity().saturating_mul(2),
        crate::Samples::Mixed { values, bits } => values
            .capacity()
            .saturating_mul(2)
            .saturating_add(bits.capacity()),
        crate::Samples::Packed(packed) => packed.storage_bytes(),
        crate::Samples::Planar(planar) => planar.storage_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Planar, SampleBuffers};

    fn planar() -> Planar {
        Planar::new(
            (8, 8),
            [1, 1],
            [[1, 1, 8]; 3],
            [vec![255; 64], vec![128; 64], vec![128; 64]],
        )
        .unwrap()
    }

    #[test]
    fn kept_planes_are_reused_only_when_they_fit_the_budget_and_size() {
        let buffers = SampleBuffers::new();
        buffers.recycle(planar(), 191);
        assert_eq!(buffers.retained_bytes(), 0);
        buffers.recycle(planar(), 192);
        assert_eq!(buffers.retained_bytes(), 192);
        // Too small, then too large to reuse: fresh allocations.
        assert_eq!(buffers.filled(65, 1).unwrap(), vec![1; 65]);
        assert_eq!(buffers.filled(31, 2).unwrap(), vec![2; 31]);
        assert_eq!(buffers.retained_bytes(), 192);
        // A fitting request takes a kept buffer and refills it.
        assert_eq!(buffers.filled(40, 3).unwrap(), vec![3; 40]);
        assert_eq!(buffers.retained_bytes(), 128);
        // Recycling replaces whatever was kept.
        buffers.recycle(planar(), usize::MAX);
        assert_eq!(buffers.retained_bytes(), 192);
    }
}
