//! Montgomery arithmetic with at most nine little-endian u64 limbs.
//!
//! A residue is always canonical, below its modulus, in the active limbs;
//! unused limbs are zero. Field and scalar moduli are odd named primes. The
//! double-width product has one extra limb for reduction carry propagation.

use std::cmp::Ordering;

use crate::bigint::{montgomery_factor, montgomery_multiply};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Uint(pub(crate) [u64; 9]);

impl Uint {
    pub(crate) const ZERO: Self = Self([0; 9]);
    pub(crate) const ONE: Self = Self([1, 0, 0, 0, 0, 0, 0, 0, 0]);

    pub(crate) fn from_be_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 72 {
            return None;
        }
        let mut out = Self::ZERO;
        for (i, &byte) in bytes.iter().rev().enumerate() {
            out.0[i / 8] |= u64::from(byte) << (8 * (i % 8));
        }
        Some(out)
    }

    pub(crate) const fn ge(self, other: Self) -> bool {
        let mut i = 9;
        while i != 0 {
            i -= 1;
            if self.0[i] != other.0[i] {
                return self.0[i] > other.0[i];
            }
        }
        true
    }

    pub(crate) fn bit(self, index: usize) -> bool {
        self.0[index / 64] & (1 << (index % 64)) != 0
    }

    pub(crate) const fn is_zero(self) -> bool {
        let mut i = 0;
        while i < 9 {
            if self.0[i] != 0 {
                return false;
            }
            i += 1;
        }
        true
    }

    // The active-limb subtraction wraps; callers account for the borrow.
    pub(crate) const fn sub(self, other: Self, limbs: usize) -> (Self, bool) {
        let mut out = Self::ZERO;
        let mut borrow = false;
        let mut i = 0;
        while i < limbs {
            let (difference, first) = self.0[i].overflowing_sub(other.0[i]);
            let (difference, second) = difference.overflowing_sub(borrow as u64);
            out.0[i] = difference;
            borrow = first || second;
            i += 1;
        }
        (out, borrow)
    }

    pub(crate) fn shr(self, bits: usize) -> Self {
        debug_assert!((1..64).contains(&bits));
        let mut out = Self::ZERO;
        for i in 0..9 {
            out.0[i] = self.0[i] >> bits;
            if i + 1 < 9 {
                out.0[i] |= self.0[i + 1] << (64 - bits);
            }
        }
        out
    }
}

impl PartialOrd for Uint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Uint {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

/// A fixed named prime and its compile-time Montgomery constants.
#[derive(Debug)]
pub(crate) struct Modulus {
    pub(crate) value: Uint,
    pub(crate) one: Uint,
    limbs: usize,
    factor: u64,
    r_squared: Uint,
}

impl Modulus {
    pub(crate) const fn new(value: Uint, limbs: usize) -> Self {
        let mut modulus = Self {
            value,
            one: Uint::ONE,
            limbs,
            factor: montgomery_factor(value.0[0]),
            r_squared: Uint::ZERO,
        };
        let mut power = Uint::ONE;
        let mut i = 0;
        while i < 128 * limbs {
            power = modulus.add(power, power);
            i += 1;
            if i == 64 * limbs {
                modulus.one = power;
            }
        }
        modulus.r_squared = power;
        modulus
    }

    /// Adds canonical residues in either ordinary or Montgomery form.
    pub(crate) const fn add(&self, a: Uint, b: Uint) -> Uint {
        let mut sum = Uint::ZERO;
        let mut carry = false;
        let mut i = 0;
        while i < self.limbs {
            let (word, first) = a.0[i].overflowing_add(b.0[i]);
            let (word, second) = word.overflowing_add(carry as u64);
            sum.0[i] = word;
            carry = first || second;
            i += 1;
        }
        if carry || sum.ge(self.value) {
            sum.sub(self.value, self.limbs).0
        } else {
            sum
        }
    }

    pub(crate) fn sub(&self, a: Uint, b: Uint) -> Uint {
        let (difference, borrow) = a.sub(b, self.limbs);
        if borrow {
            // Wrapping addition of p cancels the active-limb underflow.
            let mut out = Uint::ZERO;
            let mut carry = false;
            for (i, word) in out.0.iter_mut().enumerate().take(self.limbs) {
                let (sum, first) = difference.0[i].overflowing_add(self.value.0[i]);
                let (sum, second) = sum.overflowing_add(u64::from(carry));
                *word = sum;
                carry = first || second;
            }
            out
        } else {
            difference
        }
    }

    /// Computes a*b/R mod p (Montgomery multiplication).
    pub(crate) fn mul(&self, a: Uint, b: Uint) -> Uint {
        let limbs = self.limbs;
        let mut scratch = [0_u64; 19];
        let mut result = Uint::ZERO;
        montgomery_multiply(
            &a.0[..limbs],
            &b.0[..limbs],
            &self.value.0[..limbs],
            self.factor,
            &mut scratch[..=2 * limbs],
            &mut result.0[..limbs],
        );
        result
    }

    pub(crate) fn encode(&self, value: Uint) -> Uint {
        self.mul(value, self.r_squared)
    }

    pub(crate) fn decode(&self, value: Uint) -> Uint {
        self.mul(value, Uint::ONE)
    }

    pub(crate) fn square(&self, value: Uint) -> Uint {
        self.mul(value, value)
    }

    pub(crate) fn pow(&self, value: Uint, exponent: Uint) -> Uint {
        let mut result = self.one;
        for i in (0..64 * self.limbs).rev() {
            result = self.square(result);
            if exponent.bit(i) {
                result = self.mul(result, value);
            }
        }
        result
    }

    /// Fermat inversion; zero has no inverse and is explicitly rejected.
    pub(crate) fn inverse(&self, value: Uint) -> Option<Uint> {
        if value.is_zero() {
            return None;
        }
        let two = self.add(Uint::ONE, Uint::ONE);
        Some(self.pow(value, self.value.sub(two, self.limbs).0))
    }

    /// All three field primes equal 3 mod 4. The candidate must be checked.
    pub(crate) fn sqrt(&self, value: Uint) -> Option<Uint> {
        // (p+1)/4 = floor(p/4)+1, without a full-width p+1 overflow.
        let exponent = self.add(self.value.shr(2), Uint::ONE);
        let root = self.pow(value, exponent);
        (self.square(root) == value).then_some(root)
    }

    /// Reduces a value less than 2*p (coordinates and truncated hashes).
    pub(crate) fn reduce_once(&self, value: Uint) -> Uint {
        if value.ge(self.value) {
            value.sub(self.value, self.limbs).0
        } else {
            value
        }
    }
}
