//! PDF CMS integrity verification (12.8.3; ADR 0015).
//!
//! [`crate::signatures()`] checks every CMS signer: all must verify. Embedded
//! certificates supply public keys, never trust anchors. Certificate paths,
//! revocation, permitted changes and timestamp clock trust remain unchecked.
//! Issuer identifiers currently require identical DER names; no Unicode name
//! normalization or certificate substitution is inferred.
//!
//! Work is bounded by the document's `Limits`, with additional ceilings of 64
//! CMS signers and 256 embedded certificates per call. Parsing and hash work
//! share aggregate byte budgets; RSA uses the host's arithmetic limits per
//! verification. There are no network or filesystem capabilities.
mod algorithms;
mod ess;
mod protection;

/// Stable unsupported-feature tags for signature verification.
pub mod features {
    use pagura_core::Feature;
    /// A digest, signature or public-key algorithm outside the implemented set.
    pub const ALGORITHM: Feature = Feature::new("signature.algorithm");
    /// Every tag of this module.
    pub const ALL: &[Feature] = &[ALGORITHM];
}

use crate::Document;
use pagura_core::{Diagnostics, Error, ErrorKind, Limits, Meter, Result};
use pagura_crypto::{HashAlgorithm, Verification};
pub use pagura_document::signatures::*;
use pagura_pki::cms::{CertificateChoice, ID_DATA, SignedData, SignerIdentifier, SignerInfo};
use pagura_pki::timestamp::{ID_TST_INFO, TstInfo};
use pagura_pki::x509::{Certificate, extensions};
use pagura_pki::{der, oids, types};

#[derive(Default)]
struct CheckState {
    weak: bool,
    certificate_errors: Vec<Error>,
}

/// The most CMS signers checked in one call.
const MAX_SIGNERS: usize = 64;
/// The most embedded certificates read in one call.
const MAX_CERTIFICATES: usize = 256;

/// The work of one call, shared by its signatures and charged before each
/// step.
struct Budget {
    signers: Meter,
    certificates: Meter,
    /// CMS bytes parsed, within the document's storage budget.
    parsed_bytes: Meter,
    /// Bytes hashed: every signer may hash at most the whole file.
    hash_bytes: Meter,
}

impl Budget {
    fn new(limits: &Limits) -> Self {
        let message = "signature verification budget exceeded";
        Self {
            signers: Meter::new(MAX_SIGNERS, message),
            certificates: Meter::new(MAX_CERTIFICATES, message),
            parsed_bytes: Meter::new(limits.document_bytes, message),
            hash_bytes: Meter::new(limits.file_bytes.saturating_mul(MAX_SIGNERS), message),
        }
    }
}

pub(crate) fn verify(document: &Document) -> Result<Signatures<'_>> {
    let mut signatures = document.signatures()?;
    let limits = document.limits();
    let mut budget = Budget::new(&limits);
    for signature in signatures
        .fields
        .iter_mut()
        .filter_map(|f| f.signature.as_mut())
        .chain(signatures.usage_rights.iter_mut())
    {
        verify_signature(signature, document, &limits, &mut budget);
    }
    if let Some(certification) = &mut signatures.certification {
        // A direct dictionary's object ID can identify its enclosing object.
        // Reuse only the same cached dictionary, not another dictionary in it.
        let field = certification.object.and_then(|object| {
            signatures
                .fields
                .iter()
                .filter_map(|field| field.signature.as_ref())
                .find(|signature| {
                    signature.object == Some(object)
                        && std::ptr::eq(signature.dictionary, certification.dictionary)
                })
        });
        if let Some(field) = field {
            certification.results.clone_from(&field.results);
        } else {
            verify_signature(certification, document, &limits, &mut budget);
        }
    }
    Ok(signatures)
}

fn verify_signature(
    signature: &mut Signature<'_>,
    document: &Document,
    limits: &Limits,
    budget: &mut Budget,
) {
    if signature.byte_range.is_none() {
        return;
    }
    if !matches!(
        signature.sub_filter,
        SubFilter::Pkcs7Detached | SubFilter::CadesDetached | SubFilter::Rfc3161
    ) {
        signature.results.integrity = Integrity::UnsupportedEncoding;
        return;
    }
    let mut diagnostics = Diagnostics::new(document.recovery());
    let mut state = CheckState::default();
    let result = check(signature, limits, budget, &mut diagnostics, &mut state);
    signature.results.diagnostics = diagnostics.take();
    signature.results.certificate_errors = state.certificate_errors;
    signature.results.algorithms = if state.weak {
        AlgorithmStatus::WeakSha1
    } else {
        AlgorithmStatus::NotCheckedYet
    };
    signature.results.integrity = match result {
        Ok(integrity) => {
            if integrity == Integrity::Verified && !state.weak {
                signature.results.algorithms = AlgorithmStatus::Supported;
            }
            integrity
        }
        Err(error) => {
            let integrity = match error.kind {
                ErrorKind::Unsupported(feature) if feature == features::ALGORITHM => {
                    signature.results.algorithms = if state.weak {
                        AlgorithmStatus::WeakSha1UnsupportedAlgorithm
                    } else {
                        AlgorithmStatus::UnsupportedAlgorithm
                    };
                    Integrity::UnsupportedAlgorithm
                }
                ErrorKind::Unsupported(_) => Integrity::UnsupportedEncoding,
                ErrorKind::Limit => Integrity::ResourceLimit,
                _ => Integrity::Malformed,
            };
            signature.results.error = Some(match signature.object {
                Some(object) => error.in_object(object),
                None => error,
            });
            integrity
        }
    };
}

fn check(
    signature: &Signature<'_>,
    limits: &Limits,
    budget: &mut Budget,
    diagnostics: &mut Diagnostics,
    state: &mut CheckState,
) -> Result<Integrity> {
    budget.parsed_bytes.charge(signature.contents.len())?;
    let (tree, used) = der::Document::parse_ber_prefix(signature.contents, limits, diagnostics)?;
    if signature.contents[used..].iter().any(|&b| b != 0) {
        return Err(tree.root().error("nonzero PDF Contents padding"));
    }
    let data = SignedData::from_content_info_reported(tree.root(), diagnostics)?;
    if data.signers.is_empty() {
        return Err(data.element.error("PDF signature has no CMS signer"));
    }
    if matches!(
        signature.sub_filter,
        SubFilter::CadesDetached | SubFilter::Rfc3161
    ) && data.signers.len() != 1
    {
        return Err(data
            .element
            .error("CAdES and document timestamps require exactly one signer"));
    }
    budget.signers.charge(data.signers.len())?;
    budget.certificates.charge(data.certificates.len())?;
    state
        .certificate_errors
        .extend(data.certificates.iter().filter_map(|choice| {
            if let CertificateChoice::Malformed { error, .. } = choice {
                Some(error.clone())
            } else {
                None
            }
        }));
    let range = signature
        .byte_range
        .as_ref()
        .ok_or_else(|| data.element.error("missing checked byte ranges"))?;
    let content = if matches!(signature.sub_filter, SubFilter::Rfc3161) {
        if !data.content_type.is(ID_TST_INFO) {
            return Err(data
                .element
                .error("document timestamp content is not TSTInfo"));
        }
        let bytes = data
            .content
            .ok_or_else(|| data.element.error("timestamp has no encapsulated content"))?
            .to_bytes()?;
        let tree = der::Document::parse(&bytes, limits)?;
        let info = TstInfo::parse(tree.root())?;
        let hash = algorithms::hash(info.message_imprint.algorithm)?;
        state.weak |= hash.is_weak();
        if info.message_imprint.digest.len() != hash.digest_bytes() {
            return Err(info.element.error("timestamp imprint length mismatch"));
        }
        if info.message_imprint.digest != digest_parts(hash, range.signed_bytes(), budget)? {
            return Ok(Integrity::DigestMismatch);
        }
        Some(bytes)
    } else {
        if !data.content_type.is(ID_DATA) || data.content.is_some() {
            return Err(data
                .element
                .error("detached PDF CMS must contain detached id-data"));
        }
        None
    };
    for signer in &data.signers {
        let result = check_signer(
            signature,
            &data,
            signer,
            content.as_deref(),
            limits,
            budget,
            state,
        )?;
        if result != Integrity::Verified {
            return Ok(result);
        }
    }
    Ok(Integrity::Verified)
}

fn check_signer(
    signature: &Signature<'_>,
    data: &SignedData<'_>,
    signer: &SignerInfo<'_>,
    content: Option<&[u8]>,
    limits: &Limits,
    budget: &mut Budget,
    state: &mut CheckState,
) -> Result<Integrity> {
    let range = signature
        .byte_range
        .as_ref()
        .ok_or_else(|| data.element.error("missing checked byte ranges"))?;
    let hash = algorithms::hash(signer.digest_algorithm)?;
    state.weak |= hash.is_weak();
    if !data
        .digest_algorithms
        .iter()
        .any(|&algorithm| algorithms::hash(algorithm).is_ok_and(|declared| declared == hash))
    {
        return Err(data
            .element
            .error("SignerInfo digest absent from SignedData digestAlgorithms"));
    }
    let digest = if let Some(content) = content {
        digest_parts(hash, [content].into_iter(), budget)?
    } else {
        digest_parts(hash, range.signed_bytes(), budget)?
    };
    let Some(certificate) = certificate(data, signer, &mut state.certificate_errors)? else {
        return Ok(Integrity::SignerCertificateMissing);
    };
    if signer
        .unsigned_attributes
        .iter()
        .any(|a| a.id.is(oids::SIGNING_CERTIFICATE_V2) || a.id.is(oids::SIGNING_CERTIFICATE))
    {
        return Err(data.element.error("ESS signing certificate must be signed"));
    }
    let signed_digest = if let Some(attributes) = &signer.signed_attributes {
        if !protection::check(attributes, signer)? {
            return Ok(Integrity::SignatureInvalid);
        }
        let claimed = attributes.content_digest(data.content_type)?;
        if claimed.len() != hash.digest_bytes() {
            return Err(attributes
                .element
                .error("CMS messageDigest length mismatch"));
        }
        if claimed != digest {
            return Ok(Integrity::DigestMismatch);
        }
        if !ess::check(
            attributes,
            certificate,
            signature.sub_filter,
            &mut state.weak,
            budget,
        )? {
            return Ok(Integrity::SignatureInvalid);
        }
        digest_parts(hash, [attributes.signature_input()].into_iter(), budget)?
    } else {
        if matches!(signature.sub_filter, SubFilter::CadesDetached) {
            return Err(data.element.error("CAdES requires signed attributes"));
        }
        digest
    };
    let algorithm = algorithms::SignatureAlgorithm::parse(signer.signature_algorithm, hash)?;
    state.weak |= algorithm.weak();
    let signature_bytes = signer.signature.to_bytes()?;
    match algorithm.verify(
        certificate.public_key,
        hash,
        &signed_digest,
        &signature_bytes,
        limits,
    )? {
        Verification::Valid => Ok(Integrity::Verified),
        Verification::Invalid => Ok(Integrity::SignatureInvalid),
    }
}

fn certificate<'a>(
    data: &'a SignedData<'a>,
    signer: &SignerInfo<'_>,
    errors: &mut Vec<Error>,
) -> Result<Option<&'a Certificate<'a>>> {
    let mut found: Option<&Certificate<'_>> = None;
    for choice in &data.certificates {
        let CertificateChoice::X509(candidate) = choice else {
            continue;
        };
        let matches = match signer.sid {
            SignerIdentifier::IssuerAndSerial { issuer, serial } => {
                candidate.serial == serial
                    && candidate.issuer.element().encoded() == issuer.element().encoded()
            }
            SignerIdentifier::SubjectKeyIdentifier(ski) => {
                if let Some(extension) = candidate
                    .extensions
                    .iter()
                    .find(|e| e.id.is(extensions::SUBJECT_KEY_IDENTIFIER))
                {
                    let matched: Result<bool> = (|| {
                        let tree = extension.value_der()?;
                        Ok(types::octets(tree.root())? == ski)
                    })();
                    match matched {
                        Ok(matched) => matched,
                        Err(error) if error.kind == ErrorKind::Limit => return Err(error),
                        Err(error) => {
                            errors.push(error);
                            false
                        }
                    }
                } else {
                    false
                }
            }
        };
        if matches {
            if found.is_some_and(|old| old.element.encoded() != candidate.element.encoded()) {
                return Err(data.element.error("ambiguous CMS signer certificate"));
            }
            found = Some(candidate);
        }
    }
    Ok(found)
}

fn digest_parts<'a>(
    algorithm: HashAlgorithm,
    parts: impl Iterator<Item = &'a [u8]> + Clone,
    budget: &mut Budget,
) -> Result<Vec<u8>> {
    for part in parts.clone() {
        budget.hash_bytes.charge(part.len())?;
    }
    Ok(algorithm.digest_parts(parts))
}
