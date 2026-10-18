//! Bounded modular arithmetic on public integers.
//!
//! The representation uses little-endian radix-2^64 limbs. Odd moduli use
//! Montgomery reduction: after a product, cancel each low limb by adding a
//! multiple of the modulus, then divide by R = 2^(64 * limbs). All residues
//! stay below the modulus; reduction needs at most one final subtraction.
//! This module is variable-time and must never process private keys.

use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Meter, Position, Result};

/// Hard allocation ceiling for verification arithmetic, in bits.
/// A caller's limit may lower this ceiling, but cannot raise it.
pub const MAX_MODULUS_BITS: usize = 16_384;

/// An odd modulus with reusable Montgomery constants, for public data only.
#[derive(Clone, Debug)]
pub(crate) struct Modulus {
    limbs: Vec<u64>,
    factor: u64,
    r_squared: Vec<u64>,
    bits: usize,
}

impl Modulus {
    /// Constructs an odd modulus greater than one from unsigned big-endian bytes.
    /// `max_bits` is checked before allocating. Leading zero bytes are rejected.
    ///
    /// # Errors
    /// Returns `Syntax` for a noncanonical, even or trivial modulus, or `Limit`
    /// when it exceeds `max_bits` or [`MAX_MODULUS_BITS`].
    pub(crate) fn new(bytes: &[u8], max_bits: usize) -> Result<Self> {
        let bits = bit_length(bytes, max_bits.min(MAX_MODULUS_BITS))?;
        if bits < 2 || bytes.last().is_none_or(|byte| byte & 1 == 0) {
            return Err(Error::syntax("modulus must be odd and greater than one")
                .at(Position::Buffer(bytes.len().saturating_sub(1))));
        }
        let limbs = from_bytes(bytes, bits.div_ceil(64));
        let factor = montgomery_factor(limbs[0]);
        let mut r_squared = vec![0; limbs.len()];
        r_squared[0] = 1;
        // Compute R^2 mod n with bounded shifts; no division or reciprocal is needed.
        for _ in 0..128 * limbs.len() {
            double_mod(&mut r_squared, &limbs);
        }
        Ok(Self {
            limbs,
            factor,
            r_squared,
            bits,
        })
    }

    /// The exact bit length of the modulus.
    pub(crate) const fn bits(&self) -> usize {
        self.bits
    }

    /// Its unsigned big-endian byte width.
    pub(crate) const fn bytes(&self) -> usize {
        self.bits.div_ceil(8)
    }

    /// Computes `base^exponent mod self`, returning exactly [`Self::bytes`] bytes.
    /// The base must be less than the modulus; it may have leading zero bytes.
    /// The exponent is canonical unsigned big-endian (empty means zero).
    ///
    /// Work is charged in limb products, including Montgomery reduction, before
    /// any multiplication. `max_work` is a per-call budget; the host must also
    /// bound the number of calls in a verification operation.
    ///
    /// # Errors
    /// Returns `Syntax` for an out-of-range base or noncanonical exponent, or
    /// `Limit` for an excessive exponent size or multiplication work.
    pub(crate) fn pow(
        &self,
        base: &[u8],
        exponent: &[u8],
        max_exponent_bits: usize,
        max_work: usize,
    ) -> Result<Vec<u8>> {
        let exponent_bits = if exponent.is_empty() {
            0
        } else {
            bit_length(exponent, max_exponent_bits.min(MAX_MODULUS_BITS))?
        };
        let ones: usize = exponent
            .iter()
            .map(|byte| u32_to_usize(byte.count_ones()))
            .sum();
        let products = if exponent_bits == 0 {
            0
        } else {
            exponent_bits - 1 + ones - 1 + 2
        };
        // A product is limbs² multiply-adds plus limbs² for reduction, and each
        // reduction row also multiplies its low limb by the factor. At most
        // 256 limbs keep this far from overflow.
        let limbs = self.limbs.len();
        Meter::new(max_work, "modular exponentiation work exceeds limit")
            .charge_product(products, limbs * (2 * limbs + 1))?;
        let base = self.residue(base)?;
        if exponent_bits == 0 {
            let mut one = vec![0; self.bytes()];
            if let Some(last) = one.last_mut() {
                *last = 1;
            }
            return Ok(one);
        }
        let mut scratch = vec![0; 2 * self.limbs.len() + 1];
        let mut encoded = vec![0; self.limbs.len()];
        self.multiply(&base, &self.r_squared, &mut encoded, &mut scratch);
        let mut value = encoded.clone();
        let mut next = vec![0; self.limbs.len()];
        // The leading one initializes `value`; avoid multiplying an identity.
        for bit in (0..exponent_bits - 1).rev() {
            self.multiply(&value, &value, &mut next, &mut scratch);
            std::mem::swap(&mut value, &mut next);
            if exponent[exponent.len() - 1 - bit / 8] & (1 << (bit % 8)) != 0 {
                self.multiply(&value, &encoded, &mut next, &mut scratch);
                std::mem::swap(&mut value, &mut next);
            }
        }
        let mut one = vec![0; self.limbs.len()];
        one[0] = 1;
        self.multiply(&value, &one, &mut next, &mut scratch);
        Ok(to_bytes(&next, self.bytes()))
    }

    fn residue(&self, bytes: &[u8]) -> Result<Vec<u64>> {
        if bytes.len() > self.bytes() {
            return Err(Error::syntax("modular operand exceeds modulus width")
                .at(Position::Buffer(self.bytes())));
        }
        let value = from_bytes(bytes, self.limbs.len());
        if at_least(&value, &self.limbs) {
            return Err(
                Error::syntax("modular operand is not below modulus").at(Position::Buffer(0))
            );
        }
        Ok(value)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "low radix-2^64 limbs; carries are shifted by 64"
    )]
    fn multiply(&self, a: &[u64], b: &[u64], out: &mut [u64], scratch: &mut [u64]) {
        let count = self.limbs.len();
        scratch.fill(0);
        for (i, &left) in a.iter().enumerate() {
            let mut carry = 0u128;
            for (j, &right) in b.iter().enumerate() {
                let product =
                    u128::from(left) * u128::from(right) + u128::from(scratch[i + j]) + carry;
                scratch[i + j] = product as u64;
                carry = product >> 64;
            }
            scratch[i + count] = carry as u64;
        }
        for i in 0..count {
            let factor = scratch[i].wrapping_mul(self.factor);
            let mut carry = 0u128;
            for (j, &modulus) in self.limbs.iter().enumerate() {
                let product =
                    u128::from(factor) * u128::from(modulus) + u128::from(scratch[i + j]) + carry;
                scratch[i + j] = product as u64;
                carry = product >> 64;
            }
            // Propagate into the one extra limb; all intermediate values < 2*n*R.
            for word in &mut scratch[i + count..] {
                let sum = u128::from(*word) + carry;
                *word = sum as u64;
                carry = sum >> 64;
                if carry == 0 {
                    break;
                }
            }
        }
        out.copy_from_slice(&scratch[count..2 * count]);
        if scratch[2 * count] != 0 || at_least(out, &self.limbs) {
            subtract(out, &self.limbs);
        }
    }
}

/// `-n⁻¹ mod 2^64` for an odd low limb `n`, the REDC factor. Newton
/// iteration doubles the number of correct bits each step: six reach 64.
pub(crate) const fn montgomery_factor(low: u64) -> u64 {
    let mut inverse = 1u64;
    let mut step = 0;
    while step < 6 {
        inverse = inverse.wrapping_mul(2u64.wrapping_sub(low.wrapping_mul(inverse)));
        step += 1;
    }
    inverse.wrapping_neg()
}

pub(crate) fn bit_length(bytes: &[u8], max_bits: usize) -> Result<usize> {
    // Check byte width first so a huge input never needs scanning or allocation.
    if bytes.len() > max_bits.div_ceil(8) {
        return Err(
            Error::limit("integer width exceeds limit").at(Position::Buffer(max_bits.div_ceil(8)))
        );
    }
    let Some(&first) = bytes.first().filter(|&&byte| byte != 0) else {
        return Err(
            Error::syntax("integer must be nonempty without leading zeros").at(Position::Buffer(0)),
        );
    };
    let bits = (bytes.len() - 1)
        .checked_mul(8)
        .and_then(|bits| bits.checked_add(8 - u32_to_usize(first.leading_zeros())))
        .ok_or_else(|| Error::limit("integer bit length overflow"))?;
    if bits > max_bits {
        return Err(Error::limit("integer bit length exceeds limit").at(Position::Buffer(0)));
    }
    Ok(bits)
}

fn from_bytes(bytes: &[u8], count: usize) -> Vec<u64> {
    let mut limbs = vec![0; count];
    for (limb, bytes) in limbs.iter_mut().zip(bytes.rchunks(8)) {
        let mut word = [0; 8];
        word[8 - bytes.len()..].copy_from_slice(bytes);
        *limb = u64::from_be_bytes(word);
    }
    limbs
}

fn to_bytes(limbs: &[u64], width: usize) -> Vec<u8> {
    let mut bytes = vec![0; width];
    for (chunk, word) in bytes.rchunks_mut(8).zip(limbs) {
        chunk.copy_from_slice(&word.to_be_bytes()[8 - chunk.len()..]);
    }
    bytes
}

fn at_least(a: &[u64], b: &[u64]) -> bool {
    a.iter().rev().cmp(b.iter().rev()).is_ge()
}

fn subtract(value: &mut [u64], modulus: &[u64]) {
    let mut borrow = false;
    for (value, &modulus) in value.iter_mut().zip(modulus) {
        let (difference, first) = value.overflowing_sub(modulus);
        let (difference, second) = difference.overflowing_sub(u64::from(borrow));
        *value = difference;
        borrow = first || second;
    }
}

fn double_mod(value: &mut [u64], modulus: &[u64]) {
    let mut carry = 0;
    for word in value.iter_mut() {
        let next = *word >> 63;
        *word = (*word << 1) | carry;
        carry = next;
    }
    if carry != 0 || at_least(value, modulus) {
        subtract(value, modulus);
    }
}

#[cfg(test)]
mod tests;
