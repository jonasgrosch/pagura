//! A fast hasher for the interpreter's internal maps, whose keys are
//! identities and small integers. It is deterministic and not resistant to
//! chosen collisions; every map it serves is bounded by a budget.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// A multiply-rotate hasher over 64-bit words.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FastHasher(u64);

/// An odd constant with well-mixed bits (the 64-bit golden ratio).
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

impl Hasher for FastHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for word in words {
            self.write_u64(u64::from_le_bytes(*word));
        }
        let mut last = [0; 8];
        last[..rest.len()].copy_from_slice(rest);
        self.write_u64(u64::from_le_bytes(last) ^ (rest.len() as u64));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(SEED);
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u16(&mut self, n: u16) {
        self.write_u64(u64::from(n));
    }

    fn write_u8(&mut self, n: u8) {
        self.write_u64(u64::from(n));
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// A map with [`FastHasher`].
pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;
