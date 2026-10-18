//! Object identifier contents for signature algorithms and ESS/CMS attributes.

/// SHA-1 (1.3.14.3.2.26; RFC 4055 section 2.1).
pub const SHA1: &[u8] = &[43, 14, 3, 2, 26];
/// SHA-224 (2.16.840.1.101.3.4.2.4).
pub const SHA224: &[u8] = &[96, 134, 72, 1, 101, 3, 4, 2, 4];
/// SHA-256 (2.16.840.1.101.3.4.2.1).
pub const SHA256: &[u8] = &[96, 134, 72, 1, 101, 3, 4, 2, 1];
/// SHA-384 (2.16.840.1.101.3.4.2.2).
pub const SHA384: &[u8] = &[96, 134, 72, 1, 101, 3, 4, 2, 2];
/// SHA-512 (2.16.840.1.101.3.4.2.3).
pub const SHA512: &[u8] = &[96, 134, 72, 1, 101, 3, 4, 2, 3];
/// RSA encryption/public-key identifier (RFC 3279 section 2.3.1).
pub const RSA_ENCRYPTION: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 1, 1];
/// RSASSA-PSS (RFC 4055 section 3.1).
pub const RSASSA_PSS: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 1, 10];
/// MGF1 (RFC 4055 section 2.2).
pub const MGF1: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 1, 8];
/// EC public key (RFC 5480 section 2.1.1).
pub const EC_PUBLIC_KEY: &[u8] = &[42, 134, 72, 206, 61, 2, 1];
/// ESS signing-certificate v1 (RFC 5035 section 5).
pub const SIGNING_CERTIFICATE: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 9, 16, 2, 12];
/// ESS signing-certificate v2 (RFC 5035 section 3).
pub const SIGNING_CERTIFICATE_V2: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 9, 16, 2, 47];
/// CMS algorithm protection (1.2.840.113549.1.9.52; RFC 8933 section 4).
pub const CMS_ALGORITHM_PROTECTION: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 9, 52];
/// NIST P-256 named curve (RFC 5480 section 2.1.1.1).
pub const SECP256R1: &[u8] = &[42, 134, 72, 206, 61, 3, 1, 7];
/// NIST P-384 named curve (RFC 5480 section 2.1.1.1).
pub const SECP384R1: &[u8] = &[43, 129, 4, 0, 34];
/// NIST P-521 named curve (RFC 5480 section 2.1.1.1).
pub const SECP521R1: &[u8] = &[43, 129, 4, 0, 35];
/// RSA PKCS #1 v1.5 with SHA-1 (RFC 3279 section 2.2.1; RFC 4055 section 5).
pub const SHA1_WITH_RSA: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 1, 5];
/// ECDSA with SHA-1 (RFC 3279 section 2.2.3; RFC 5758 section 3.2).
pub const ECDSA_WITH_SHA1: &[u8] = &[42, 134, 72, 206, 61, 4, 1];
/// RSA PKCS #1 v1.5 with SHA-224 (RFC 3279 section 2.2.1; RFC 4055 section 5).
pub const SHA224_WITH_RSA: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 1, 14];
/// ECDSA with SHA-224 (RFC 3279 section 2.2.3; RFC 5758 section 3.2).
pub const ECDSA_WITH_SHA224: &[u8] = &[42, 134, 72, 206, 61, 4, 3, 1];
/// RSA PKCS #1 v1.5 with SHA-256 (RFC 3279 section 2.2.1; RFC 4055 section 5).
pub const SHA256_WITH_RSA: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 1, 11];
/// ECDSA with SHA-256 (RFC 3279 section 2.2.3; RFC 5758 section 3.2).
pub const ECDSA_WITH_SHA256: &[u8] = &[42, 134, 72, 206, 61, 4, 3, 2];
/// RSA PKCS #1 v1.5 with SHA-384 (RFC 3279 section 2.2.1; RFC 4055 section 5).
pub const SHA384_WITH_RSA: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 1, 12];
/// ECDSA with SHA-384 (RFC 3279 section 2.2.3; RFC 5758 section 3.2).
pub const ECDSA_WITH_SHA384: &[u8] = &[42, 134, 72, 206, 61, 4, 3, 3];
/// RSA PKCS #1 v1.5 with SHA-512 (RFC 3279 section 2.2.1; RFC 4055 section 5).
pub const SHA512_WITH_RSA: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 1, 13];
/// ECDSA with SHA-512 (RFC 3279 section 2.2.3; RFC 5758 section 3.2).
pub const ECDSA_WITH_SHA512: &[u8] = &[42, 134, 72, 206, 61, 4, 3, 4];
