#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Cryptographic primitives and the PDF standard security handler
//! (ISO 32000-2 clause 7.6), implemented from their published standards.
//!
//! The public primitives know nothing about PDF: the RFC 1321 MD5 and
//! FIPS 180-4 SHA-1/SHA-2 hashes behind [`Digest`], and FIPS 186-5 ECDSA and
//! RFC 8017 RSA signature verification. The [`standard`] module implements
//! password authentication and per-object decryption from plain parameters;
//! the document layer maps the `/Encrypt` dictionary onto them. Its ciphers
//! (RC4, and FIPS 197 AES with SP 800-38A CBC) are internal.
//!
//! Cipher operations are limited to decryption and the encryption steps that
//! key derivation needs. Signature operations are verification only. This is
//! not a general-purpose cryptography library. AES encryption, decryption and
//! key expansion use secret-dependent table lookups; finite-field arithmetic
//! also has data-dependent branches. No constant-time guarantee is made. Local
//! document processing does not remove the possibility of timing or cache
//! observations in a shared execution environment. Key material is cleared
//! when dropped, on a best-effort basis only (ADR 0021).

mod aes;
mod bigint;
mod ec;
pub mod ecdsa;
mod hash;
mod rc4;
pub mod rsa;
mod secret;
pub mod standard;
mod verification;

pub use bigint::MAX_MODULUS_BITS;
pub use hash::{Digest, Md5, Sha1, Sha224, Sha256, Sha384, Sha512};
pub use rsa::{HashAlgorithm, PssParameters, RsaLimits, RsaPublicKey};
pub use verification::Verification;

/// Stable tags for security handlers this crate does not implement.
pub mod features {
    use pagura_core::Feature;

    /// A handler revision outside the implemented standard revisions.
    pub const REVISION: Feature = Feature::new("security.revision");
    /// The unpublished version 3 encryption algorithm.
    pub const VERSION3: Feature = Feature::new("security.V3");

    /// Every feature tag of this crate.
    pub const ALL: &[Feature] = &[REVISION, VERSION3];
}

/// Unicode data copyright and permission notice for the SASLprep tables.
#[must_use]
pub const fn unicode_notice() -> &'static str {
    include_str!("../../../data/unicode/LICENSE")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::features;
    use std::path::PathBuf;

    /// Decodes a hexadecimal vector field; `-` stands for no bytes.
    pub(crate) fn decode(field: &str) -> Vec<u8> {
        if field == "-" {
            Vec::new()
        } else {
            pagura_testkit::unhex(field)
        }
    }

    /// A reference suite exported from the private cache by
    /// `python3 tools/signature_vectors.py`, which `tools/check.py` runs
    /// before the tests; `None` with a note when it has not been exported
    /// (as on CI, which has no cache).
    pub(crate) fn cached_suite(name: &str) -> Option<String> {
        let root = std::env::var_os("PAGURA_REFERENCES_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
                Some(PathBuf::from(home).join(".cache/pagura-references"))
            })?;
        let path = root
            .join("signatures/export/crypto")
            .join(format!("{name}.txt"));
        let text = std::fs::read_to_string(&path).ok();
        if text.is_none() {
            eprintln!(
                "{}: not exported (python3 tools/signature_vectors.py); the bundled cases still run",
                path.display()
            );
        }
        text
    }

    #[test]
    fn feature_tags_are_listed_once_in_the_security_area() {
        for (index, tag) in features::ALL.iter().enumerate() {
            assert!(tag.as_str().starts_with("security."), "{tag:?}");
            assert!(!features::ALL[..index].contains(tag), "{tag:?}");
        }
        assert_eq!(features::ALL, [features::REVISION, features::VERSION3]);
    }
}
