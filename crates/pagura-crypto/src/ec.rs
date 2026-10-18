//! Fixed-size, variable-time arithmetic for the three prime-order NIST curves.
//!
//! All values are public. Neither these operations nor their callers may be
//! used for private keys. Sizes and loop bounds come only from named curves.

mod field;
mod parameters;
mod point;

pub(super) use field::Uint;
pub(super) use parameters::Parameters;
pub(super) use point::Point;

/// NIST prime curves from SP 800-186 sections 3.2.1.3–3.2.1.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Curve {
    /// The 256-bit prime curve, also called secp256r1.
    P256,
    /// The 384-bit prime curve, also called secp384r1.
    P384,
    /// The 521-bit prime curve, also called secp521r1.
    P521,
}

impl Curve {
    /// Size of one SEC1 field coordinate in bytes, including P-521's padding.
    pub const fn coordinate_bytes(self) -> usize {
        self.bits().div_ceil(8)
    }

    pub(super) const fn bits(self) -> usize {
        match self {
            Self::P256 => 256,
            Self::P384 => 384,
            Self::P521 => 521,
        }
    }

    pub(super) const fn parameters(self) -> &'static Parameters {
        match self {
            Self::P256 => &parameters::P256,
            Self::P384 => &parameters::P384,
            Self::P521 => &parameters::P521,
        }
    }
}
