//! Deferred sample layouts for painters that convert and filter only the
//! rows they need.
//!
//! [`SampleLayout::Deferred`] keeps a baseline YCbCr JPEG in its original
//! component planes ([`Planar`]) and one-bit, one-component samples in their
//! packed rows ([`Packed`]). Every other image decodes exactly as with
//! [`SampleLayout::Interleaved`].
//!
//! A deferred JPEG consumes every coefficient. Chroma and single-component
//! samples are the reference's bytes (T.81 A.3.3 in binary64, then rounding
//! and clamping). A luma block whose certified f32 error is below a quarter
//! level may instead keep the lower end of an adjacent-byte interval that
//! contains the reference byte; [`Planar::interval`] marks those blocks.
//! Table 13 conversion is 1-Lipschitz in luma except at exact green half
//! ties, so frames with such a tie are decoded again with exact luma.
mod colour;
mod packed;
mod planar;

pub use colour::{Constant, deficits, uniform, ycbcr, ycbcr8};
pub use packed::Packed;
pub use planar::Planar;

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
