//! Streaming hash functions: RFC 1321 MD5 and FIPS 180-4 SHA-1 and SHA-2.
//!
//! All six are Merkle–Damgård constructions. They share one [`Engine`]: the
//! block buffer, the message length and the padding of RFC 1321 3.1–3.2 and
//! FIPS 180-4 5.1. Each algorithm supplies only its chaining state,
//! compression function, length-field encoding and output encoding.

use pagura_core::convert::usize_to_u64;

use crate::secret::wipe;

/// A streaming hash function.
pub trait Digest: Clone + Default {
    /// The digest, a fixed-size byte array.
    type Output: AsRef<[u8]> + Copy;

    /// Absorbs bytes without allocating. The message length is counted
    /// modulo the algorithm's length field.
    fn update(&mut self, data: &[u8]);

    /// Pads the message and returns its digest.
    fn finalize(self) -> Self::Output;

    /// Hashes one complete message.
    fn digest(data: &[u8]) -> Self::Output {
        let mut hash = Self::default();
        hash.update(data);
        hash.finalize()
    }
}

/// Defines a public hasher over an [`Engine`]: its initial chaining state,
/// compression function, length field and output encoding are all that
/// differ between the algorithms here.
macro_rules! hasher {
    (
        $(#[$attribute:meta])*
        $name:ident {
            state: [$word:ty; $words:literal] = $initial:expr,
            block: $block:literal,
            compress: $compress:path,
            length: $length:ident,
            output: [u8; $bytes:literal] = $encode:path $(,)?
        }
    ) => {
        $(#[$attribute])*
        #[derive(Clone)]
        pub struct $name(pub(super) super::Engine<$word, $words, $block>);

        impl $name {
            /// Initializes the chaining state.
            pub const fn new() -> Self {
                Self(super::Engine::new($initial))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        /// Never prints buffered input, which can hold password bytes.
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }

        impl super::Digest for $name {
            type Output = [u8; $bytes];

            fn update(&mut self, data: &[u8]) {
                self.0.update(data, $compress);
            }

            fn finalize(mut self) -> [u8; $bytes] {
                let length = self.0.$length();
                $encode(&self.0.finish(&length, $compress))
            }
        }
    };
}

mod md5;
mod sha1;
mod sha2;

pub use md5::Md5;
pub use sha1::Sha1;
pub use sha2::{Sha224, Sha256, Sha384, Sha512};

/// The chaining state, block buffer and message length of one hash
/// computation. State and buffer are cleared when dropped: during the
/// security handler's algorithms they hold password-derived bytes.
#[derive(Clone)]
struct Engine<W: Copy + Default, const S: usize, const N: usize> {
    state: [W; S],
    buffer: [u8; N],
    used: usize,
    bytes: u128,
}

impl<W: Copy + Default, const S: usize, const N: usize> Engine<W, S, N> {
    const fn new(state: [W; S]) -> Self {
        Self {
            state,
            buffer: [0; N],
            used: 0,
            bytes: 0,
        }
    }

    /// Compresses each completed block. Whole blocks are read straight from
    /// `data`; only a partial block is copied into the buffer. `compress` is
    /// a generic parameter, so each hasher's function is called directly.
    fn update(&mut self, mut data: &[u8], compress: impl Fn(&mut [W; S], &[u8; N])) {
        self.bytes = self
            .bytes
            .wrapping_add(u128::from(usize_to_u64(data.len())));
        if self.used != 0 {
            let (head, rest) = data.split_at((N - self.used).min(data.len()));
            self.buffer[self.used..self.used + head.len()].copy_from_slice(head);
            self.used += head.len();
            if self.used < N {
                return;
            }
            compress(&mut self.state, &self.buffer);
            self.used = 0;
            data = rest;
        }
        let (blocks, rest) = data.as_chunks::<N>();
        for block in blocks {
            compress(&mut self.state, block);
        }
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.used = rest.len();
    }

    /// Appends the 1 bit, zero bits and the `length` field (RFC 1321 3.1–3.2,
    /// FIPS 180-4 5.1), compresses the last one or two blocks and returns the
    /// final chaining state.
    fn finish(&mut self, length: &[u8], compress: impl Fn(&mut [W; S], &[u8; N])) -> [W; S] {
        let used = self.used;
        self.buffer[used] = 0x80;
        self.buffer[used + 1..].fill(0);
        if used + 1 > N - length.len() {
            compress(&mut self.state, &self.buffer);
            self.buffer.fill(0);
        }
        self.buffer[N - length.len()..].copy_from_slice(length);
        compress(&mut self.state, &self.buffer);
        self.state
    }

    /// The message length in bits, modulo 2^128.
    const fn bits(&self) -> u128 {
        self.bytes.wrapping_mul(8)
    }

    /// RFC 1321 3.2: the length modulo 2^64, low-order byte first.
    fn length_le64(&self) -> [u8; 8] {
        let bytes = self.bits().to_le_bytes();
        let mut length = [0; 8];
        length.copy_from_slice(&bytes[..8]);
        length
    }

    /// FIPS 180-4 5.1.1: the length as a big-endian 64-bit integer.
    fn length_be64(&self) -> [u8; 8] {
        let bytes = self.bits().to_be_bytes();
        let mut length = [0; 8];
        length.copy_from_slice(&bytes[8..]);
        length
    }

    /// FIPS 180-4 5.1.2: the length as a big-endian 128-bit integer.
    const fn length_be128(&self) -> [u8; 16] {
        self.bits().to_be_bytes()
    }
}

impl<W: Copy + Default, const S: usize, const N: usize> Drop for Engine<W, S, N> {
    fn drop(&mut self) {
        wipe(&mut self.state);
        wipe(&mut self.buffer);
    }
}

/// The leading `B` bytes of `words`, each word big-endian (FIPS 180-4 3.1).
fn be32<const B: usize>(words: &[u32]) -> [u8; B] {
    let mut out = [0; B];
    for (bytes, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *bytes = word.to_be_bytes();
    }
    out
}

/// The leading `B` bytes of `words`, each word big-endian (FIPS 180-4 3.1).
fn be64<const B: usize>(words: &[u64]) -> [u8; B] {
    let mut out = [0; B];
    for (bytes, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(words) {
        *bytes = word.to_be_bytes();
    }
    out
}

#[cfg(test)]
mod tests;
