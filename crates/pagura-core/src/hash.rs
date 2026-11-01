//! Deterministic hashing for bounded tables of internal identities, enums
//! and numeric bits. This non-cryptographic hasher does not resist chosen
//! collisions; callers bound each table with its domain's work and memory
//! budgets. Keys still use their complete equality comparison.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// A [`HashMap`] with [`FastHasher`].
pub type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

/// A [`HashSet`] with [`FastHasher`].
pub type FastSet<T> = HashSet<T, BuildHasherDefault<FastHasher>>;

/// Add-multiply hashing of whole words, rotated at the end so that the
/// well-mixed high bits of the last product reach the bucket index.
#[derive(Debug, Clone, Copy, Default)]
pub struct FastHasher {
    hash: u64,
}

/// An odd multiplier with well-spread bits.
const MULTIPLIER: u64 = 0xf135_7aea_2e62_a9c5;

impl FastHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = self.hash.wrapping_add(word).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.add(u64::from_le_bytes(*chunk));
        }
        let mut last = [0_u8; 8];
        if let Some(tail) = last.get_mut(..rest.len()) {
            tail.copy_from_slice(rest);
            self.add(u64::from_le_bytes(last) ^ (rest.len() as u64) << 56);
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash.rotate_left(26)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasher, Hash};

    fn hash<T: Hash>(value: &T) -> u64 {
        BuildHasherDefault::<FastHasher>::default().hash_one(value)
    }

    #[test]
    fn equal_keys_hash_equally_and_nearby_keys_spread() {
        assert_eq!(
            hash(&(3_u32, [1.5_f64.to_bits(); 6])),
            hash(&(3_u32, [1.5_f64.to_bits(); 6]))
        );
        let mut buckets = [0_u32; 64];
        for id in 0..64_000_u32 {
            let slot = usize::try_from(hash(&(id, 7_u8)) >> 58).unwrap();
            buckets[slot] += 1;
        }
        assert!(
            buckets.iter().all(|&n| (800..1200).contains(&n)),
            "{buckets:?}"
        );
        let mut low = [0_u32; 64];
        for id in 0..64_000_u32 {
            let slot = usize::try_from(hash(&id) & 63).unwrap();
            low[slot] += 1;
        }
        assert!(low.iter().all(|&n| (800..1200).contains(&n)), "{low:?}");
    }

    #[test]
    fn maps_iterate_in_one_order_for_one_insertion_order() {
        let fill = || {
            let mut map = FastMap::default();
            for id in (0..500_u32).rev() {
                map.insert((id, id % 3), id);
            }
            map.into_iter().collect::<Vec<_>>()
        };
        assert_eq!(fill(), fill());
        let mut keys = FastSet::default();
        assert!(keys.insert(5_u32));
        assert!(!keys.insert(5_u32));
    }
}
