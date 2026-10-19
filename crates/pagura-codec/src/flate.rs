//! Zlib/DEFLATE, RFC 1950 2.2/2.3 and RFC 1951 3.2.
//!
//! Decoding (`inflate`) expands each canonical Huffman code into a lookup
//! table of at most 2^10 entries: codes up to 10 bits take one lookup, and
//! longer ones a short canonical search. The fixed code's tables are built
//! once per stream. Backrefs read predictor-free output directly; predictors
//! retain a bounded circular window of pre-predictor bytes (at most 32 KiB).
//! [`compress`] (`deflate`) chooses stored, fixed or dynamic blocks with
//! bounded LZ77 work (`matcher`). Both directions share the RFC 1951
//! tables (`tables`), the canonical code assignment (`huffman`), the bit
//! packing (`bits`) and the zlib framing (`zlib`).
use pagura_core::{Limits, Result};

mod bits;
mod deflate;
mod huffman;
mod inflate;
mod matcher;
mod tables;
mod zlib;

pub(crate) use inflate::{decode, raw};
pub(crate) use tables::WINDOW;

/// The shortest zlib stream: header, an empty fixed block (ten bits padded
/// to two bytes) and the Adler-32 trailer.
pub(crate) const MIN_STREAM: usize = zlib::HEADER + 2 + zlib::TRAILER;

/// Compression effort. Both modes emit deterministic RFC 1950 zlib streams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Level {
    /// Short hash-chain searches and greedy matches.
    Fast,
    /// Deeper searches with one-byte lazy matching.
    #[default]
    Default,
}

/// Compresses bytes to zlib (RFC 1950 2.2) containing DEFLATE (RFC 1951 3.2).
///
/// Uses at most a 32 KiB history and 65,535-byte blocks. Search depth and match
/// length are bounded, so work is linear in input length with level-dependent
/// constants. Scratch storage is bounded independently of large input sizes.
/// `Limits::decoded_bytes` bounds input and `Limits::stream_bytes` bounds output.
/// Empty input produces a complete stream; no preset dictionary is used.
///
/// No product path calls this yet; ADR 0020 records the intended consumer
/// (Flate-encoded streams written by `pagura-write`).
///
/// # Errors
///
/// Returns a `Limit` error, without a position, before exceeding either
/// byte budget or when bounded scratch/output storage cannot be allocated.
///
/// # Examples
///
/// ```
/// use pagura_codec::flate::{compress, Level};
/// use pagura_core::Limits;
/// let zlib = compress(b"PDF stream bytes", Level::Default, &Limits::default())?;
/// assert!(zlib.len() >= 8);
/// # Ok::<(), pagura_core::Error>(())
/// ```
pub fn compress(data: &[u8], level: Level, limits: &Limits) -> Result<Vec<u8>> {
    deflate::compress(data, level, limits)
}
