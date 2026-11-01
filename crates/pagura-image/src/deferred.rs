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

pub use colour::ycbcr;
pub(crate) use colour::{floor_byte, offset};
pub use packed::Packed;
pub use planar::Planar;

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
