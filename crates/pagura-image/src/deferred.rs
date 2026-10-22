//! Deferred sample layouts for painters that convert and filter only the
//! rows they need.
//!
//! [`Layout::Native`](crate::Layout::Native) keeps a baseline YCbCr JPEG in
//! its original component planes ([`Planar`]) and one-bit, one-component
//! samples in their packed rows ([`Packed`]). Every other image decodes
//! exactly as with [`Layout::Interleaved`](crate::Layout::Interleaved).
//!
//! Deferred samples are the decoders' samples: a deferred JPEG consumes every
//! coefficient, and each component sample is the reference's byte (T.81
//! A.3.3 in binary64, then rounding and clamping). Only the layout differs.
mod colour;
mod packed;
mod planar;

pub(crate) use colour::floor_byte;
pub use colour::ycbcr;
pub use packed::Packed;
pub use planar::Planar;

/// How an image's samples may be represented after decoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum SampleLayout {
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
    /// As [`Self::Deferred`], with an eight-bit baseline JPEG reconstructed
    /// at this many cells per block axis as [`Self::Cells`] reconstructs it,
    /// in native planes of cells.
    DeferredCells(u8),
}

impl SampleLayout {
    /// JPEG samples kept per eight-sample block axis in this layout.
    pub(crate) fn cells(self) -> u8 {
        match self {
            Self::Cells(cells) | Self::DeferredCells(cells) => cells,
            Self::Interleaved | Self::Deferred => 8,
        }
    }

    /// Whether native layouts are kept where they apply.
    pub(crate) fn deferred(self) -> bool {
        matches!(self, Self::Deferred | Self::DeferredCells(_))
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
