use super::budget::Budget;
use super::names::Dn;
use super::path::{report, state, validate};
use super::{
    BuiltPath, Check, InvalidReason, Issue, PathSearch, PathState, SignatureVerifier, TrustAnchor,
    Uncertainty, ValidationOptions,
};
use crate::x509::{Certificate, CertificateList, Name};
use pagura_core::Limits;

/// Enumerates paths from a caller-selected target through only caller-supplied
/// certificates and anchors. No AIA fetching, system roots or self-sign trust.
/// Cycles use exact certificate encodings, permitting distinct self-issued
/// rollover certificates. A shared budget bounds the entire search, including
/// validation of every completed path. Exhaustion is explicitly incomplete.
pub fn build_paths(
    target: usize,
    certificates: &[&Certificate<'_>],
    anchors: &[TrustAnchor<'_>],
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    limits: &Limits,
) -> PathSearch {
    with_budget(
        target,
        certificates,
        anchors,
        options,
        verifier,
        &mut Budget::new(limits),
    )
}

pub(super) fn with_budget(
    target: usize,
    certificates: &[&Certificate<'_>],
    anchors: &[TrustAnchor<'_>],
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    budget: &mut Budget<'_>,
) -> PathSearch {
    with_evidence(
        target,
        certificates,
        anchors,
        None,
        options,
        verifier,
        budget,
    )
}

pub(super) fn with_evidence(
    target: usize,
    certificates: &[&Certificate<'_>],
    anchors: &[TrustAnchor<'_>],
    crls: Option<&[&CertificateList<'_>]>,
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    budget: &mut Budget<'_>,
) -> PathSearch {
    let mut search = Search {
        certificates,
        anchors,
        crls,
        options,
        names: Vec::new(),
        anchor_names: Vec::new(),
        result: PathSearch {
            paths: Vec::new(),
            complete: true,
            exhaustive: true,
            stopped: None,
        },
    };
    let run = search.run(target, verifier, budget);
    if let Err(issue) = run {
        let reason = match issue {
            Issue::Unknown(reason) => reason,
            Issue::Invalid(_) => Uncertainty::NameForm,
        };
        search.result.complete = false;
        search.result.exhaustive = false;
        search.result.stopped = Some(reason);
    }
    search.result
}

struct PoolNames {
    issuer: Option<Dn>,
    subject: Option<Dn>,
}
struct Search<'c, 'a, 'o, 'v> {
    certificates: &'c [&'c Certificate<'a>],
    anchors: &'c [TrustAnchor<'a>],
    options: &'o ValidationOptions<'v>,
    crls: Option<&'c [&'c CertificateList<'a>]>,
    names: Vec<PoolNames>,
    anchor_names: Vec<Option<Dn>>,
    result: PathSearch,
}

impl Search<'_, '_, '_, '_> {
    fn acceptable(&self, report: &super::PathReport) -> bool {
        if self.crls.is_some() {
            report.state == PathState::Valid
        } else {
            report.basic == PathState::Valid
        }
    }
    fn prepare_name(name: Name<'_>, budget: &mut Budget<'_>) -> Check<Option<Dn>> {
        match Dn::read(name, budget) {
            Ok(name) => Ok(Some(name)),
            Err(Issue::Unknown(Uncertainty::NameForm)) => Ok(None),
            Err(issue) => Err(issue),
        }
    }
    fn run(
        &mut self,
        target: usize,
        verifier: &mut impl SignatureVerifier,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        if target >= self.certificates.len() {
            budget.push(
                &mut self.result.paths,
                BuiltPath {
                    certificates: Vec::new(),
                    anchor: None,
                    report: report(
                        state(Issue::Invalid(InvalidReason::Input), None),
                        Vec::new(),
                        0,
                    ),
                },
            )?;
            return Ok(());
        }
        for &certificate in self.certificates {
            budget.node()?;
            let issuer = Self::prepare_name(certificate.issuer, budget)?;
            let subject = Self::prepare_name(certificate.subject, budget)?;
            budget.push(&mut self.names, PoolNames { issuer, subject })?;
        }
        for anchor in self.anchors {
            budget.node()?;
            let name = Self::prepare_name(anchor.name, budget)?;
            budget.push(&mut self.anchor_names, name)?;
        }
        budget.node()?;
        budget.bytes(8)?;
        let mut pending = Vec::new();
        budget.push(&mut pending, vec![target])?;
        while let Some(reverse) = pending.pop() {
            budget.depth(reverse.len())?;
            let current = *reverse.last().ok_or(Issue::Invalid(InvalidReason::Input))?;
            if self.names[current].issuer.is_none() {
                self.dead_end(&reverse, Issue::Unknown(Uncertainty::NameForm), budget)?;
                continue;
            }
            let mut unknown_name = false;
            let anchored = self.terminate(&reverse, verifier, &mut unknown_name, budget)?;
            if self.options.search == super::SearchMode::FirstAcceptable
                && self
                    .result
                    .paths
                    .last()
                    .is_some_and(|p| self.acceptable(&p.report))
            {
                self.result.exhaustive = false;
                return Ok(());
            }
            let extended = self.extend(&reverse, &mut pending, &mut unknown_name, budget)?;
            if !anchored && !extended {
                let issue = if unknown_name {
                    Issue::Unknown(Uncertainty::NameForm)
                } else {
                    Issue::Invalid(InvalidReason::Untrusted)
                };
                self.dead_end(&reverse, issue, budget)?;
            }
        }
        Ok(())
    }
    fn terminate(
        &mut self,
        reverse: &[usize],
        verifier: &mut impl SignatureVerifier,
        unknown_name: &mut bool,
        budget: &mut Budget<'_>,
    ) -> Check<bool> {
        let current = *reverse.last().ok_or(Issue::Invalid(InvalidReason::Input))?;
        let issuer = self.names[current]
            .issuer
            .as_ref()
            .ok_or(Issue::Unknown(Uncertainty::NameForm))?;
        let mut found = false;
        for (i, &anchor) in self.anchors.iter().enumerate() {
            let Some(name) = &self.anchor_names[i] else {
                *unknown_name = true;
                continue;
            };
            if !candidate_match(issuer.equal(name, budget), unknown_name)? {
                continue;
            }
            found = true;
            budget.node()?;
            budget.bytes(reverse.len().saturating_mul(16))?;
            let path = reverse
                .iter()
                .rev()
                .map(|&i| self.certificates[i])
                .collect::<Vec<_>>();
            let mut validation = validate(&path, anchor, self.options, verifier, budget);
            let stopped = self.crls.and_then(|crls| {
                super::revocation::engine::resolve(
                    &mut validation,
                    &path,
                    super::revocation::engine::Evidence {
                        certificates: self.certificates,
                        crls,
                    },
                    anchor,
                    self.options,
                    verifier,
                    budget,
                )
            });
            let exhausted = if let Some(Uncertainty::Limit(name)) = stopped {
                Some(name)
            } else if let PathState::Indeterminate {
                reason: Uncertainty::Limit(name),
                ..
            } = &validation.state
            {
                Some(*name)
            } else {
                None
            };
            budget.push(
                &mut self.result.paths,
                BuiltPath {
                    certificates: reverse.iter().rev().copied().collect(),
                    anchor: Some(i),
                    report: validation,
                },
            )?;
            if self.options.search == super::SearchMode::FirstAcceptable
                && self
                    .result
                    .paths
                    .last()
                    .is_some_and(|p| self.acceptable(&p.report))
            {
                break;
            }
            if let Some(name) = exhausted {
                return Err(Issue::Unknown(Uncertainty::Limit(name)));
            }
        }
        Ok(found)
    }
    fn extend(
        &self,
        reverse: &[usize],
        pending: &mut Vec<Vec<usize>>,
        unknown_name: &mut bool,
        budget: &mut Budget<'_>,
    ) -> Check<bool> {
        let current = *reverse.last().ok_or(Issue::Invalid(InvalidReason::Input))?;
        let issuer = self.names[current]
            .issuer
            .as_ref()
            .ok_or(Issue::Unknown(Uncertainty::NameForm))?;
        let mut found = false;
        for (i, &candidate) in self.certificates.iter().enumerate() {
            let Some(subject) = &self.names[i].subject else {
                *unknown_name = true;
                continue;
            };
            if !candidate_match(issuer.equal(subject, budget), unknown_name)? {
                continue;
            }
            let mut cycle = false;
            for &j in reverse {
                budget.work(candidate.element.encoded().len())?;
                cycle |= candidate.element.encoded() == self.certificates[j].element.encoded();
            }
            if cycle || self.is_anchor(i, budget)? {
                continue;
            }
            found = true;
            budget.depth(reverse.len().saturating_add(1))?;
            budget.node()?;
            budget.bytes(reverse.len().saturating_add(1).saturating_mul(8))?;
            let mut next = Vec::with_capacity(reverse.len() + 1);
            next.extend_from_slice(reverse);
            next.push(i);
            budget.push(pending, next)?;
        }
        Ok(found)
    }
    fn is_anchor(&self, candidate: usize, budget: &mut Budget<'_>) -> Check<bool> {
        let Some(subject) = &self.names[candidate].subject else {
            return Ok(false);
        };
        for (i, anchor) in self.anchors.iter().enumerate() {
            let Some(name) = &self.anchor_names[i] else {
                continue;
            };
            let key = self.certificates[candidate].public_key.element.encoded();
            budget.work(key.len())?;
            if key == anchor.public_key.element.encoded()
                && candidate_match(subject.equal(name, budget), &mut false)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn dead_end(&mut self, reverse: &[usize], issue: Issue, budget: &mut Budget<'_>) -> Check<()> {
        budget.node()?;
        budget.bytes(reverse.len().saturating_mul(8))?;
        let mut validation = report(state(issue, None), Vec::new(), reverse.len());
        if self.crls.is_some() {
            for _ in reverse {
                budget.push(
                    &mut validation.revocation,
                    super::revocation::RevocationOutcome::Unknown(
                        super::revocation::UnknownReason::SignerUntrusted,
                    ),
                )?;
            }
        }
        budget.push(
            &mut self.result.paths,
            BuiltPath {
                certificates: reverse.iter().rev().copied().collect(),
                anchor: None,
                report: validation,
            },
        )?;
        Ok(())
    }
}

// An unsupported comparison concerns this edge, not the rest of the pool.
fn candidate_match(result: Check<bool>, unknown: &mut bool) -> Check<bool> {
    match result {
        Err(Issue::Unknown(Uncertainty::NameForm)) => {
            *unknown = true;
            Ok(false)
        }
        other => other,
    }
}
