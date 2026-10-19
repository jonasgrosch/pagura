//! Verification-only RSA (RFC 8017 sections 5.2.2, 8.1.2 and 8.2.2).
//!
//! These operations establish only a mathematical signature match. They do not
//! establish signer identity, trust, time, revision coverage or revocation.
//! Historical weak algorithms can be checked, but callers must separately apply
//! a strength policy with [`RsaPublicKey::check_strength`] and
//! [`HashAlgorithm::check_strength`] (ADR 0015). No private-key operation exists.
//! Malformed inputs are errors; a signature mismatch is [`Verification::Invalid`].

use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Position, Result};

use crate::bigint::{MAX_MODULUS_BITS, Modulus, bit_length};
use crate::{Digest, Sha1, Sha224, Sha256, Sha384, Sha512, Verification};

mod pss;
pub use pss::PssParameters;

/// Hash algorithms supported by RSA verification (RFC 8017 Appendix B.1).
/// RFC 8017 lists more, so further variants may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HashAlgorithm {
    /// SHA-1, weak; available only for historical mathematical verification.
    Sha1,
    /// SHA-224.
    Sha224,
    /// SHA-256.
    Sha256,
    /// SHA-384.
    Sha384,
    /// SHA-512.
    Sha512,
}

impl HashAlgorithm {
    /// Digest length in bytes.
    pub const fn digest_bytes(self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha224 => 28,
            Self::Sha256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }

    /// Whether collision resistance is known to be broken (ADR 0015).
    pub const fn is_weak(self) -> bool {
        matches!(self, Self::Sha1)
    }

    /// Rejects a weak hash independently of mathematical verification.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for SHA-1.
    pub fn check_strength(self) -> Result<()> {
        if self.is_weak() {
            return Err(Error::invalid_argument("SHA-1 is a weak signature hash"));
        }
        Ok(())
    }

    /// Hashes a message using the selected algorithm.
    pub fn digest(self, message: &[u8]) -> Vec<u8> {
        self.digest_parts([message])
    }

    /// Hashes borrowed message parts in iteration order without concatenating them.
    /// Empty parts and an empty iterator are permitted (FIPS 180-4).
    pub fn digest_parts<'a>(self, parts: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
        fn digest<'a, D: Digest>(parts: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
            let mut hash = D::default();
            for part in parts {
                hash.update(part);
            }
            hash.finalize().as_ref().to_vec()
        }
        match self {
            Self::Sha1 => digest::<Sha1>(parts),
            Self::Sha224 => digest::<Sha224>(parts),
            Self::Sha256 => digest::<Sha256>(parts),
            Self::Sha384 => digest::<Sha384>(parts),
            Self::Sha512 => digest::<Sha512>(parts),
        }
    }

    // Exact DER encodings from RFC 8017 section 9.2, including required NULL.
    fn digest_info(self) -> &'static [u8] {
        match self {
            Self::Sha1 => b"\x30\x21\x30\x09\x06\x05\x2b\x0e\x03\x02\x1a\x05\x00\x04\x14",
            Self::Sha224 => {
                b"\x30\x2d\x30\x0d\x06\x09\x60\x86\x48\x01\x65\x03\x04\x02\x04\x05\x00\x04\x1c"
            }
            Self::Sha256 => {
                b"\x30\x31\x30\x0d\x06\x09\x60\x86\x48\x01\x65\x03\x04\x02\x01\x05\x00\x04\x20"
            }
            Self::Sha384 => {
                b"\x30\x41\x30\x0d\x06\x09\x60\x86\x48\x01\x65\x03\x04\x02\x02\x05\x00\x04\x30"
            }
            Self::Sha512 => {
                b"\x30\x51\x30\x0d\x06\x09\x60\x86\x48\x01\x65\x03\x04\x02\x03\x05\x00\x04\x40"
            }
        }
    }
}

/// Per-key and per-verification arithmetic budgets (ADR 0015).
///
/// Hosts can pass `&pagura_core::Limits` directly to [`RsaPublicKey::new`], or
/// convert it with [`RsaLimits::from`]. The hard modulus ceiling remains
/// [`MAX_MODULUS_BITS`]. Work is measured in RSA limb products; ECDSA's fixed
/// per-call bounds do not consume this budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct RsaLimits {
    /// Maximum modulus size in bits (default 16,384).
    pub modulus_bits: usize,
    /// Maximum public exponent size in bits (default 256).
    pub exponent_bits: usize,
    /// Maximum radix-2^64 limb products per verification (default 64 million).
    pub work: usize,
}

impl Default for RsaLimits {
    fn default() -> Self {
        Self::from(&Limits::default())
    }
}

impl From<&Limits> for RsaLimits {
    fn from(limits: &Limits) -> Self {
        Self {
            modulus_bits: u32_to_usize(limits.signatures.rsa_modulus_bits),
            exponent_bits: u32_to_usize(limits.signatures.rsa_exponent_bits),
            work: u32_to_usize(limits.signatures.rsa_work),
        }
    }
}

/// A validated, bounded RSA public key, with reusable arithmetic constants.
/// Its constructor does not assert primality or cryptographic strength.
#[derive(Debug, Clone)]
pub struct RsaPublicKey {
    modulus: Modulus,
    exponent: Vec<u8>,
    limits: RsaLimits,
}

impl RsaPublicKey {
    /// Reads canonical unsigned big-endian `n` and `e` (without DER sign padding).
    /// Requires odd `n > 1` and odd `3 <= e < n` (RFC 8017 section 3.1).
    ///
    /// # Errors
    /// Returns `Syntax` for malformed components, or `Limit` for excessive width.
    pub fn new(n: &[u8], e: &[u8], limits: impl Into<RsaLimits>) -> Result<Self> {
        let limits = limits.into();
        bit_length(n, limits.modulus_bits.min(MAX_MODULUS_BITS))?;
        let exponent_bits = bit_length(e, limits.exponent_bits.min(MAX_MODULUS_BITS))?;
        if exponent_bits < 2 || e.last().is_none_or(|byte| byte & 1 == 0) {
            return Err(Error::syntax(
                "RSA public exponent must be odd and at least three",
            ));
        }
        if e.len() > n.len() || (e.len() == n.len() && e >= n) {
            return Err(Error::syntax("RSA public exponent must be below modulus"));
        }
        Ok(Self {
            modulus: Modulus::new(n, limits.modulus_bits)?,
            exponent: e.to_vec(),
            limits,
        })
    }

    /// Exact modulus bit length.
    pub const fn modulus_bits(&self) -> usize {
        self.modulus.bits()
    }

    /// Required signature length, in bytes.
    pub const fn signature_bytes(&self) -> usize {
        self.modulus.bytes()
    }

    /// Checks a conservative strength policy: modulus >= 2048 bits and e >= 65537.
    /// This does not establish proper key generation or trust.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a smaller modulus or public exponent.
    pub fn check_strength(&self) -> Result<()> {
        if self.modulus.bits() < 2048 {
            return Err(Error::invalid_argument(
                "RSA modulus is below 2048-bit strength policy",
            ));
        }
        if self.exponent.len() < 3
            || (self.exponent.len() == 3 && self.exponent.as_slice() < [1, 0, 1].as_slice())
        {
            return Err(Error::invalid_argument(
                "RSA public exponent is below 65537 strength policy",
            ));
        }
        Ok(())
    }

    /// Verifies RSASSA-PKCS1-v1_5 over a precomputed digest (RFC 8017 section 8.2.2).
    /// Exact DER DigestInfo, including NULL parameters, and at least eight FF
    /// padding bytes are required; BER and absent-NULL variants are rejected.
    /// The caller must select the hash independently of the signature bytes.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a wrong digest width, `Syntax` for a
    /// malformed signature width or representative, or an unsuitable modulus,
    /// and `Limit` for excessive work. A well-formed signature that does not
    /// match the expected encoding returns `Ok(Verification::Invalid)`.
    pub fn verify_pkcs1v15(
        &self,
        hash: HashAlgorithm,
        digest: &[u8],
        signature: &[u8],
    ) -> Result<Verification> {
        check_digest(hash, digest)?;
        let prefix = hash.digest_info();
        let tail = prefix.len() + digest.len();
        let width = self.signature_bytes();
        if width < tail + 11 {
            return Err(Error::syntax("RSA modulus too short for PKCS1-v1_5 digest"));
        }
        let mut expected = vec![0xff; width];
        expected[..2].copy_from_slice(&[0, 1]);
        let separator = width - tail - 1;
        expected[separator] = 0;
        expected[separator + 1..width - digest.len()].copy_from_slice(prefix);
        expected[width - digest.len()..].copy_from_slice(digest);
        Ok(if self.recover(signature)? == expected {
            Verification::Valid
        } else {
            Verification::Invalid
        })
    }

    /// Verifies RSASSA-PSS over a precomputed digest (RFC 8017 section 8.1.2).
    /// The salt length is exact, never inferred from untrusted padding.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a wrong digest width, `Syntax` for malformed
    /// signature width, representative or unsuitable parameters, or `Limit` for
    /// excessive work. A recovered message that fails the PSS verification
    /// checks returns `Ok(Verification::Invalid)`.
    pub fn verify_pss(
        &self,
        parameters: PssParameters,
        digest: &[u8],
        signature: &[u8],
    ) -> Result<Verification> {
        check_digest(parameters.hash(), digest)?;
        parameters.encoded_bytes(self.modulus.bits() - 1)?;
        parameters.verify(&self.recover(signature)?, self.modulus.bits() - 1, digest)
    }

    fn recover(&self, signature: &[u8]) -> Result<Vec<u8>> {
        if signature.len() != self.signature_bytes() {
            return Err(
                Error::syntax("RSA signature length differs from modulus width").at(
                    Position::Buffer(signature.len().min(self.signature_bytes())),
                ),
            );
        }
        self.modulus.pow(
            signature,
            &self.exponent,
            self.limits.exponent_bits,
            self.limits.work,
        )
    }
}

fn check_digest(hash: HashAlgorithm, digest: &[u8]) -> Result<()> {
    if digest.len() != hash.digest_bytes() {
        return Err(Error::invalid_argument(
            "RSA digest length does not match hash algorithm",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
