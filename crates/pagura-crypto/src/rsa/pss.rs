//! EMSA-PSS and MGF1 from RFC 8017 sections 9.1.2 and B.2.1.

use crate::Verification;
use pagura_core::{Error, Result};

use super::HashAlgorithm;
use crate::bigint::MAX_MODULUS_BITS;

/// Resolved RSASSA-PSS parameters (RFC 4055 section 3.1).
/// ASN.1 decoding belongs to the PKI layer. Only MGF1 and trailer field 1
/// (the byte BC) are supported. Different message and MGF1 hashes are allowed.
/// The PKI layer must enforce PSS public-key restrictions, including minimum
/// salt length, separately (RFC 4055 section 3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PssParameters {
    hash: HashAlgorithm,
    mgf_hash: HashAlgorithm,
    salt_bytes: usize,
}

impl Default for PssParameters {
    /// Defaults of RFC 4055: SHA-1, MGF1-SHA-1, 20-byte salt, trailer field 1.
    fn default() -> Self {
        Self {
            hash: HashAlgorithm::Sha1,
            mgf_hash: HashAlgorithm::Sha1,
            salt_bytes: 20,
        }
    }
}

impl PssParameters {
    /// Resolves already decoded parameters; `trailer_field` is the ASN.1 value
    /// 1, not the encoded trailer byte BC. Zero-length salts are permitted.
    ///
    /// # Errors
    /// Returns `Syntax` for any trailer field other than 1, or `Limit` for a
    /// salt larger than any supported RSA modulus can hold.
    pub fn new(
        hash: HashAlgorithm,
        mgf_hash: HashAlgorithm,
        salt_bytes: usize,
        trailer_field: u64,
    ) -> Result<Self> {
        if trailer_field != 1 {
            return Err(Error::syntax("RSA-PSS trailer field must be one"));
        }
        if salt_bytes > MAX_MODULUS_BITS / 8 {
            return Err(Error::limit("RSA-PSS salt length exceeds modulus ceiling"));
        }
        Ok(Self {
            hash,
            mgf_hash,
            salt_bytes,
        })
    }

    /// The hash of the signed message.
    pub const fn hash(self) -> HashAlgorithm {
        self.hash
    }

    /// The hash used inside MGF1.
    pub const fn mgf_hash(self) -> HashAlgorithm {
        self.mgf_hash
    }

    /// The exact salt length in bytes.
    pub const fn salt_bytes(self) -> usize {
        self.salt_bytes
    }

    /// Rejects SHA-1 for both message hashing and mask generation.
    ///
    /// # Errors
    /// Returns `InvalidArgument` if either hash is weak.
    pub fn check_strength(self) -> Result<()> {
        self.hash.check_strength()?;
        self.mgf_hash.check_strength()
    }

    pub(super) fn encoded_bytes(self, bits: usize) -> Result<usize> {
        let width = bits.div_ceil(8);
        let minimum = self.hash.digest_bytes() + self.salt_bytes + 2;
        if width < minimum {
            return Err(Error::syntax("RSA modulus too short for PSS parameters"));
        }
        Ok(width)
    }

    pub(super) fn verify(
        self,
        recovered: &[u8],
        bits: usize,
        digest: &[u8],
    ) -> Result<Verification> {
        let width = self.encoded_bytes(bits)?;
        // If modBits = 1 mod 8, RFC 8017 I2OSP must discard a zero byte only.
        let skipped = recovered
            .len()
            .checked_sub(width)
            .ok_or_else(|| Error::syntax("RSA-PSS encoded message is too short"))?;
        if recovered[..skipped].iter().any(|&byte| byte != 0) {
            return Ok(Verification::Invalid);
        }
        let encoded = &recovered[skipped..];
        if encoded.last() != Some(&0xbc) {
            return Ok(Verification::Invalid);
        }
        let db_len = width - self.hash.digest_bytes() - 1;
        let (masked, tail) = encoded.split_at(db_len);
        let h = &tail[..self.hash.digest_bytes()];
        let unused = width * 8 - bits;
        let top_mask = 0xffu8 >> unused;
        if masked[0] & !top_mask != 0 {
            return Ok(Verification::Invalid);
        }
        let mut db = masked.to_vec();
        mgf1_xor(self.mgf_hash, h, &mut db);
        db[0] &= top_mask;
        let padding = db_len - self.salt_bytes - 1;
        if db[..padding].iter().any(|&byte| byte != 0) || db[padding] != 1 {
            return Ok(Verification::Invalid);
        }
        let expected = self
            .hash
            .digest_parts(&[&[0; 8], digest, &db[padding + 1..]]);
        Ok(if expected == h {
            Verification::Valid
        } else {
            Verification::Invalid
        })
    }
}

fn mgf1_xor(hash: HashAlgorithm, seed: &[u8], output: &mut [u8]) {
    // RSA bounds output to <= 2048 bytes, hence the counter always fits u32.
    for (counter, block) in (0u32..).zip(output.chunks_mut(hash.digest_bytes())) {
        let digest = hash.digest_parts(&[seed, &counter.to_be_bytes()]);
        for (byte, mask) in block.iter_mut().zip(digest) {
            *byte ^= mask;
        }
    }
}

#[cfg(test)]
mod tests;
