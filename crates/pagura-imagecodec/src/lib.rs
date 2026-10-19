#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Image codecs behind PDF's image filters (ISO 32000-2 clauses 7.4.6–7.4.9;
//! ADR 0010 and ADR 0027): DCT (JPEG, ITU-T T.81), CCITT fax (T.4 and T.6),
//! JBIG2 (T.88) and JPEG 2000 (T.800).
//!
//! The codecs read encoded bytes and return samples. They know nothing about
//! PDF objects: `pagura-image` reads image dictionaries, maps their
//! parameters onto the codecs and places codec positions in the document.

mod budget;
pub mod ccitt;
pub mod dct;
pub mod features;
pub mod jbig2;
pub mod jpx;
mod mq;
pub mod repairs;
mod t4;

use pagura_core::convert::usize_to_u64;
use pagura_core::{Error, Limits, Result};

/// The number of sample values in an image of `width × height` pixels with
/// `components` values each, after checking it against the pixel budget and
/// its unpacked size (one byte per value up to 8 bits, two above) against
/// `decoded_bytes`.
///
/// # Errors
///
/// A `Limit` error when the image exceeds either budget or the address space.
pub fn sample_count(
    width: u32,
    height: u32,
    components: u8,
    bits: u8,
    limits: &Limits,
) -> Result<usize> {
    let pixels = u64::from(width) * u64::from(height);
    let count = pixels
        .checked_mul(u64::from(components))
        .and_then(|v| usize::try_from(v).ok())
        .ok_or(Error::limit("image sample size overflow"))?;
    if pixels > usize_to_u64(limits.pixels)
        || count
            .checked_mul(if bits > 8 { 2 } else { 1 })
            .is_none_or(|bytes| bytes > limits.decoded_bytes)
    {
        return Err(Error::limit("image sample budget exceeded"));
    }
    Ok(count)
}
