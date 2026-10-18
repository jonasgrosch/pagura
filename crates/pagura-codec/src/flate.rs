//! Zlib/DEFLATE, RFC 1950 sections 2.2/2.3 and RFC 1951 section 3.2.
//! [`compress`] chooses stored, fixed or dynamic blocks with bounded LZ77 work.
//! Canonical Huffman codes are expanded into direct lookup tables. Backrefs
//! read predictor-free output directly; predictors retain a bounded circular
//! window of pre-predictor bytes (at most 32 KiB).
use pagura_core::Result;

mod adler;
mod deflate;
mod huffman;
mod inflate;
mod matcher;
mod tables;

pub(crate) use inflate::{decode, raw};

/// Compression effort. Both modes emit deterministic RFC 1950 zlib streams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
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
/// # Errors
///
/// Returns a positioned `Limit` error before exceeding either byte budget or
/// when bounded scratch/output storage cannot be allocated.
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
pub fn compress(data: &[u8], level: Level, limits: &pagura_core::Limits) -> Result<Vec<u8>> {
    deflate::compress(data, level, limits)
}
