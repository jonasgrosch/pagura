use super::budget::Budget;
use super::extensions::Prepared;
use super::names::{Constraints, Dn};
use super::policy::Graph;
use super::{
    Check, InvalidReason, Issue, Oid, PathReport, PathState, SignatureInput, SignatureResult,
    SignatureVerifier, TrustAnchor, Uncertainty, ValidationOptions, WorkingPublicKey,
};
use crate::ObjectIdentifier;
use crate::der::{self, Element};
use crate::x509::extensions::{self as ext, KeyUsage};
use crate::x509::{Certificate, SubjectPublicKeyInfo};
use pagura_core::Limits;

/// Validates a root-to-target path, excluding the explicitly supplied anchor.
/// Returns basic validation separately from the initially missing revocation.
/// All resources, including policy graph edges, share one operation budget.
pub fn validate_path(
    path: &[&Certificate<'_>],
    anchor: TrustAnchor<'_>,
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    limits: &Limits,
) -> PathReport {
    validate(path, anchor, options, verifier, &mut Budget::new(limits))
}

pub(super) fn state(issue: Issue, certificate: Option<usize>) -> PathState {
    match issue {
        Issue::Invalid(reason) => PathState::Invalid {
            certificate,
            reason,
        },
        Issue::Unknown(reason) => PathState::Indeterminate {
            certificate,
            reason,
        },
    }
}
pub(super) fn report(basic: PathState, policies: Vec<Oid>, certificates: usize) -> PathReport {
    let state = if basic == PathState::Valid {
        PathState::Indeterminate {
            certificate: None,
            reason: Uncertainty::Revocation,
        }
    } else {
        basic.clone()
    };
    PathReport {
        basic,
        state,
        policies,
        certificates,
        effective_public_key: None,
        revocation: Vec::new(),
    }
}

pub(super) fn validate(
    path: &[&Certificate<'_>],
    anchor: TrustAnchor<'_>,
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    budget: &mut Budget<'_>,
) -> PathReport {
    let mut position = None;
    let mut check = || -> Check<(Vec<Oid>, Option<PathState>, super::EffectivePublicKey)> {
        if path.is_empty() {
            return Err(Issue::Invalid(InvalidReason::Input));
        }
        // A Time holds checked calendar values, from DER or Time::from_utc.
        budget.string(options.time.fraction().len())?;
        budget.depth(path.len())?;
        budget.work(options.policies.len())?;
        budget.nodes(options.policies.len())?;
        budget.bytes(options.policies.iter().fold(0usize, |n, p| {
            n.saturating_add(p.0.len()).saturating_add(32)
        }))?;
        let mut validator = Validator::new(anchor, options, path.len(), budget)?;
        for (index, &certificate) in path.iter().enumerate() {
            position = Some(index);
            validator.step(certificate, index, path.len(), verifier, budget)?;
        }
        let policies = validator
            .policies
            .finish(&options.policies, path.len(), budget)?;
        let key = validator.working_key;
        let parameters = key.parameters.map(Element::encoded);
        budget.bytes(
            key.algorithm.as_bytes().len()
                + key.key.as_bytes().len()
                + parameters.map_or(0, <[u8]>::len),
        )?;
        let key = super::EffectivePublicKey {
            algorithm: Oid::new(key.algorithm),
            parameters: parameters.map(<[u8]>::to_vec),
            key: key.key.as_bytes().to_vec(),
            unused_bits: key.key.unused_bits(),
        };
        let unresolved = validator.unresolved.or_else(|| {
            validator
                .deferred_crl
                .map(|index| state(Issue::Unknown(Uncertainty::Revocation), Some(index)))
        });
        Ok((policies, unresolved, key))
    };
    match check() {
        Ok((policies, unresolved, key)) => {
            let mut result = report(unresolved.unwrap_or(PathState::Valid), policies, path.len());
            result.effective_public_key = Some(key);
            result
        }
        Err(issue) => report(state(issue, position), Vec::new(), path.len()),
    }
}

struct Validator<'a, 'o, 'v> {
    working_name: Dn,
    working_key: WorkingPublicKey<'a>,
    constraints: Constraints,
    policies: Graph,
    max_length: usize,
    options: &'o ValidationOptions<'v>,
    // CRL semantics can be discharged by revocation; other uncertainties cannot.
    unresolved: Option<PathState>,
    deferred_crl: Option<usize>,
}
impl<'a, 'o, 'v> Validator<'a, 'o, 'v> {
    fn new(
        anchor: TrustAnchor<'a>,
        options: &'o ValidationOptions<'v>,
        length: usize,
        budget: &mut Budget<'_>,
    ) -> Check<Self> {
        let mut constraints = Constraints::default();
        constraints.deny(&options.denied_name_forms, budget)?;
        if let Some(initial) = options.name_constraints {
            constraints.add(initial, budget)?;
        }
        Ok(Self {
            working_name: Dn::read(anchor.name, budget)?,
            working_key: key(anchor.public_key),
            constraints,
            policies: Graph::new(length, options, budget)?,
            max_length: length,
            options,
            unresolved: None,
            deferred_crl: None,
        })
    }
    fn step(
        &mut self,
        certificate: &Certificate<'a>,
        index: usize,
        length: usize,
        verifier: &mut impl SignatureVerifier,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        // Verify before extending the policy graph (RFC 9618 6.1).
        match budget.verify(
            SignatureInput {
                algorithm: certificate.signature_algorithm,
                message: certificate.tbs.encoded(),
                signature: certificate.signature,
                issuer_key: self.working_key,
            },
            verifier,
        )? {
            SignatureResult::Verified => (),
            SignatureResult::Invalid => return Err(Issue::Invalid(InvalidReason::Signature)),
            SignatureResult::Unavailable => self.uncertain(Uncertainty::Signature, index),
        }
        if self.options.time < certificate.not_before {
            return Err(Issue::Invalid(InvalidReason::NotYetValid));
        }
        if self.options.time > certificate.not_after {
            return Err(Issue::Invalid(InvalidReason::Expired));
        }
        let prepared = Prepared::read(certificate, budget)?;
        if !prepared.issuer.equal(&self.working_name, budget)? {
            return Err(Issue::Invalid(InvalidReason::IssuerName));
        }
        let target = index + 1 == length;
        // Compare self-issued names only when the distinction can affect
        // constraints or a counter before this path ends. Opaque subject names
        // need no matching rule for an otherwise unconstrained path.
        let remaining = length - index;
        let relevant = !self.constraints.is_empty()
            || self.max_length < remaining
            || self.policies.explicit <= remaining
            || self.policies.mapping <= remaining
            || self.policies.any <= remaining;
        let self_issued = !target && relevant && prepared.self_issued(budget)?;
        if !self_issued || target {
            self.constraints.check_certificate(certificate, budget)?;
        }
        if let Some(purpose) = &self.options.extended_key_usage
            && prepared.eku.as_ref().is_some_and(|eku| {
                !eku.contains(purpose) && !eku.iter().any(|p| *p == ext::ANY_EXTENDED_KEY_USAGE)
            })
        {
            return Err(Issue::Invalid(InvalidReason::ExtendedKeyUsage));
        }
        if prepared.deferred_critical {
            self.deferred_crl.get_or_insert(index);
        }
        self.policies.process(
            prepared.policies.as_deref(),
            index + 1,
            self_issued,
            target,
            budget,
        )?;
        if target {
            target_usage(&prepared, self.options)?;
            self.policies.explicit = self.policies.explicit.saturating_sub(1);
            if prepared.explicit == Some(0) {
                self.policies.explicit = 0;
            }
        } else {
            self.prepare_next(&prepared, index + 1, self_issued, budget)?;
        }
        self.working_name = prepared.subject;
        let mut next_key = key(certificate.public_key);
        if next_key.parameters.is_none() && next_key.algorithm == self.working_key.algorithm {
            next_key.parameters = self.working_key.parameters;
        }
        self.working_key = next_key;
        Ok(())
    }
    fn prepare_next(
        &mut self,
        prepared: &Prepared,
        depth: usize,
        self_issued: bool,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        self.policies.mappings(&prepared.mappings, depth, budget)?;
        if !prepared.ca {
            return Err(Issue::Invalid(InvalidReason::NotCa));
        }
        if prepared
            .usage
            .is_some_and(|usage| !usage.contains(KeyUsage::KEY_CERT_SIGN))
        {
            return Err(Issue::Invalid(InvalidReason::KeyCertSign));
        }
        if !self_issued {
            if self.max_length == 0 {
                return Err(Issue::Invalid(InvalidReason::PathLength));
            }
            self.max_length -= 1;
            self.policies.decrement();
        }
        if let Some(length) = prepared.path_len {
            self.max_length = self.max_length.min(length);
        }
        if let Some(n) = prepared.explicit {
            self.policies.explicit = self.policies.explicit.min(n);
        }
        if let Some(n) = prepared.mapping {
            self.policies.mapping = self.policies.mapping.min(n);
        }
        if let Some(n) = prepared.inhibit_any {
            self.policies.any = self.policies.any.min(n);
        }
        self.constraints.append(&prepared.constraints, budget)
    }
    fn uncertain(&mut self, reason: Uncertainty, index: usize) {
        if self.unresolved.is_none() {
            self.unresolved = Some(state(Issue::Unknown(reason), Some(index)));
        }
    }
}

pub(super) fn key(spki: SubjectPublicKeyInfo<'_>) -> WorkingPublicKey<'_> {
    WorkingPublicKey {
        algorithm: spki.algorithm.algorithm,
        parameters: spki.algorithm.parameters.filter(|p| p.tag() != der::NULL),
        key: spki.key,
    }
}

/// The keyUsage bits compatible with each known purpose (RFC 5280 4.2.1.12).
const PURPOSE_USAGES: [(ObjectIdentifier<'static>, KeyUsage); 6] = [
    (
        ext::SERVER_AUTH,
        KeyUsage::DIGITAL_SIGNATURE
            .union(KeyUsage::KEY_ENCIPHERMENT)
            .union(KeyUsage::KEY_AGREEMENT),
    ),
    (
        ext::CLIENT_AUTH,
        KeyUsage::DIGITAL_SIGNATURE.union(KeyUsage::KEY_AGREEMENT),
    ),
    (ext::CODE_SIGNING, KeyUsage::DIGITAL_SIGNATURE),
    (
        ext::EMAIL_PROTECTION,
        KeyUsage::DIGITAL_SIGNATURE
            .union(KeyUsage::CONTENT_COMMITMENT)
            .union(KeyUsage::KEY_ENCIPHERMENT)
            .union(KeyUsage::KEY_AGREEMENT),
    ),
    (
        ext::TIME_STAMPING,
        KeyUsage::DIGITAL_SIGNATURE.union(KeyUsage::CONTENT_COMMITMENT),
    ),
    (
        ext::OCSP_SIGNING,
        KeyUsage::DIGITAL_SIGNATURE.union(KeyUsage::CONTENT_COMMITMENT),
    ),
];

fn target_usage(prepared: &Prepared, options: &ValidationOptions<'_>) -> Check<()> {
    let Some(usage) = prepared.usage else {
        return Ok(());
    };
    if !usage.contains(options.key_usage) {
        return Err(Issue::Invalid(InvalidReason::KeyUsage));
    }
    let Some(purpose) = &options.extended_key_usage else {
        return Ok(());
    };
    if *purpose == ext::ANY_EXTENDED_KEY_USAGE {
        return Ok(());
    }
    // RFC 5280 4.2.1.12: an EKU never overrides an incompatible keyUsage.
    let allowed = PURPOSE_USAGES
        .iter()
        .find(|(id, _)| *purpose == *id)
        .map(|&(_, allowed)| allowed);
    match allowed {
        Some(allowed) if !usage.intersects(allowed) => Err(Issue::Invalid(InvalidReason::KeyUsage)),
        Some(_) => Ok(()),
        None if !options.key_usage.is_empty() => Ok(()),
        None => Err(Issue::Unknown(Uncertainty::KeyUsagePurpose)),
    }
}
