//! SHA-1 for verification (FIPS 180-4 section 6.1).
//!
//! SHA-1 is retained for checking historical signatures. Its mathematical
//! verification must not be confused with acceptance by a security policy.

use super::absorb;

/// Streaming SHA-1 (FIPS 180-4). SHA-1 is cryptographically weak.
#[derive(Clone, Debug)]
pub struct Sha1 {
    state: [u32; 5],
    buffer: [u8; 64],
    used: usize,
    bytes: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    /// Initializes the chaining state of FIPS 180-4 section 5.3.1.
    pub const fn new() -> Self {
        Self {
            state: [
                0x6745_2301,
                0xefcd_ab89,
                0x98ba_dcfe,
                0x1032_5476,
                0xc3d2_e1f0,
            ],
            buffer: [0; 64],
            used: 0,
            bytes: 0,
        }
    }

    /// Absorbs bytes without allocating, counting length modulo 2^64 bits.
    pub fn update(&mut self, data: &[u8]) {
        self.bytes = self.bytes.wrapping_add(data.len() as u64);
        absorb(&mut self.buffer, &mut self.used, data, |block| {
            compress(&mut self.state, block);
        });
    }

    /// Appends the padding of FIPS 180-4 section 5.1.1 and returns the digest.
    pub fn finalize(mut self) -> [u8; 20] {
        let bits = self.bytes.wrapping_mul(8).to_be_bytes();
        let count = if self.used < 56 {
            56 - self.used
        } else {
            120 - self.used
        };
        let mut padding = [0; 64];
        padding[0] = 0x80;
        self.update(&padding[..count]);
        self.update(&bits);
        let mut digest = [0; 20];
        for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *bytes = word.to_be_bytes();
        }
        digest
    }
}

/// Computes SHA-1 (FIPS 180-4). Use only to verify historical signatures.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut hash = Sha1::new();
    hash.update(data);
    hash.finalize()
}

fn compress(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut words = [0u32; 80];
    for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for i in 16..80 {
        words[i] = (words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (i, word) in words.into_iter().enumerate() {
        // FIPS 180-4 sections 4.1.1 and 4.2.1.
        let (f, k) = match i {
            0..20 => ((b & c) | (!b & d), 0x5a82_7999),
            20..40 => (b ^ c ^ d, 0x6ed9_eba1),
            40..60 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
            _ => (b ^ c ^ d, 0xca62_c1d6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }
    for (word, value) in state.iter_mut().zip([a, b, c, d, e]) {
        *word = word.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests;
