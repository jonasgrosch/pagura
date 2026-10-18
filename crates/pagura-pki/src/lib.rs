//! Bounded PKI parsing and host-anchored certification path validation.
//!
//! Parsed data never establishes integrity, trust, freshness or revocation.
//! Original encodings remain available for verification. The validation module
//! takes explicit host anchors, time and signature callbacks; it has no clock,
//! network, algorithm implementation or implicit trust store.
#![forbid(unsafe_code)]

pub mod cms;
pub mod der;
pub mod oids;
pub mod timestamp;
pub mod types;
pub mod validation;
pub mod x509;

pub use types::{BitString, Integer, ObjectIdentifier, Time};

#[cfg(test)]
mod tests;

/// Stable tags for ASN.1 types outside the supported encoding subset.
pub mod features {
    use pagura_core::Feature;

    /// A universal type whose primitive encoding is not implemented.
    pub const UNIVERSAL_TYPE: Feature = Feature::new("asn1.universal-type");
    /// A constructed universal type outside the supported forms.
    pub const CONSTRUCTED_TYPE: Feature = Feature::new("asn1.constructed-type");
}

/// Stable tags for the deliberately narrow BER compatibility entry point.
pub mod tags {
    use pagura_core::Repair;

    /// A constructed BER value uses an end-of-contents marker (X.690 §8.1.3).
    pub const BER_INDEFINITE: Repair = Repair::new("asn1.ber-indefinite");
    /// BER length has redundant octets or uses long form below 128 (§10.1).
    pub const BER_LENGTH: Repair = Repair::new("asn1.ber-length");
    /// BER SET OF members are retained in their noncanonical source order (§11.6).
    pub const BER_SET_ORDER: Repair = Repair::new("asn1.ber-set-order");
    /// A BER OCTET STRING is constructed rather than primitive (§10.2).
    pub const BER_OCTETS: Repair = Repair::new("asn1.ber-octets");
}
