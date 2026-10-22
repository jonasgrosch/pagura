//! A seeded multiply-fold hasher for the rendering caches' small keys.
//!
//! Glyph, mask and clip keys are a few machine words (addresses, matrix bits
//! and phases) looked up once or more per glyph placement. SipHash, the
//! standard library's default, costs more than the work those lookups save
//! on a cold page. Each map draws its seed from the standard library's
//! per-process random keys, so colliding keys cannot be prepared in a
//! document; iteration order is never observed.

use std::hash::{BuildHasher, Hasher, RandomState};

/// An odd 64-bit constant with well-spread bits (the golden ratio).
const MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;

/// A hash map keyed with [`Seeded`] hashing.
pub(crate) type FastMap<K, V> = std::collections::HashMap<K, V, Seeded>;

/// A hash set keyed with [`Seeded`] hashing.
pub(crate) type FastSet<K> = std::collections::HashSet<K, Seeded>;

/// Builds [`Fold`] hashers from a per-map random seed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Seeded(u64);

impl Default for Seeded {
    fn default() -> Self {
        Self(RandomState::new().hash_one(MULTIPLIER))
    }
}

impl BuildHasher for Seeded {
    type Hasher = Fold;

    fn build_hasher(&self) -> Fold {
        Fold(self.0)
    }
}

/// Folds each 64-bit word into the state with one widening multiply.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fold(u64);

#[expect(
    clippy::cast_possible_truncation,
    reason = "the high and low halves of the 128-bit product are folded together"
)]
fn fold(a: u64, b: u64) -> u64 {
    let product = u128::from(a) * u128::from(b);
    (product as u64) ^ ((product >> 64) as u64)
}

impl Hasher for Fold {
    fn write(&mut self, bytes: &[u8]) {
        let (words, tail) = bytes.as_chunks::<8>();
        for word in words {
            self.write_u64(u64::from_le_bytes(*word));
        }
        if !tail.is_empty() {
            let mut word = [0; 8];
            word[..tail.len()].copy_from_slice(tail);
            self.write_u64(u64::from_le_bytes(word) ^ ((tail.len() as u64) << 59));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = fold(self.0 ^ value, MULTIPLIER);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    fn finish(&self) -> u64 {
        fold(self.0, MULTIPLIER.rotate_left(32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_keys_hash_equally_and_nearby_keys_spread() {
        let seeded = Seeded::default();
        let key = (7_usize, [1.5_f64.to_bits(), 0, 0, 1.5_f64.to_bits()]);
        assert_eq!(seeded.hash_one(key), seeded.hash_one(key));
        let buckets: FastSet<u64> = (0..4096_u64).map(|n| seeded.hash_one(n) >> 52).collect();
        // 4096 consecutive keys over 4096 top-bit buckets fill most of them.
        assert!(buckets.len() > 2048, "{}", buckets.len());
        let tails = [&[1_u8][..], &[1, 0], &[1, 0, 0]];
        let tails: FastSet<u64> = tails.iter().map(|t| seeded.hash_one(t)).collect();
        assert_eq!(tails.len(), 3);
    }
}
