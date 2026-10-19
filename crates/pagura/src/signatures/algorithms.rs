//! Algorithm identifiers: RFC 4055 §§2–3, RFC 5480 §2, RFC 5758 §3.
use pagura_core::{Error, Limits, Position, Result};
use pagura_crypto::ecdsa::{Curve, PublicKey};
use pagura_crypto::{HashAlgorithm, PssParameters, RsaPublicKey, Verification};
use pagura_pki::der::{self, Document, Element, Fields, Form, Tag, single};
use pagura_pki::oids::{
    EC_PUBLIC_KEY, ECDSA_WITH_SHA1, ECDSA_WITH_SHA224, ECDSA_WITH_SHA256, ECDSA_WITH_SHA384,
    ECDSA_WITH_SHA512, MGF1, RSA_ENCRYPTION, RSASSA_PSS, SECP256R1, SECP384R1, SECP521R1, SHA1,
    SHA1_WITH_RSA, SHA224, SHA224_WITH_RSA, SHA256, SHA256_WITH_RSA, SHA384, SHA384_WITH_RSA,
    SHA512, SHA512_WITH_RSA,
};
use pagura_pki::x509::{AlgorithmIdentifier, SubjectPublicKeyInfo};
use pagura_pki::{Integer, ObjectIdentifier};

fn unsupported(element: Element<'_>) -> Error {
    Error::unsupported(
        super::features::ALGORITHM,
        "unsupported signature algorithm",
    )
    .at(Position::Buffer(element.range().start))
}
pub(super) fn null_or_absent(algorithm: AlgorithmIdentifier<'_>) -> Result<()> {
    if let Some(parameter) = algorithm.parameters {
        parameter.expect(der::NULL)?;
    }
    Ok(())
}
pub(super) fn hash(algorithm: AlgorithmIdentifier<'_>) -> Result<HashAlgorithm> {
    null_or_absent(algorithm)?;
    match algorithm.algorithm {
        SHA1 => Ok(HashAlgorithm::Sha1),
        SHA224 => Ok(HashAlgorithm::Sha224),
        SHA256 => Ok(HashAlgorithm::Sha256),
        SHA384 => Ok(HashAlgorithm::Sha384),
        SHA512 => Ok(HashAlgorithm::Sha512),
        _ => Err(unsupported(algorithm.element)),
    }
}

#[derive(Clone, Copy)]
pub(super) enum SignatureAlgorithm {
    Rsa,
    Pss(PssParameters),
    Ec,
}
impl SignatureAlgorithm {
    pub(super) fn parse(algorithm: AlgorithmIdentifier<'_>, digest: HashAlgorithm) -> Result<Self> {
        let oid = algorithm.algorithm;
        if oid == RSASSA_PSS {
            let parameters = pss(algorithm
                .parameters
                .ok_or_else(|| algorithm.element.error("missing PSS signature parameters"))?)?;
            if parameters.hash() != digest {
                return Err(algorithm
                    .element
                    .error("PSS digest differs from SignerInfo digest"));
            }
            return Ok(Self::Pss(parameters));
        }
        let combined = match oid {
            SHA1_WITH_RSA | ECDSA_WITH_SHA1 => Some(HashAlgorithm::Sha1),
            SHA224_WITH_RSA | ECDSA_WITH_SHA224 => Some(HashAlgorithm::Sha224),
            SHA256_WITH_RSA | ECDSA_WITH_SHA256 => Some(HashAlgorithm::Sha256),
            SHA384_WITH_RSA | ECDSA_WITH_SHA384 => Some(HashAlgorithm::Sha384),
            SHA512_WITH_RSA | ECDSA_WITH_SHA512 => Some(HashAlgorithm::Sha512),
            _ => None,
        };
        if combined.is_some_and(|h| h != digest) {
            return Err(algorithm
                .element
                .error("signature and digest algorithms disagree"));
        }
        if oid == RSA_ENCRYPTION
            || matches!(
                oid,
                SHA1_WITH_RSA
                    | SHA224_WITH_RSA
                    | SHA256_WITH_RSA
                    | SHA384_WITH_RSA
                    | SHA512_WITH_RSA
            )
        {
            null_or_absent(algorithm)?;
            Ok(Self::Rsa)
        } else if combined.is_some() {
            if algorithm.parameters.is_some() {
                return Err(algorithm.element.error("ECDSA parameters must be absent"));
            }
            Ok(Self::Ec)
        } else {
            Err(unsupported(algorithm.element))
        }
    }
    pub(super) fn weak(self) -> bool {
        matches!(self, Self::Pss(p) if p.mgf_hash().is_weak())
    }
    pub(super) fn verify(
        self,
        key: SubjectPublicKeyInfo<'_>,
        hash: HashAlgorithm,
        digest: &[u8],
        signature: &[u8],
        limits: &Limits,
    ) -> Result<Verification> {
        if key.key.unused_bits() != 0 {
            return Err(key.element.error("signature public key has unused bits"));
        }
        match self {
            Self::Rsa | Self::Pss(_) => {
                if key.algorithm.algorithm == RSA_ENCRYPTION {
                    key.algorithm
                        .parameters
                        .ok_or_else(|| {
                            key.element
                                .error("rsaEncryption key requires NULL parameters")
                        })?
                        .expect(der::NULL)?;
                } else if key.algorithm.algorithm == RSASSA_PSS {
                    let Self::Pss(parameters) = self else {
                        return Err(key.element.error("PSS-only key used for PKCS1 signature"));
                    };
                    if let Some(restrictions) = key.algorithm.parameters {
                        let minimum = pss(restrictions)?;
                        if minimum.hash() != parameters.hash()
                            || minimum.mgf_hash() != parameters.mgf_hash()
                            || minimum.salt_bytes() > parameters.salt_bytes()
                        {
                            return Err(key
                                .element
                                .error("signature violates PSS key restrictions"));
                        }
                    }
                } else {
                    return Err(unsupported(key.algorithm.element));
                }
                let tree = Document::parse(key.key.as_bytes(), limits)?;
                let mut fields = Fields::sequence(tree.root())?;
                let n = Integer::parse(fields.take()?)?.unsigned()?;
                let e = Integer::parse(fields.take()?)?.unsigned()?;
                fields.finish()?;
                let public = RsaPublicKey::new(n, e, limits)?;
                match self {
                    Self::Pss(p) => public.verify_pss(p, digest, signature),
                    _ => public.verify_pkcs1v15(hash, digest, signature),
                }
            }
            Self::Ec => {
                if key.algorithm.algorithm != EC_PUBLIC_KEY {
                    return Err(unsupported(key.algorithm.element));
                }
                let parameter = key
                    .algorithm
                    .parameters
                    .ok_or_else(|| key.element.error("missing EC named curve"))?;
                let curve = match ObjectIdentifier::parse(parameter)? {
                    SECP256R1 => Curve::P256,
                    SECP384R1 => Curve::P384,
                    SECP521R1 => Curve::P521,
                    _ => return Err(unsupported(parameter)),
                };
                PublicKey::from_sec1(curve, key.key.as_bytes())?.verify_der(digest, signature)
            }
        }
    }
}

pub(super) fn pss(element: Element<'_>) -> Result<PssParameters> {
    let mut fields = Fields::sequence(element)?;
    let digest = fields
        .optional(Tag::context(0, Form::Constructed))
        .map(|v| hash(AlgorithmIdentifier::parse(single(v)?)?))
        .transpose()?
        .unwrap_or(HashAlgorithm::Sha1);
    let mgf = if let Some(value) = fields.optional(Tag::context(1, Form::Constructed)) {
        let algorithm = AlgorithmIdentifier::parse(single(value)?)?;
        if algorithm.algorithm != MGF1 {
            return Err(unsupported(algorithm.element));
        }
        hash(AlgorithmIdentifier::parse(
            algorithm
                .parameters
                .ok_or_else(|| value.error("missing MGF1 hash"))?,
        )?)?
    } else {
        HashAlgorithm::Sha1
    };
    let salt = fields
        .optional(Tag::context(2, Form::Constructed))
        .map(pss_integer)
        .transpose()?
        .unwrap_or(20);
    let trailer = fields
        .optional(Tag::context(3, Form::Constructed))
        .map(pss_integer)
        .transpose()?
        .unwrap_or(1);
    fields.finish()?;
    // DER defaults are checked by schema, not by the generic tree reader.
    for value in element.children() {
        let is_default = match value.tag().number {
            0 => digest == HashAlgorithm::Sha1,
            1 => mgf == HashAlgorithm::Sha1,
            2 => salt == 20,
            3 => trailer == 1,
            _ => false,
        };
        if is_default {
            return Err(value.error("explicit PSS DEFAULT parameter"));
        }
    }
    PssParameters::new(
        digest,
        mgf,
        usize::try_from(salt).map_err(|_| Error::limit("PSS salt exceeds address space"))?,
        trailer,
    )
}

/// A nonnegative PSS parameter INTEGER; one wider than 64 bits exceeds the
/// implementation's range.
fn pss_integer(value: Element<'_>) -> Result<u64> {
    let integer = Integer::parse(single(value)?)?;
    integer.unsigned()?;
    integer.to_u64().ok_or_else(|| {
        Error::limit("ASN.1 integer exceeds u64").at(Position::Buffer(value.range().start))
    })
}
