use super::budget::Budget;
use super::names::{Constraints, Dn};
use super::{Check, InvalidReason, Issue, Oid};
use crate::der::{Document, Element, Fields, SEQUENCE, Tag};
use crate::x509::{Certificate, Extension, extensions as ext};
use crate::{Integer, ObjectIdentifier, types};
use std::collections::BTreeSet;

pub(super) struct Prepared {
    pub issuer: Dn,
    pub subject: Dn,
    pub constraints: Constraints,
    pub ca: bool,
    pub path_len: Option<usize>,
    pub usage: Option<u16>,
    pub eku: Option<Vec<Oid>>,
    pub policies: Option<Vec<Oid>>,
    pub mappings: Vec<(Oid, Oid)>,
    pub explicit: Option<usize>,
    pub mapping: Option<usize>,
    pub inhibit_any: Option<usize>,
    pub deferred_critical: bool,
}

impl Prepared {
    pub fn read(certificate: &Certificate<'_>, budget: &mut Budget<'_>) -> Check<Self> {
        budget.node()?;
        budget.bytes(512)?;
        let subject = Dn::read(certificate.subject, budget)?;
        let issuer = Dn::read(certificate.issuer, budget)?;
        let mut value = Self {
            issuer,
            subject,
            constraints: Constraints::default(),
            ca: false,
            path_len: None,
            usage: None,
            eku: None,
            policies: None,
            mappings: Vec::new(),
            explicit: None,
            mapping: None,
            inhibit_any: None,
            deferred_critical: false,
        };
        for &extension in &certificate.extensions {
            value.extension(extension, budget)?;
        }
        Ok(value)
    }
    fn extension(&mut self, extension: Extension<'_>, budget: &mut Budget<'_>) -> Check<()> {
        budget.work(extension.value.len())?;
        let oid = extension.id.as_bytes();
        // A failure report retains its reason in both basic and final states.
        budget.string(oid.len())?;
        budget.bytes(oid.len())?;
        let known = [
            ext::BASIC_CONSTRAINTS,
            ext::KEY_USAGE,
            ext::EXTENDED_KEY_USAGE,
            ext::SUBJECT_KEY_IDENTIFIER,
            ext::AUTHORITY_KEY_IDENTIFIER,
            ext::CERTIFICATE_POLICIES,
            ext::POLICY_MAPPINGS,
            ext::POLICY_CONSTRAINTS,
            ext::INHIBIT_ANY_POLICY,
            ext::NAME_CONSTRAINTS,
            ext::SUBJECT_ALT_NAME,
        ];
        if !known.contains(&oid) {
            if extension.critical {
                // CRL-dependent critical semantics are explicitly unresolved.
                if oid == ext::CRL_DISTRIBUTION_POINTS || oid == [85, 29, 46] {
                    self.deferred_critical = true;
                } else {
                    return Err(Issue::Invalid(InvalidReason::CriticalExtension(
                        oid.to_vec(),
                    )));
                }
            }
            return Ok(());
        }
        // The temporary inner DER index does not escape; interpreted values
        // below own their OIDs and names, with aggregate validation budgets.
        budget.bytes(extension.value.len().saturating_mul(64))?;
        let document = Document::parse(extension.value, &budget.der_limits()).map_err(|error| {
            if matches!(error.kind, pagura_core::ErrorKind::Limit) {
                Issue::Unknown(super::Uncertainty::Limit("pki.der"))
            } else {
                malformed(extension)
            }
        })?;
        budget.nodes(document.root().walk().count())?;
        self.decode(document.root(), extension, budget)
    }
    fn decode(
        &mut self,
        root: Element<'_>,
        extension: Extension<'_>,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        let oid = extension.id.as_bytes();
        if oid == ext::SUBJECT_KEY_IDENTIFIER {
            types::octets(root).map_err(|_| malformed(extension))?;
        } else if oid == ext::AUTHORITY_KEY_IDENTIFIER {
            ext::AuthorityKeyIdentifier::parse(root).map_err(|_| malformed(extension))?;
        } else if oid == ext::BASIC_CONSTRAINTS {
            let basic = ext::BasicConstraints::parse(root).map_err(|_| malformed(extension))?;
            self.ca = basic.ca;
            self.path_len = basic
                .path_len
                .map(|v| bounded_integer(v, budget.limits.depth));
        } else if oid == ext::KEY_USAGE {
            let usage = ext::KeyUsage::parse(root).map_err(|_| malformed(extension))?;
            let bits = (0..9)
                .filter(|&n| usage.permits(n))
                .fold(0, |bits, n| bits | (1 << n));
            if bits & ((1 << 7) | (1 << 8)) != 0 && bits & (1 << 4) == 0 {
                return Err(malformed(extension));
            }
            self.usage = Some(bits);
        } else if oid == ext::EXTENDED_KEY_USAGE {
            let oids = ext::object_identifiers(root).map_err(|_| malformed(extension))?;
            self.eku = Some(oids.into_iter().map(Oid::new).collect());
        } else if oid == ext::CERTIFICATE_POLICIES {
            self.policies = Some(read_policies(root, extension, budget)?);
        } else if oid == ext::POLICY_MAPPINGS {
            for (a, b) in ext::policy_mappings(root).map_err(|_| malformed(extension))? {
                budget.node()?;
                budget.bytes(64 + a.as_bytes().len() + b.as_bytes().len())?;
                let pair = (Oid::new(a), Oid::new(b));
                if pair.0 == Oid::any() || pair.1 == Oid::any() {
                    return Err(Issue::Invalid(InvalidReason::AnyPolicyMapping));
                }
                self.mappings.push(pair);
            }
        } else if oid == ext::POLICY_CONSTRAINTS {
            let constraints =
                ext::PolicyConstraints::parse(root).map_err(|_| malformed(extension))?;
            self.explicit = constraints
                .require_explicit_policy
                .map(|n| bounded_integer(n, budget.limits.depth));
            self.mapping = constraints
                .inhibit_policy_mapping
                .map(|n| bounded_integer(n, budget.limits.depth));
        } else if oid == ext::INHIBIT_ANY_POLICY {
            let n = ext::nonnegative(root).map_err(|_| malformed(extension))?;
            self.inhibit_any = Some(bounded_integer(n, budget.limits.depth));
        } else if oid == ext::NAME_CONSTRAINTS {
            if let Err(issue) = self.constraints.add(root, budget) {
                return Err(match issue {
                    Issue::Unknown(super::Uncertainty::NameForm) if extension.critical => {
                        Issue::Invalid(InvalidReason::CriticalExtension(oid.to_vec()))
                    }
                    Issue::Invalid(_) => malformed(extension),
                    other @ Issue::Unknown(_) => other,
                });
            }
        } else if oid == ext::SUBJECT_ALT_NAME {
            // GeneralName envelope syntax is checked now. Matching rules are
            // applied only if the accumulated constraints cover that form.
            ext::general_names(root).map_err(|_| malformed(extension))?;
        }
        Ok(())
    }
    pub fn self_issued(&self, budget: &mut Budget<'_>) -> Check<bool> {
        self.issuer.equal(&self.subject, budget)
    }
}

fn malformed(extension: Extension<'_>) -> Issue {
    Issue::Invalid(InvalidReason::Extension(extension.id.as_bytes().to_vec()))
}

// SkipCerts and pathLenConstraint have no fixed ASN.1 width. Values larger
// than the maximum possible path are equivalent; do not overflow or reject.
fn bounded_integer(integer: Integer<'_>, ceiling: usize) -> usize {
    integer.as_bytes().iter().fold(0usize, |n, &b| {
        n.saturating_mul(256)
            .saturating_add(usize::from(b))
            .min(ceiling.saturating_add(1))
    })
}

fn qualifiers_check(qualifiers: Element<'_>, extension: Extension<'_>, any: bool) -> Check<()> {
    let check = || -> pagura_core::Result<()> {
        for qualifier in qualifiers.children() {
            let mut fields = Fields::sequence(qualifier)?;
            let oid = ObjectIdentifier::parse(fields.take()?)?;
            let value = fields.take()?;
            fields.finish()?;
            if oid.is(&[43, 6, 1, 5, 5, 7, 2, 1]) {
                value.expect(Tag::universal(22, false))?; // CPS URI is inert.
            } else if oid.is(&[43, 6, 1, 5, 5, 7, 2, 2]) {
                let mut notice = Fields::sequence(value)?;
                if let Some(reference) = notice.optional(SEQUENCE) {
                    let mut reference = Fields::sequence(reference)?;
                    display_text(reference.take()?)?;
                    let numbers = reference.take()?.expect(SEQUENCE)?;
                    for n in numbers.children() {
                        Integer::parse(n)?;
                    }
                    reference.finish()?;
                }
                if let Some(text) = notice.rest() {
                    display_text(text)?;
                }
                notice.finish()?;
            } else if extension.critical || any {
                return Err(value.error("unprocessed policy qualifier"));
            }
        }
        Ok(())
    };
    check().map_err(|_| malformed(extension))
}
fn display_text(value: Element<'_>) -> pagura_core::Result<()> {
    if !matches!(value.tag().number, 12 | 22 | 26 | 30) {
        return Err(value.error("invalid policy DisplayText"));
    }
    types::StringValue::parse(value)?;
    let count = match value.tag().number {
        12 => std::str::from_utf8(value.content()).map_or(usize::MAX, |s| s.chars().count()),
        30 => value.content().len() / 2,
        _ => value.content().len(),
    };
    // RFC 5280 §4.2.1.4 asks readers to handle notices exceeding the
    // producer's 200-character bound. DER/string budgets already bound them.
    if count == 0 {
        return Err(value.error("empty policy DisplayText"));
    }
    Ok(())
}

fn read_policies(
    root: Element<'_>,
    extension: Extension<'_>,
    budget: &mut Budget<'_>,
) -> Check<Vec<Oid>> {
    let policies = ext::certificate_policies(root).map_err(|_| malformed(extension))?;
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    for policy in policies {
        budget.node()?;
        let bytes = policy.id.as_bytes();
        budget.bytes(512 + bytes.len())?;
        let comparisons =
            usize::try_from(seen.len().checked_ilog2().unwrap_or(0) + 2).unwrap_or(usize::MAX);
        budget.work(bytes.len().saturating_add(1).saturating_mul(comparisons))?;
        if !seen.insert(bytes) {
            return Err(malformed(extension));
        }
        let id = Oid::new(policy.id);
        if let Some(qualifiers) = policy.qualifiers {
            qualifiers_check(qualifiers, extension, id == Oid::any())?;
        }
        ids.push(id);
    }
    Ok(ids)
}
