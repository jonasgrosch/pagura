use super::parse::{self, DpName, Info, document, malformed, unknown};
use super::{RevocationOutcome, UnknownReason};
use crate::validation::budget::Budget;
use crate::validation::names::Dn;
use crate::validation::{
    self, Check, EffectivePublicKey, InvalidReason, Issue, Oid, PathReport, PathState,
    SignatureInput, SignatureResult, SignatureVerifier, TrustAnchor, Uncertainty,
    ValidationOptions, WorkingPublicKey,
};
use crate::x509::extensions::{
    AUTHORITY_KEY_IDENTIFIER, AuthorityKeyIdentifier, GeneralName, KeyUsage, SUBJECT_KEY_IDENTIFIER,
};
use crate::x509::{Certificate, CertificateList};
use crate::{Time, types};

pub(super) struct Authorization {
    pub path: Vec<usize>,
    pub key: EffectivePublicKey,
    pub active: bool,
}
pub(super) struct Cached {
    pub info: Option<Info>,
    pub failure: UnknownReason,
    pub authorizations: Vec<Authorization>,
}
impl Cached {
    pub(super) fn active(&self) -> bool {
        self.authorizations.iter().any(|a| a.active)
    }
}
pub(super) struct Engine<'c, 'a> {
    pub certificates: &'c [&'c Certificate<'a>],
    pub crls: &'c [&'c CertificateList<'a>],
    pub cache: Vec<Cached>,
    pub time: Time<'c>,
}

#[derive(Clone, Copy)]
pub(in crate::validation) struct Evidence<'c, 'a> {
    pub certificates: &'c [&'c Certificate<'a>],
    pub crls: &'c [&'c CertificateList<'a>],
}

pub(in crate::validation) fn resolve(
    report: &mut PathReport,
    path: &[&Certificate<'_>],
    evidence: Evidence<'_, '_>,
    anchor: TrustAnchor<'_>,
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    budget: &mut Budget<'_>,
) -> Option<Uncertainty> {
    let mut processed = 0;
    let result = (|| -> Check<()> {
        budget.bytes(path.len().saturating_mul(size_of::<RevocationOutcome>()))?;
        report
            .revocation
            .try_reserve_exact(path.len())
            .map_err(|_| Issue::Unknown(Uncertainty::Limit("pki.allocation")))?;
        report.revocation.resize(
            path.len(),
            RevocationOutcome::Unknown(UnknownReason::NoData),
        );
        let mut pool: Vec<&Certificate<'_>> = Vec::new();
        for &certificate in evidence.certificates.iter().chain(path.iter()) {
            budget.work(pool.len())?;
            if !pool.iter().any(|&c| std::ptr::eq(c, certificate)) {
                budget.push(&mut pool, certificate)?;
            }
        }
        let mut engine = Engine {
            certificates: &pool,
            crls: evidence.crls,
            cache: Vec::new(),
            time: options.time,
        };
        engine.prepare(anchor, options, verifier, budget)?;
        engine.prune(budget)?;
        for &certificate in path {
            let outcome = engine.status(certificate, budget)?;
            report.revocation[processed] = outcome;
            processed += 1;
        }
        Ok(())
    })();
    if let Err(issue) = &result {
        let reason = match issue {
            Issue::Unknown(Uncertainty::Limit(name)) => UnknownReason::Limit(name),
            Issue::Unknown(Uncertainty::RevocationData(reason)) => *reason,
            Issue::Unknown(Uncertainty::NameForm) => UnknownReason::NameForm,
            _ => UnknownReason::SignerUntrusted,
        };
        for outcome in &mut report.revocation[processed..] {
            *outcome = RevocationOutcome::Unknown(reason);
        }
    }
    let stopped = match &result {
        Err(Issue::Unknown(Uncertainty::Limit(name))) => Some(Uncertainty::Limit(name)),
        _ => None,
    };
    finish(report, path.len(), result);
    stopped
}
fn finish(report: &mut PathReport, length: usize, result: Check<()>) {
    report.state = report.basic.clone();
    if !matches!(report.state, PathState::Invalid { .. }) {
        // A CRL-dependent critical certificate extension is resolved only after
        // the stage parsed its semantics and established every status as Good.
        let crl_only = matches!(
            report.basic,
            PathState::Indeterminate {
                reason: Uncertainty::Revocation,
                ..
            }
        );
        if report.revocation.len() == length
            && result.is_ok()
            && report
                .revocation
                .iter()
                .all(|r| *r == RevocationOutcome::Good)
        {
            if crl_only {
                report.state = PathState::Valid;
            }
        } else if report.state == PathState::Valid || crl_only {
            let first = report.revocation.iter().enumerate().find_map(|(i, r)| {
                if let RevocationOutcome::Unknown(reason) = r {
                    Some((i, *reason))
                } else {
                    None
                }
            });
            report.state = if let Some((index, reason)) = first {
                validation::path::state(unknown(reason), Some(index))
            } else {
                validation::path::state(unknown(UnknownReason::NoData), None)
            };
        }
        for (index, outcome) in report.revocation.iter().enumerate() {
            if matches!(outcome, RevocationOutcome::Revoked { .. }) {
                report.state = PathState::Invalid {
                    certificate: Some(index),
                    reason: InvalidReason::Revoked,
                };
                break;
            }
        }
        if let Err(issue) = result
            && !matches!(report.state, PathState::Invalid { .. })
        {
            report.state = validation::path::state(issue, None);
        }
    }
}

impl Engine<'_, '_> {
    fn prepare(
        &mut self,
        anchor: TrustAnchor<'_>,
        options: &ValidationOptions<'_>,
        verifier: &mut impl SignatureVerifier,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        let anchor_name = Dn::read(anchor.name, budget)?;
        let mut signer_options = ValidationOptions::new(options.time);
        signer_options.key_usage = KeyUsage::CRL_SIGN;
        signer_options.search = validation::SearchMode::All;
        signer_options.name_constraints = options.name_constraints;
        budget.bytes(options.denied_name_forms.len().saturating_mul(8))?;
        signer_options
            .denied_name_forms
            .clone_from(&options.denied_name_forms);
        for &crl in self.crls {
            budget.node()?;
            let parsed = Info::read(crl, budget).and_then(|info| {
                parse::entry_extensions(crl, &info, budget)?;
                Ok(info)
            });
            let mut cached = Cached {
                info: None,
                failure: UnknownReason::SignerUntrusted,
                authorizations: Vec::new(),
            };
            match parsed {
                Ok(info) => cached.info = Some(info),
                Err(issue) => {
                    cached.failure = failure(&issue)?;
                    budget.push(&mut self.cache, cached)?;
                    continue;
                }
            }
            let info = cached.info.as_ref().ok_or_else(malformed)?;
            if candidate_name(info.issuer.equal(&anchor_name, budget), &mut cached.failure)? {
                cached.failure = UnknownReason::VerificationFailed;
                let key = validation::path::key(anchor.public_key);
                if verify(crl, key, verifier, budget)? {
                    let key = owned_key(key, budget)?;
                    budget.push(
                        &mut cached.authorizations,
                        Authorization {
                            path: Vec::new(),
                            key,
                            active: true,
                        },
                    )?;
                }
            }
            self.signers(crl, anchor, &signer_options, verifier, &mut cached, budget)?;
            budget.push(&mut self.cache, cached)?;
        }
        Ok(())
    }
    fn signers(
        &self,
        crl: &CertificateList<'_>,
        anchor: TrustAnchor<'_>,
        options: &ValidationOptions<'_>,
        verifier: &mut impl SignatureVerifier,
        cached: &mut Cached,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        let issuer = Dn::read(crl.issuer, budget)?;
        for (i, &certificate) in self.certificates.iter().enumerate() {
            budget.node()?;
            let matches = (|| {
                Ok(
                    Dn::read(certificate.subject, budget)?.equal(&issuer, budget)?
                        && authority_matches(crl, certificate, budget)?,
                )
            })();
            if !candidate_name(matches, &mut cached.failure)? {
                continue;
            }
            let search = validation::build::with_budget(
                i,
                self.certificates,
                &[anchor],
                options,
                verifier,
                budget,
            );
            if let Some(Uncertainty::Limit(name)) = search.stopped {
                return Err(Issue::Unknown(Uncertainty::Limit(name)));
            }
            for built in search.paths {
                if !acceptable(&built.report.basic) || built.anchor.is_none() {
                    continue;
                }
                let mut key = validation::path::key(anchor.public_key);
                for &j in &built.certificates {
                    let mut next = validation::path::key(self.certificates[j].public_key);
                    budget.work(next.algorithm.as_bytes().len())?;
                    if next.parameters.is_none() && next.algorithm == key.algorithm {
                        next.parameters = key.parameters;
                    }
                    key = next;
                }
                cached.failure = UnknownReason::VerificationFailed;
                if verify(crl, key, verifier, budget)? {
                    let key = owned_key(key, budget)?;
                    budget.push(
                        &mut cached.authorizations,
                        Authorization {
                            path: built.certificates,
                            key,
                            active: true,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
    fn prune(&mut self, budget: &mut Budget<'_>) -> Check<()> {
        // Start with anchored mathematical authorisations, then eliminate those
        // whose signer path lacks Good status. This permits a CRL signing key's
        // own CRL to cover its certificate, while a revoked signer can never
        // supply Good evidence. Each changing pass removes an authorisation.
        loop {
            let mut statuses = Vec::new();
            for &certificate in self.certificates {
                let status = self.status(certificate, budget)?;
                budget.push(&mut statuses, status)?;
            }
            let mut changed = false;
            for cached in &mut self.cache {
                for auth in &mut cached.authorizations {
                    budget.node()?;
                    if !auth.active {
                        continue;
                    }
                    for &i in &auth.path {
                        budget.work(1)?;
                        if statuses[i] != RevocationOutcome::Good {
                            auth.active = false;
                            cached.failure = UnknownReason::SignerUntrusted;
                            changed = true;
                            break;
                        }
                    }
                }
            }
            if !changed {
                return Ok(());
            }
        }
    }
}
fn acceptable(state: &PathState) -> bool {
    matches!(
        state,
        PathState::Valid
            | PathState::Indeterminate {
                reason: Uncertainty::Revocation,
                ..
            }
    )
}
fn failure(issue: &Issue) -> Check<UnknownReason> {
    match issue {
        Issue::Unknown(Uncertainty::Limit(name)) => Err(Issue::Unknown(Uncertainty::Limit(name))),
        Issue::Unknown(Uncertainty::RevocationData(reason)) => Ok(*reason),
        Issue::Unknown(Uncertainty::NameForm) => Ok(UnknownReason::NameForm),
        _ => Ok(UnknownReason::Malformed),
    }
}
fn candidate_name(result: Check<bool>, reason: &mut UnknownReason) -> Check<bool> {
    match result {
        Ok(matched) => Ok(matched),
        Err(issue) => {
            *reason = failure(&issue)?;
            Ok(false)
        }
    }
}
fn owned_key(key: WorkingPublicKey<'_>, budget: &mut Budget<'_>) -> Check<EffectivePublicKey> {
    let parameters = key.parameters.map(crate::der::Element::encoded);
    budget.bytes(
        key.algorithm.as_bytes().len()
            + key.key.as_bytes().len()
            + parameters.map_or(0, <[u8]>::len),
    )?;
    Ok(EffectivePublicKey {
        algorithm: Oid::new(key.algorithm),
        parameters: parameters.map(<[u8]>::to_vec),
        key: key.key.as_bytes().to_vec(),
        unused_bits: key.key.unused_bits(),
    })
}
fn verify(
    crl: &CertificateList<'_>,
    key: WorkingPublicKey<'_>,
    verifier: &mut impl SignatureVerifier,
    budget: &mut Budget<'_>,
) -> Check<bool> {
    Ok(budget.verify(
        SignatureInput {
            algorithm: crl.signature_algorithm,
            message: crl.tbs.encoded(),
            signature: crl.signature,
            issuer_key: key,
        },
        verifier,
    )? == SignatureResult::Verified)
}
fn authority_matches(
    crl: &CertificateList<'_>,
    certificate: &Certificate<'_>,
    budget: &mut Budget<'_>,
) -> Check<bool> {
    let Some(ext) = crl
        .extensions
        .iter()
        .find(|e| e.id() == AUTHORITY_KEY_IDENTIFIER)
        .copied()
    else {
        return Ok(true);
    };
    let doc = document(ext, budget)?;
    let aki = AuthorityKeyIdentifier::parse(doc.root()).map_err(|_| malformed())?;
    if let Some(id) = aki.key_identifier
        && let Some(ski) = certificate.extension(SUBJECT_KEY_IDENTIFIER)
    {
        let doc = document(ski, budget)?;
        let ski = types::octets(doc.root()).map_err(|_| malformed())?;
        budget.work(id.len())?;
        if id != ski {
            return Ok(false);
        }
    }
    // The schema pairs the issuer names with the serial (RFC 5280 4.2.1.1).
    if let Some(authority) = aki.certificate {
        let serial = authority.serial;
        budget.work(serial.as_bytes().len())?;
        if serial != certificate.serial {
            return Ok(false);
        }
        let issuer = Dn::read(certificate.issuer, budget)?;
        let mut matched = false;
        for &name in &authority.issuer {
            if let GeneralName::Directory(name) = name {
                matched |= issuer.equal(&Dn::read(name, budget)?, budget)?;
            }
        }
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn issuer_matches(
    names: &[DpName],
    issuer: &Dn,
    budget: &mut Budget<'_>,
) -> Check<bool> {
    for name in names {
        if let DpName::Directory(name) = name
            && name.equal(issuer, budget)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}
