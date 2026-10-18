//! Verification-only ECDSA on the NIST prime curves (FIPS 186-5 §6.4.2).
//!
//! This layer checks mathematical signatures only. The caller selects and
//! computes the message hash, applies algorithm policy, and establishes signer
//! trust separately (ADR 0015). All arithmetic is variable-time on public data;
//! there are no private-key or signing operations.
//!
//! Work and storage per operation have fixed bounds: at most 133 SEC1 bytes,
//! 139 DER bytes, nine limbs, 521 scalar bits and a four-point table. Only the
//! leftmost 66 digest bytes are read (at most 521 bits are used). A higher
//! layer must budget the number of keys and signature verifications across a
//! document.

mod der;
#[cfg(test)]
mod tests;

use crate::Verification;
pub use crate::ec::Curve;
use crate::ec::{Point, Uint};
use pagura_core::{Error, Position, Result};

/// A validated, finite SEC1 public key on a named prime-order curve.
///
/// SP 800-186 §D.1.1 requires finite, in-range, on-curve points of order n.
/// The three named curves have prime order and cofactor one, so every finite
/// on-curve point has order n; a separate multiplication by n is redundant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    curve: Curve,
    point: Point,
}

impl PublicKey {
    /// Decodes a compressed (02/03) or uncompressed (04) SEC1 public point.
    ///
    /// Coordinates have exactly the named curve's width; identity, hybrid,
    /// off-curve, noncanonical and trailing-byte encodings are rejected.
    /// Compressed points use SP 800-186 §D.2.1; RFC 5480 §2.2 identifies
    /// the permitted SEC1 prefixes.
    ///
    /// # Errors
    /// Returns a positioned syntax error for any malformed or invalid point.
    pub fn from_sec1(curve: Curve, bytes: &[u8]) -> Result<Self> {
        let size = curve.coordinate_bytes();
        let prefix = bytes.first().copied().ok_or_else(|| point_error(0))?;
        let compressed = match prefix {
            2 | 3 if bytes.len() == 1 + size => true,
            4 if bytes.len() == 1 + 2 * size => false,
            _ => return Err(point_error(0)),
        };
        let parameters = curve.parameters();
        let f = &parameters.field;
        let x = Uint::from_be_bytes(&bytes[1..=size]).ok_or_else(|| point_error(1))?;
        if x >= f.value {
            return Err(point_error(1));
        }
        let x = f.encode(x);
        let equation = parameters.equation(x);
        let y = if compressed {
            let mut root = f.sqrt(equation).ok_or_else(|| point_error(1))?;
            if (f.decode(root).0[0] & 1) != u64::from(prefix & 1) {
                root = f.sub(Uint::ZERO, root);
            }
            // A zero square root cannot satisfy an odd SEC1 prefix.
            if (f.decode(root).0[0] & 1) != u64::from(prefix & 1) {
                return Err(point_error(0));
            }
            root
        } else {
            let y = Uint::from_be_bytes(&bytes[1 + size..]).ok_or_else(|| point_error(1 + size))?;
            if y >= f.value {
                return Err(point_error(1 + size));
            }
            f.encode(y)
        };
        if f.square(y) != equation {
            return Err(point_error(1 + usize::from(!compressed) * size));
        }
        Ok(Self {
            curve,
            point: Point::affine(x, y, parameters),
        })
    }

    /// The named curve on which the public point has been validated.
    pub const fn curve(&self) -> Curve {
        self.curve
    }

    /// Verifies a previously parsed signature against a precomputed digest.
    ///
    /// FIPS 186-5 §6.4.2: digest truncation uses the leftmost order-sized
    /// bit string, including P-521's non-byte-aligned truncation. Both high
    /// and low s are accepted; ECDSA itself does not impose a low-s rule.
    /// `Verification::Valid` establishes only the mathematical equation, never trust,
    /// hash strength, policy acceptance or PDF signed-byte coverage.
    ///
    /// # Errors
    /// Returns an invalid-argument error for an empty digest or a signature
    /// parsed for a different curve. A mathematical mismatch is
    /// `Ok(Verification::Invalid)`.
    pub fn verify_digest(&self, digest: &[u8], signature: &Signature) -> Result<Verification> {
        if signature.curve != self.curve {
            return Err(Error::invalid_argument(
                "ECDSA signature and public key curves differ",
            ));
        }
        if digest.is_empty() {
            return Err(Error::invalid_argument("ECDSA digest must not be empty"));
        }
        let parameters = self.curve.parameters();
        let scalar = &parameters.scalar;
        let Some(inverse) = scalar.inverse(scalar.encode(signature.s)) else {
            // Signature construction already rejects zero; keep inversion total.
            return Ok(Verification::Invalid);
        };
        let e = digest_integer(self.curve, digest);
        let u = scalar.decode(scalar.mul(scalar.encode(scalar.reduce_once(e)), inverse));
        let v = scalar.decode(scalar.mul(scalar.encode(signature.r), inverse));
        let result = self
            .point
            .double_scalar(u, v, parameters, self.curve.bits());
        let Some((x, _)) = result.affine_coordinates(parameters) else {
            return Ok(Verification::Invalid);
        };
        // SP 800-186's named parameters give p < 2*n for all three curves.
        Ok(if scalar.reduce_once(x) == signature.r {
            Verification::Valid
        } else {
            Verification::Invalid
        })
    }

    /// Parses a strict DER signature and verifies it against a digest.
    ///
    /// # Errors
    /// Returns the errors from [`Signature::from_der`] or
    /// [`Self::verify_digest`]. No BER or malformed-integer tolerance is used.
    pub fn verify_der(&self, digest: &[u8], der: &[u8]) -> Result<Verification> {
        self.verify_digest(digest, &Signature::from_der(self.curve, der)?)
    }
}

/// A strict DER ECDSA signature with 1 ≤ r,s < n on one named curve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    curve: Curve,
    r: Uint,
    s: Uint,
}

impl Signature {
    /// Parses ECDSA-Sig-Value (RFC 3279 §2.2.3) as strict DER.
    ///
    /// X.690 §§8.3.1–8.3.2 and 10.1 require minimally encoded signed
    /// INTEGERs and shortest definite lengths. Both integers must be positive
    /// and less than the curve order (FIPS 186-5 §6.4.2 step 1). There is no
    /// tolerance for negative, padded, indefinite, truncated or trailing data.
    ///
    /// # Errors
    /// Returns a positioned syntax error for malformed DER or out-of-range
    /// integers, or a limit error if the input exceeds 139 bytes.
    pub fn from_der(curve: Curve, bytes: &[u8]) -> Result<Self> {
        let (r, s) = der::parse(curve, bytes)?;
        Ok(Self { curve, r, s })
    }

    /// The curve whose order bounds the signature integers.
    pub const fn curve(&self) -> Curve {
        self.curve
    }
}

fn point_error(offset: usize) -> Error {
    Error::syntax("invalid ECDSA SEC1 public point").at(Position::Buffer(offset))
}

fn digest_integer(curve: Curve, digest: &[u8]) -> Uint {
    let count = digest.len().min(curve.coordinate_bytes());
    // Fixed curve widths ensure the conversion always fits in nine limbs.
    let mut result = Uint::ZERO;
    for &byte in &digest[..count] {
        for i in (1..9).rev() {
            result.0[i] = (result.0[i] << 8) | (result.0[i - 1] >> 56);
        }
        result.0[0] = (result.0[0] << 8) | u64::from(byte);
    }
    let excess = (count * 8).saturating_sub(curve.bits());
    if excess != 0 {
        result = result.shr(excess);
    }
    result
}
