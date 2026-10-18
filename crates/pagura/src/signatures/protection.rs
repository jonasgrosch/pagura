//! Signed algorithm-identifier binding (RFC 8933 §4; RFC 6211 §2).
use super::algorithms;
use pagura_core::{ErrorKind, Result};
use pagura_pki::ObjectIdentifier;
use pagura_pki::cms::{SignedAttributes, SignerInfo};
use pagura_pki::der::{Element, Fields, Tag, single};
use pagura_pki::oids;
use pagura_pki::x509::AlgorithmIdentifier;

pub(super) fn check(attributes: &SignedAttributes<'_>, signer: &SignerInfo<'_>) -> Result<bool> {
    let Some(attribute) = attributes.unique(oids::CMS_ALGORITHM_PROTECTION)? else {
        return Ok(true);
    };
    let mut fields = Fields::sequence(single(attribute.values)?)?;
    let digest = fields.take()?.expect(pagura_pki::der::SEQUENCE)?;
    let signature = fields.take()?.expect(Tag::context(1, true))?;
    fields.finish()?; // macAlgorithm is forbidden in SignedData.
    // The signatureAlgorithm field is IMPLICIT: compare its components,
    // without rewriting or retagging bytes covered by the CMS signature.
    let digest_matches = matches_identifier(signer.digest_algorithm, digest)?;
    let signature_matches = matches_identifier(signer.signature_algorithm, signature)?;
    Ok(digest_matches && signature_matches)
}

fn matches_identifier(expected: AlgorithmIdentifier<'_>, actual: Element<'_>) -> Result<bool> {
    let mut fields = Fields::new(actual);
    let oid = ObjectIdentifier::parse(fields.take()?)?;
    let parameters = fields.rest();
    fields.finish()?;
    if oid != expected.algorithm {
        return Ok(false);
    }
    if oid.is(oids::RSASSA_PSS) {
        return match (expected.parameters, parameters) {
            (Some(expected), Some(actual)) => {
                let expected = algorithms::pss(expected)?;
                match algorithms::pss(actual) {
                    Ok(actual) => Ok(expected == actual),
                    Err(error) if matches!(error.kind, ErrorKind::Unsupported(_)) => Ok(false),
                    Err(error) => Err(error),
                }
            }
            (None, None) => Ok(true),
            _ => Ok(false),
        };
    }
    let absent_null_equivalent = matches!(
        oid.as_bytes(),
        oids::SHA1
            | oids::SHA224
            | oids::SHA256
            | oids::SHA384
            | oids::SHA512
            | oids::RSA_ENCRYPTION
            | oids::SHA1_WITH_RSA
            | oids::SHA224_WITH_RSA
            | oids::SHA256_WITH_RSA
            | oids::SHA384_WITH_RSA
            | oids::SHA512_WITH_RSA
    );
    // RFC 4055 §§2.1, 5 accepts absent/NULL hash and RSA parameters.
    // ECDSA parameters remain absent; all other parameters compare exactly.
    Ok(parameter_bytes(expected.parameters, absent_null_equivalent)
        == parameter_bytes(parameters, absent_null_equivalent))
}

fn parameter_bytes(parameter: Option<Element<'_>>, absent_null_equivalent: bool) -> Option<&[u8]> {
    parameter
        .filter(|p| !absent_null_equivalent || p.tag() != Tag::universal(5, false))
        .map(Element::encoded)
}
