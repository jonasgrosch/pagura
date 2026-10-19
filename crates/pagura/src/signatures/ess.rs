//! ESS certificate binding, RFC 5035 §§3–5 and RFC 5816 §2.2.1.
use super::{SubFilter, algorithms};
use pagura_core::Result;
use pagura_crypto::HashAlgorithm;
use pagura_pki::cms::{Attribute, SignedAttributes};
use pagura_pki::der::{Element, Fields, Form, SEQUENCE, Tag, single};
use pagura_pki::oids::{SIGNING_CERTIFICATE, SIGNING_CERTIFICATE_V2};
use pagura_pki::x509::{AlgorithmIdentifier, Certificate};
use pagura_pki::{Integer, types};

pub(super) fn check(
    attributes: &SignedAttributes<'_>,
    certificate: &Certificate<'_>,
    subfilter: SubFilter<'_>,
    weak: &mut bool,
    budget: &mut super::Budget,
) -> Result<bool> {
    let v2 = attributes.unique(SIGNING_CERTIFICATE_V2)?;
    let v1 = attributes.unique(SIGNING_CERTIFICATE)?;
    if matches!(subfilter, SubFilter::CadesDetached | SubFilter::Rfc3161)
        && v1.is_none()
        && v2.is_none()
    {
        return Err(attributes
            .element
            .error("missing required ESS signing certificate"));
    }
    let mut matches = true;
    if let Some(attribute) = v2 {
        matches &= check_attribute(&attribute, certificate, true, weak, budget)?;
    }
    if let Some(attribute) = v1 {
        matches &= check_attribute(&attribute, certificate, false, weak, budget)?;
    }
    Ok(matches)
}

/// Only the first identifier binds the signer; subsequent identifiers and
/// policies constrain path validation, which remains explicitly unchecked.
fn check_attribute(
    attribute: &Attribute<'_>,
    certificate: &Certificate<'_>,
    v2: bool,
    weak: &mut bool,
    budget: &mut super::Budget,
) -> Result<bool> {
    let mut value = Fields::sequence(single(attribute.values)?)?;
    let certs = value.take()?.expect(SEQUENCE)?;
    if certs.children().next().is_none() {
        return Err(certs.error("empty ESS certificate identifiers"));
    }
    let mut matches = true;
    for (index, id) in certs.children().enumerate() {
        let mut fields = Fields::sequence(id)?;
        let hash = if v2 {
            let algorithm = fields.optional(SEQUENCE);
            let hash = algorithm
                .map(AlgorithmIdentifier::parse)
                .transpose()?
                .map(algorithms::hash)
                .transpose()?
                .unwrap_or(HashAlgorithm::Sha256);
            if algorithm.is_some() && hash == HashAlgorithm::Sha256 {
                return Err(id.error("explicit ESS DEFAULT hash algorithm"));
            }
            hash
        } else {
            HashAlgorithm::Sha1
        };
        *weak |= hash.is_weak();
        let digest = types::octets(fields.take()?)?;
        if digest.len() != hash.digest_bytes() {
            return Err(id.error("ESS certificate hash length mismatch"));
        }
        if index == 0 {
            matches &= digest
                == super::digest_parts(hash, [certificate.element.encoded()].into_iter(), budget)?;
        }
        if let Some(issuer) = fields.optional(SEQUENCE) {
            let identified = issuer_serial(issuer, certificate)?;
            if index == 0 {
                matches &= identified;
            }
        }
        fields.finish()?;
    }
    if let Some(policies) = value.optional(SEQUENCE) {
        check_policies(policies)?;
    }
    value.finish()?;
    Ok(matches)
}

fn issuer_serial(issuer: Element<'_>, certificate: &Certificate<'_>) -> Result<bool> {
    let mut fields = Fields::sequence(issuer)?;
    let names = fields.take()?.expect(SEQUENCE)?;
    let name = single(single(names)?.expect(Tag::context(4, Form::Constructed))?)?;
    pagura_pki::x509::Name::parse(name)?;
    let serial = Integer::parse(fields.take()?)?;
    serial.unsigned()?;
    fields.finish()?;
    Ok(name.encoded() == certificate.issuer.element().encoded() && serial == certificate.serial)
}

fn check_policies(policies: Element<'_>) -> Result<()> {
    if policies.children().next().is_none() {
        return Err(policies.error("empty ESS policies"));
    }
    for policy in policies.children() {
        let mut fields = Fields::sequence(policy)?;
        pagura_pki::ObjectIdentifier::parse(fields.take()?)?;
        if let Some(qualifiers) = fields.optional(SEQUENCE) {
            if qualifiers.children().next().is_none() {
                return Err(qualifiers.error("empty ESS policy qualifiers"));
            }
            for qualifier in qualifiers.children() {
                let mut fields = Fields::sequence(qualifier)?;
                pagura_pki::ObjectIdentifier::parse(fields.take()?)?;
                fields.take()?;
                fields.finish()?;
            }
        }
        fields.finish()?;
    }
    Ok(())
}
