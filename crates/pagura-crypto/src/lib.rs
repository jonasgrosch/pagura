#![forbid(unsafe_code)]
//! Cryptographic primitives and the PDF standard security handler
//! (ISO 32000-2 clause 7.6), implemented from their published standards.
//!
//! Primitives (RFC 1321 MD5, FIPS 180-4 SHA-2, RC4, FIPS 197 AES with
//! SP 800-38A CBC, FIPS 186-5 ECDSA verification) know nothing about PDF. The [`standard`] module
//! implements password authentication and per-object decryption from plain
//! parameters; the document layer maps the `/Encrypt` dictionary onto them.
//!
//! Cipher operations are limited to decryption and the encryption steps that
//! key derivation needs. Signature operations are verification only. This is
//! not a general-purpose cryptography library. AES encryption, decryption and
//! key expansion use secret-dependent S-box table lookups;
//! finite-field arithmetic also has data-dependent branches. No constant-time
//! guarantee is made. Local document processing does not remove the possibility
//! of timing or cache observations in a shared execution environment.

pub mod aes;
mod ec;
pub mod ecdsa;
mod hash;
mod rc4;
pub mod standard;

pub use aes::Aes;
pub use hash::{Md5, Sha256, Sha384, Sha512, md5, sha256, sha384, sha512};
pub use rc4::Rc4;

/// Unicode data copyright and permission notice for the SASLprep tables.
#[must_use]
pub const fn unicode_notice() -> &'static str {
    include_str!("../../../data/unicode/LICENSE")
}
