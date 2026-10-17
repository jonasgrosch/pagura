//! Unicode 3.2 NFKC boundary (RFC 4013 section 2.2; RFC 3454 section 4).
use pagura_core::{Error, Result};
use std::borrow::Cow;

/// Normalizes mapped text to Unicode 3.2 NFKC, borrowing when unchanged.
///
/// Only ASCII can currently be established as unchanged without the missing
/// Unicode 3.2 normalization data. Non-ASCII, even if already normalized, is
/// explicitly unsupported rather than guessed from a newer Unicode version.
/// The future implementation must decompose, reorder and compose the complete
/// mapped string before output validation and the PDF's 127-byte truncation.
pub(super) fn nfkc32(input: Cow<'_, str>) -> Result<Cow<'_, str>> {
    if input.is_ascii() {
        Ok(input)
    } else {
        Err(Error::unsupported(
            super::super::SASLPREP,
            "revision 6 password requires Unicode 3.2 NFKC normalization data",
        ))
    }
}
