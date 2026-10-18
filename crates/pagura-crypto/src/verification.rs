//! Mathematical signature outcomes shared by RSA and ECDSA.

/// The mathematical result of a signature verification, separate from malformed
/// inputs, resource errors, algorithm policy, signer trust and PDF revision
/// coverage (ADR 0015). A successful `Result` must still have its outcome checked.
///
/// Using `?` alone does not consume this outcome:
///
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use pagura_core::Result;
/// use pagura_crypto::Verification;
/// fn discard(result: Result<Verification>) -> Result<()> {
///     result?;
///     Ok(())
/// }
/// ```
#[must_use = "a signature verification outcome must be checked"]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verification {
    /// The signature matches the supplied public key, digest and parameters.
    Valid,
    /// Well-formed inputs failed the signature verification checks.
    Invalid,
}
