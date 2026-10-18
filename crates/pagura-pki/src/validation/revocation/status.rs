use super::engine::{Cached, Engine, issuer_matches};
use super::parse::{self, Info, document, malformed};
use super::{RevocationOutcome, RevocationReason, UnknownReason};
use crate::Time;
use crate::validation::budget::Budget;
use crate::validation::names::Dn;
use crate::validation::{Check, Issue, Uncertainty};
use crate::x509::{Certificate, CertificateList};

impl Engine<'_, '_> {
    pub fn status(
        &self,
        certificate: &Certificate<'_>,
        budget: &mut Budget<'_>,
    ) -> Check<RevocationOutcome> {
        match self.check(certificate, budget) {
            Ok(status) => Ok(status),
            Err(Issue::Unknown(Uncertainty::RevocationData(reason))) => {
                Ok(RevocationOutcome::Unknown(reason))
            }
            Err(Issue::Unknown(Uncertainty::NameForm)) => {
                Ok(RevocationOutcome::Unknown(UnknownReason::NameForm))
            }
            Err(issue) => Err(issue),
        }
    }
    fn check(
        &self,
        certificate: &Certificate<'_>,
        budget: &mut Budget<'_>,
    ) -> Check<RevocationOutcome> {
        let points = parse::certificate_points(certificate, budget)?;
        let issuer = Dn::read(certificate.issuer, budget)?;
        let mut reasons = 0;
        let mut failure = UnknownReason::NoData;
        for (i, cached) in self.cache.iter().enumerate() {
            budget.node()?;
            // Only authorised CRLs participate in scope comparisons.
            if !cached.active() {
                failure = prefer(failure, cached.failure);
                continue;
            }
            match self.check_crl(i, certificate, &points, &issuer, budget) {
                Ok((_, Some((reason, time)))) => {
                    return Ok(RevocationOutcome::Revoked { reason, time });
                }
                Ok((mask, None)) => reasons |= mask,
                Err(Issue::Unknown(Uncertainty::NameForm)) => {
                    failure = prefer(failure, UnknownReason::NameForm);
                }
                Err(Issue::Unknown(Uncertainty::RevocationData(reason))) => {
                    failure = prefer(failure, reason);
                }
                Err(issue) => return Err(issue),
            }
        }
        Ok(if reasons == parse::ALL {
            RevocationOutcome::Good
        } else if reasons != 0 {
            RevocationOutcome::Unknown(UnknownReason::IncompleteReasons)
        } else {
            RevocationOutcome::Unknown(failure)
        })
    }
    fn check_crl(
        &self,
        i: usize,
        certificate: &Certificate<'_>,
        points: &[parse::Point],
        issuer: &Dn,
        budget: &mut Budget<'_>,
    ) -> Check<(u16, Option<(RevocationReason, Time<'static>)>)> {
        let cached = &self.cache[i];
        let crl_issuer = Dn::read(self.crls[i].issuer, budget)?;
        let mut relevant = crl_issuer.equal(issuer, budget)?;
        for point in points {
            if let Some(issuers) = &point.issuers {
                relevant |= issuer_matches(issuers, &crl_issuer, budget)?;
            }
        }
        if !relevant {
            return Ok((0, None));
        }
        let info = cached.info.as_ref().ok_or_else(malformed)?;
        if info.delta.is_some() {
            return Err(parse::unknown(UnknownReason::MissingBase));
        }
        let mut mask = 0;
        for point in points {
            budget.node()?;
            mask |= parse::scope(certificate, point, info, budget)?;
        }
        if mask == 0 {
            return Err(parse::unknown(UnknownReason::OutOfScope));
        }
        let delta = match self.select_delta(i, budget)? {
            Ok(delta) => delta,
            Err(reason) => {
                return Err(parse::unknown(reason));
            }
        };
        if delta.is_none() && certificate.extension(parse::FRESHEST).is_some() {
            return Err(parse::unknown(UnknownReason::MissingDelta));
        }
        if self.superseded(i, delta, budget)? {
            return Ok((0, None));
        }

        let entry = combined_entry(
            self.crls[i],
            delta.map(|j| self.crls[j]),
            certificate,
            budget,
        )?;
        Ok((mask, entry))
    }
    fn select_delta(
        &self,
        base: usize,
        budget: &mut Budget<'_>,
    ) -> Check<Result<Option<usize>, UnknownReason>> {
        let crl = self.crls[base];
        let info = self.cache[base].info.as_ref().ok_or_else(malformed)?;
        if crl.this_update > self.time {
            return Ok(Err(UnknownReason::NotYetCurrent));
        }
        if crl.next_update.is_none_or(|next| next <= crl.this_update) {
            return Ok(Err(UnknownReason::Stale));
        }
        let mut selected: Option<usize> = None;
        for (i, cached) in self.cache.iter().enumerate() {
            budget.node()?;
            let Some(delta) = &cached.info else {
                continue;
            };
            if !cached.active()
                || delta.delta.is_none()
                || !compatible(info, delta, budget)?
                || !same_key(&self.cache[base], cached, budget)?
            {
                continue;
            }
            let crl = self.crls[i];
            if crl.this_update < self.crls[base].this_update || !current(crl, self.time) {
                continue;
            }
            let newer = if let Some(j) = selected {
                crl.this_update > self.crls[j].this_update
                    || (crl.this_update == self.crls[j].this_update
                        && number_greater(
                            delta.number.as_ref(),
                            self.cache[j].info.as_ref().and_then(|v| v.number.as_ref()),
                            budget,
                        )?)
            } else {
                true
            };
            if newer {
                selected = Some(i);
            }
        }
        if selected.is_some() {
            return Ok(Ok(selected));
        }
        if info.freshest {
            return Ok(Err(UnknownReason::MissingDelta));
        }
        if !current(crl, self.time) {
            return Ok(Err(UnknownReason::Stale));
        }
        Ok(Ok(None))
    }
    fn superseded(
        &self,
        base: usize,
        delta: Option<usize>,
        budget: &mut Budget<'_>,
    ) -> Check<bool> {
        let info = self.cache[base].info.as_ref().ok_or_else(malformed)?;
        let effective = self.crls[delta.unwrap_or(base)].this_update;
        for (i, cached) in self.cache.iter().enumerate() {
            budget.node()?;
            let Some(other) = &cached.info else {
                continue;
            };
            if i == base
                || other.delta.is_some()
                || !cached.active()
                || !same_scope(info, other, budget)?
                || !same_key(&self.cache[base], cached, budget)?
            {
                continue;
            }
            if let Ok(new_delta) = self.select_delta(i, budget)? {
                let newer = self.crls[new_delta.unwrap_or(i)].this_update;
                if newer > effective
                    || (newer == effective
                        && number_greater(other.number.as_ref(), info.number.as_ref(), budget)?)
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
// Report the most specific observed obstacle independently of input order.
fn prefer(a: UnknownReason, b: UnknownReason) -> UnknownReason {
    let rank = |reason| match reason {
        UnknownReason::NoData => 0,
        UnknownReason::OutOfScope => 1,
        UnknownReason::MissingBase => 2,
        UnknownReason::MissingDelta => 3,
        UnknownReason::NotYetCurrent => 4,
        UnknownReason::Stale => 5,
        UnknownReason::IncompleteReasons => 6,
        UnknownReason::SignerUntrusted => 7,
        UnknownReason::VerificationFailed => 8,
        UnknownReason::UnsupportedCriticalExtension => 9,
        UnknownReason::Malformed => 10,
        UnknownReason::NameForm => 11,
        UnknownReason::Limit(_) => 12,
    };
    if rank(a) >= rank(b) { a } else { b }
}
fn current(crl: &CertificateList<'_>, time: Time<'_>) -> bool {
    crl.this_update <= time
        && crl
            .next_update
            .is_some_and(|next| time < next && crl.this_update < next)
}
fn number_greater(
    a: Option<&Vec<u8>>,
    b: Option<&Vec<u8>>,
    budget: &mut Budget<'_>,
) -> Check<bool> {
    if let (Some(a), Some(b)) = (a, b) {
        budget.work(a.len().saturating_add(b.len()))?;
        Ok((a.len(), a) > (b.len(), b))
    } else {
        Ok(false)
    }
}
fn same_scope(a: &Info, b: &Info, budget: &mut Budget<'_>) -> Check<bool> {
    budget.work(a.idp.as_ref().map_or(0, Vec::len) + a.aki.as_ref().map_or(0, Vec::len))?;
    Ok(a.idp == b.idp
        && a.aki == b.aki
        && match a.issuer.equal(&b.issuer, budget) {
            Err(Issue::Unknown(Uncertainty::NameForm)) => false,
            other => other?,
        })
}
fn compatible(base: &Info, delta: &Info, budget: &mut Budget<'_>) -> Check<bool> {
    let (Some(base_number), Some(minimum), Some(delta_number)) =
        (&base.number, &delta.delta, &delta.number)
    else {
        return Ok(false);
    };
    budget.work(base_number.len() + minimum.len() + delta_number.len())?;
    Ok((base_number.len(), base_number) >= (minimum.len(), minimum)
        && (base_number.len(), base_number) < (delta_number.len(), delta_number)
        && same_scope(base, delta, budget)?)
}
fn same_key(a: &Cached, b: &Cached, budget: &mut Budget<'_>) -> Check<bool> {
    for x in a.authorizations.iter().filter(|a| a.active) {
        for y in b.authorizations.iter().filter(|a| a.active) {
            budget.work(
                x.key.key.len()
                    + x.key.algorithm.len()
                    + x.key.parameters.as_ref().map_or(0, Vec::len)
                    + 1,
            )?;
            if x.key == y.key {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn combined_entry(
    base: &CertificateList<'_>,
    delta: Option<&CertificateList<'_>>,
    certificate: &Certificate<'_>,
    budget: &mut Budget<'_>,
) -> Check<Option<(RevocationReason, Time<'static>)>> {
    let update = delta
        .map(|crl| entry(crl, certificate, budget))
        .transpose()?
        .flatten();
    if update.is_some_and(|(reason, _)| reason == RevocationReason::RemoveFromCrl) {
        if certificate.not_after >= delta.map_or(base.this_update, |crl| crl.this_update)
            && entry(base, certificate, budget)?
                .is_some_and(|(reason, _)| reason != RevocationReason::CertificateHold)
        {
            return Err(malformed());
        }
        return Ok(None);
    }
    if let Some(status) = update {
        Ok(Some(status))
    } else {
        entry(base, certificate, budget)
    }
}
fn entry(
    crl: &CertificateList<'_>,
    certificate: &Certificate<'_>,
    budget: &mut Budget<'_>,
) -> Check<Option<(RevocationReason, Time<'static>)>> {
    let issuer = Dn::read(certificate.issuer, budget)?;
    let mut current_issuers = Vec::new();
    let name = Dn::read(crl.issuer, budget)?;
    budget.push(&mut current_issuers, parse::DpName::Directory(name))?;
    let mut status = None;
    for entry in &crl.revoked {
        budget.node()?;
        let mut reason = RevocationReason::Unspecified;
        for &ext in &entry.extensions {
            match ext.id.as_bytes() {
                [85, 29, 29] => {
                    let doc = document(ext, budget)?;
                    current_issuers = parse::names(doc.root(), budget)?;
                }
                [85, 29, 21] => {
                    let doc = document(ext, budget)?;
                    reason = parse::reason(doc.root())?;
                }
                _ => {}
            }
        }
        budget.work(entry.serial.as_bytes().len())?;
        if entry.serial == certificate.serial && issuer_matches(&current_issuers, &issuer, budget)?
        {
            if status.is_some() {
                return Err(malformed());
            }
            // RFC 5280 6.3.3(i–j) uses the matching authenticated entry;
            // a future revocationDate does not turn its status into Good.
            status = Some((
                reason,
                Time {
                    fraction: "",
                    ..entry.date
                },
            ));
        }
    }
    Ok(status)
}
