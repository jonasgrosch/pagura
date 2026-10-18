//! SHA-1 for verification (FIPS 180-4 section 6.1).
//!
//! SHA-1 is retained for checking historical signatures. Its mathematical
//! verification must not be confused with acceptance by a security policy.

use super::be32;

hasher! {
    /// Streaming SHA-1 (FIPS 180-4 6.1). SHA-1 is cryptographically weak.
    Sha1 {
        // FIPS 180-4 5.3.1.
        state: [u32; 5] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0],
        block: 64,
        compress: compress,
        length: length_be64,
        output: [u8; 20] = be32,
    }
}

/// FIPS 180-4 6.1.2: one 512-bit block.
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
