//! Conservative RFC 5280 7 and RFC 4518 comparison; RFC 9549/9598 domain constraints.
use super::budget::Budget;
use super::{Check, InvalidReason, Issue, Uncertainty};
use crate::der::{self, Element, Fields, Form, Tag, single};
use crate::types::StringValue;
use crate::x509::{
    Name,
    extensions::{self, GeneralName},
};
use crate::{ObjectIdentifier, oids};

/// The id-at arc, 2.5.4, of X.520 attribute types.
const ID_AT: [u8; 2] = [85, 4];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Attribute {
    oid: Vec<u8>,
    value: Vec<u8>,
    unprepared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Dn(Vec<Vec<Attribute>>);

impl Dn {
    pub(super) fn read(name: Name<'_>, budget: &mut Budget<'_>) -> Check<Self> {
        Self::read_rdns(name.element().children(), budget)
    }
    pub(super) fn relative(
        base: Name<'_>,
        rdn: Element<'_>,
        budget: &mut Budget<'_>,
    ) -> Check<Self> {
        let mut name = Self::read(base, budget)?;
        let tail = Self::read_rdns(std::iter::once(rdn), budget)?;
        for rdn in tail.0 {
            budget.push(&mut name.0, rdn)?;
        }
        Ok(name)
    }
    fn read_rdns<'a>(
        rdns: impl Iterator<Item = Element<'a>>,
        budget: &mut Budget<'_>,
    ) -> Check<Self> {
        let mut result = Vec::new();
        for rdn in rdns {
            budget.node()?;
            budget.bytes(32)?;
            let mut attributes = Vec::new();
            for attribute in rdn.children() {
                budget.node()?;
                let mut fields = Fields::sequence(attribute).map_err(|_| unknown())?;
                let id = ObjectIdentifier::parse(fields.take().map_err(|_| unknown())?)
                    .map_err(|_| unknown())?;
                let oid = id.as_bytes();
                let value = fields.take().map_err(|_| unknown())?;
                fields.finish().map_err(|_| unknown())?;
                // The standard X.520 caseIgnore family (id-at arcs 2.5.4.n), UID
                // and domainComponent. Unknown matching rules cannot be safely
                // guessed from ASN.1 tags.
                let x520 = oid.strip_prefix(&ID_AT).is_some_and(|arc| {
                    matches!(arc, [3..=13 | 15..=19 | 41..=46 | 51 | 54 | 65 | 97])
                });
                let dc = id == oids::DOMAIN_COMPONENT;
                let uid = id == oids::USER_ID;
                let email = id == oids::EMAIL_ADDRESS;
                budget.string(value.encoded().len())?;
                budget.bytes(oid.len() + 64)?;
                let prepared = if x520 || dc || uid || email {
                    ascii(value, budget)?
                } else {
                    None
                };
                let unprepared = prepared.is_none();
                let prepared = prepared.unwrap_or_else(|| value.encoded().to_vec());
                budget.push(
                    &mut attributes,
                    Attribute {
                        oid: oid.to_vec(),
                        value: prepared,
                        unprepared,
                    },
                )?;
            }
            budget.push(&mut result, attributes)?;
        }
        Ok(Self(result))
    }
    pub(super) fn owned_bytes(&self) -> usize {
        self.0
            .iter()
            .map(|rdn| {
                64 + rdn
                    .iter()
                    .map(|a| 128 + a.oid.len() + a.value.len())
                    .sum::<usize>()
            })
            .sum()
    }
    pub(super) fn equal(&self, other: &Self, budget: &mut Budget<'_>) -> Check<bool> {
        budget.work(
            self.0
                .iter()
                .flatten()
                .map(|a| a.oid.len() + a.value.len() + 1)
                .sum(),
        )?;
        rdns_equal(&self.0, &other.0, budget)
    }
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn subtree(&self, constraint: &Self, budget: &mut Budget<'_>) -> Check<bool> {
        if self.0.len() < constraint.0.len() {
            return Ok(false);
        }
        rdns_equal(&self.0[..constraint.0.len()], &constraint.0, budget)
    }
    pub(super) fn emails(&self, budget: &mut Budget<'_>) -> Check<Vec<General>> {
        let mut values = Vec::new();
        for attribute in self.0.iter().flatten() {
            if attribute.oid == oids::EMAIL_ADDRESS.as_bytes() {
                budget.node()?;
                budget.bytes(96 + attribute.value.len())?;
                budget.work(attribute.value.len())?;
                if attribute.unprepared {
                    return Err(unknown());
                }
                let bytes = &attribute.value;
                values.push(General::Mail(mailbox(bytes, false, budget)?));
            }
        }
        Ok(values)
    }
}

fn unknown() -> Issue {
    Issue::Unknown(Uncertainty::NameForm)
}
fn bad() -> Issue {
    Issue::Invalid(InvalidReason::NameConstraint)
}

// First-stage RFC 4518 comparison: exact values, or the supported ASCII
// caseIgnore subset. Non-identical values outside that subset stay unknown.
fn ascii_space(byte: u8) -> bool {
    byte == b' ' || (b'\t'..=b'\r').contains(&byte)
}
fn ascii(value: Element<'_>, budget: &mut Budget<'_>) -> Check<Option<Vec<u8>>> {
    if StringValue::parse(value).is_err() {
        return Ok(None);
    }
    let bytes = value.content();
    if !matches!(value.tag().number, 12 | 19 | 18 | 22 | 26 | 20)
        || !bytes.is_ascii()
        || bytes.iter().any(|&b| b < 32 && !ascii_space(b))
        || bytes.contains(&127)
    {
        return Ok(None);
    }
    budget.string(bytes.len())?;
    let mut result = Vec::new();
    let mut pending = false;
    for &byte in bytes {
        if ascii_space(byte) {
            pending = !result.is_empty();
        } else {
            if pending {
                result.push(b' ');
                pending = false;
            }
            result.push(byte.to_ascii_lowercase());
        }
    }
    Ok(Some(result))
}
fn rdns_equal(a: &[Vec<Attribute>], b: &[Vec<Attribute>], budget: &mut Budget<'_>) -> Check<bool> {
    if a.len() != b.len() {
        return Ok(false);
    }
    let mut unresolved = false;
    for (a, b) in a.iter().zip(b) {
        if a.len() != b.len() {
            return Ok(false);
        }
        budget.work(a.len().saturating_mul(b.len()))?;
        for attribute in a {
            let mut exact_a = 0;
            let mut exact_b = 0;
            let mut uncertain = false;
            for other in a {
                budget.work(
                    attribute
                        .value
                        .len()
                        .saturating_add(attribute.oid.len())
                        .saturating_add(1),
                )?;
                if other == attribute {
                    exact_a += 1;
                }
            }
            for other in b {
                budget.work(attribute.value.len().saturating_add(attribute.oid.len()))?;
                if other == attribute {
                    exact_b += 1;
                } else if other.oid == attribute.oid && (other.unprepared || attribute.unprepared) {
                    uncertain = true;
                }
            }
            if exact_a != exact_b {
                if !uncertain {
                    return Ok(false);
                }
                unresolved = true;
            }
        }
    }
    if unresolved { Err(unknown()) } else { Ok(true) }
}

#[derive(Debug, Clone)]
pub(super) enum General {
    Directory(Dn),
    Dns(String),
    Mail(String), // Only the domain is relevant to updated mail constraints.
    Uri(Option<String>),
    Ip(Vec<u8>),
    Other(u32),
}
impl General {
    fn owned_bytes(&self) -> usize {
        96 + match self {
            Self::Directory(name) => name.owned_bytes(),
            Self::Dns(name) | Self::Mail(name) => name.len(),
            Self::Uri(host) => host.as_ref().map_or(0, String::len),
            Self::Ip(bytes) => bytes.len(),
            Self::Other(_) => 0,
        }
    }
    fn kind(&self) -> u32 {
        match self {
            Self::Directory(_) => 4,
            Self::Dns(_) => 2,
            Self::Mail(_) => 1,
            Self::Uri(_) => 6,
            Self::Ip(_) => 7,
            Self::Other(n) => *n,
        }
    }
    pub(super) fn read(
        value: GeneralName<'_>,
        constraint: bool,
        budget: &mut Budget<'_>,
    ) -> Check<Self> {
        Ok(match value {
            GeneralName::Directory(name) => Self::Directory(Dn::read(name, budget)?),
            GeneralName::Dns(bytes) => Self::Dns(domain(bytes, constraint, budget)?),
            GeneralName::Mailbox(bytes) => {
                if constraint {
                    // RFC 9549 2.2 removes individual-mailbox constraints.
                    Self::Mail(domain(bytes, true, budget)?)
                } else {
                    Self::Mail(mailbox(bytes, false, budget)?)
                }
            }
            GeneralName::Uri(bytes) => {
                if constraint {
                    Self::Uri(Some(domain(bytes, true, budget)?))
                } else {
                    Self::Uri(uri_host(bytes, budget)?)
                }
            }
            GeneralName::Ip(bytes) => {
                if !(if constraint {
                    matches!(bytes.len(), 8 | 32)
                } else {
                    matches!(bytes.len(), 4 | 16)
                }) {
                    return Err(bad());
                }
                if constraint && !prefix_mask(&bytes[bytes.len() / 2..]) {
                    return Err(bad());
                }
                budget.string(bytes.len())?;
                Self::Ip(bytes.to_vec())
            }
            GeneralName::RegisteredId(_) => Self::Other(8),
            GeneralName::Other(element) if element.tag() == Tag::context(0, Form::Constructed) => {
                let mut fields = Fields::new(element);
                let oid = ObjectIdentifier::parse(fields.take().map_err(|_| bad())?)
                    .map_err(|_| bad())?;
                let value = fields
                    .take()
                    .map_err(|_| bad())?
                    .expect(Tag::context(0, Form::Constructed))
                    .map_err(|_| bad())?;
                fields.finish().map_err(|_| bad())?;
                if oid == extensions::SMTP_UTF8_MAILBOX {
                    if constraint {
                        return Err(bad());
                    }
                    let value = single(value)
                        .map_err(|_| bad())?
                        .expect(der::UTF8_STRING)
                        .map_err(|_| bad())?;
                    Self::Mail(mailbox(value.content(), true, budget)?)
                } else {
                    Self::Other(0)
                }
            }
            GeneralName::Other(element) => Self::Other(element.tag().number),
        })
    }
}

fn prefix_mask(mask: &[u8]) -> bool {
    let mut zero_seen = false;
    for &byte in mask {
        for bit in (0..8).rev() {
            if byte & (1 << bit) == 0 {
                zero_seen = true;
            } else if zero_seen {
                return false;
            }
        }
    }
    true
}

fn domain(bytes: &[u8], constraint: bool, budget: &mut Budget<'_>) -> Check<String> {
    budget.string(bytes.len())?;
    let mut value = std::str::from_utf8(bytes).map_err(|_| bad())?;
    if !value.is_ascii() {
        return Err(bad());
    } // A-labels only, no UTS46 conversion.
    if constraint && value.starts_with('.') {
        value = &value[1..];
    }
    if value.is_empty()
        || value.len() > 253
        || value.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(bad());
    }
    Ok(std::str::from_utf8(bytes)
        .map_err(|_| bad())?
        .to_ascii_lowercase())
}
fn mailbox(bytes: &[u8], smtp: bool, budget: &mut Budget<'_>) -> Check<String> {
    let value = std::str::from_utf8(bytes).map_err(|_| bad())?;
    let (local, host) = value.rsplit_once('@').ok_or_else(bad)?;
    if local.is_empty() || local.is_ascii() == smtp || !local_part(local) {
        return Err(bad());
    }
    budget.work(local.len())?;
    domain(host.as_bytes(), false, budget)
}
fn local_part(local: &str) -> bool {
    if let Some(quoted) = local.strip_prefix('"') {
        let Some(quoted) = quoted.strip_suffix('"') else {
            return false;
        };
        let mut escaped = false;
        for c in quoted.chars() {
            if c.is_control() {
                return false;
            }
            if escaped {
                if !c.is_ascii() {
                    return false;
                }
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                return false;
            }
        }
        return !escaped;
    }
    local.split('.').all(|atom| {
        !atom.is_empty()
            && atom.chars().all(|c| {
                !c.is_control()
                    && (!c.is_ascii()
                        || c.is_ascii_alphanumeric()
                        || "!#$%&'*+-/=?^_`{|}~".contains(c))
            })
    })
}

fn uri_host(bytes: &[u8], budget: &mut Budget<'_>) -> Check<Option<String>> {
    budget.string(bytes.len())?;
    let value = std::str::from_utf8(bytes).map_err(|_| bad())?;
    let (scheme, rest) = value.split_once(':').ok_or_else(bad)?;
    if scheme.is_empty()
        || !scheme.as_bytes()[0].is_ascii_alphabetic()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return Err(bad());
    }
    let Some(authority) = rest.strip_prefix("//") else {
        return Ok(None);
    };
    let authority = authority.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    if host_port.starts_with('[') {
        return Ok(None);
    }
    let host = host_port.split(':').next().unwrap_or("");
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return Ok(None);
    }
    Ok(Some(domain(host.as_bytes(), false, budget)?))
}

#[derive(Debug, Clone, Default)]
pub(super) struct Constraints {
    denied: u16,
    forms: u16,
    // Each permitted group is a union. Groups from successive issuers intersect.
    permitted: Vec<Vec<General>>,
    excluded: Vec<General>,
}
impl Constraints {
    pub(super) fn is_empty(&self) -> bool {
        self.denied == 0 && self.permitted.is_empty() && self.excluded.is_empty()
    }
    pub(super) fn deny(&mut self, forms: &[super::NameForm], budget: &mut Budget<'_>) -> Check<()> {
        budget.work(forms.len())?;
        for form in forms {
            self.denied |= 1 << form.number();
        }
        Ok(())
    }
    pub(super) fn add(&mut self, value: Element<'_>, budget: &mut Budget<'_>) -> Check<()> {
        budget.bytes(value.encoded().len().saturating_mul(64))?;
        budget.nodes(value.walk().count())?;
        budget.work(value.encoded().len())?;
        let constraints = extensions::NameConstraints::parse(value).map_err(|_| bad())?;
        let mut permitted = Vec::new();
        for (subtrees, excluded) in [(constraints.permitted, false), (constraints.excluded, true)] {
            for subtree in subtrees {
                budget.node()?;
                budget.bytes(96)?;
                if subtree.minimum.is_some() || subtree.maximum.is_some() {
                    return Err(bad());
                }
                let name = General::read(subtree.base, true, budget)?;
                if matches!(name, General::Other(_)) {
                    return Err(unknown());
                }
                self.forms |= 1 << name.kind();
                if excluded {
                    self.excluded.push(name);
                } else {
                    permitted.push(name);
                }
            }
        }
        if !permitted.is_empty() {
            self.permitted.push(permitted);
        }
        Ok(())
    }
    pub(super) fn append(&mut self, other: &Self, budget: &mut Budget<'_>) -> Check<()> {
        let bytes = other
            .permitted
            .iter()
            .flatten()
            .chain(&other.excluded)
            .map(General::owned_bytes)
            .sum();
        budget.bytes(bytes)?;
        self.denied |= other.denied;
        self.forms |= other.forms;
        self.permitted.extend(other.permitted.iter().cloned());
        self.excluded.extend(other.excluded.iter().cloned());
        Ok(())
    }
    fn covers(&self, kind: u32) -> bool {
        (self.denied | self.forms) & (1 << kind) != 0
    }
    pub(super) fn check_certificate(
        &self,
        certificate: &crate::x509::Certificate<'_>,
        budget: &mut Budget<'_>,
    ) -> Check<()> {
        if self.is_empty() {
            return Ok(());
        }
        let mut names = Vec::new();
        if self.covers(4) {
            let name = Dn::read(certificate.subject, budget)?;
            if !name.is_empty() {
                budget.push(&mut names, General::Directory(name))?;
            }
        }
        if let Some(extension) = certificate.extension(extensions::SUBJECT_ALT_NAME) {
            budget.bytes(extension.value().len().saturating_mul(64))?;
            let doc =
                der::Document::parse(extension.value(), &budget.der_limits()).map_err(|error| {
                    if matches!(error.kind, pagura_core::ErrorKind::Limit) {
                        Issue::Unknown(Uncertainty::Limit("pki.der"))
                    } else {
                        bad()
                    }
                })?;
            budget.nodes(doc.root().walk().count())?;
            for name in extensions::general_names(doc.root()).map_err(|_| bad())? {
                budget.node()?;
                let kind = match name {
                    GeneralName::Directory(_) => 4,
                    GeneralName::Dns(_) => 2,
                    GeneralName::Mailbox(_) => 1,
                    GeneralName::Uri(_) => 6,
                    GeneralName::Ip(_) => 7,
                    GeneralName::RegisteredId(_) => 8,
                    GeneralName::Other(value) => value.tag().number,
                };
                // SmtpUTF8Mailbox is an otherName but shares mailbox constraints.
                if !(self.covers(kind) || kind == 0 && self.covers(1)) {
                    continue;
                }
                let name = General::read(name, false, budget)?;
                if self.covers(name.kind()) {
                    budget.push(&mut names, name)?;
                }
            }
        } else if self.covers(1) {
            for name in Dn::read(certificate.subject, budget)?.emails(budget)? {
                budget.push(&mut names, name)?;
            }
        }
        self.check(&names, budget)
    }
    pub(super) fn check(&self, names: &[General], budget: &mut Budget<'_>) -> Check<()> {
        for name in names {
            budget.work(1)?;
            if self.denied & (1 << name.kind()) != 0 {
                return Err(bad());
            }
            for constraint in &self.excluded {
                budget.work(1)?;
                if name.kind() == constraint.kind() && within(name, constraint, budget)? {
                    return Err(bad());
                }
            }
            for group in &self.permitted {
                let mut constrained = false;
                let mut matched = false;
                for constraint in group {
                    budget.work(1)?;
                    if name.kind() == constraint.kind() {
                        constrained = true;
                        matched |= within(name, constraint, budget)?;
                    }
                }
                if constrained && !matched {
                    return Err(bad());
                }
            }
        }
        Ok(())
    }
}
fn within(name: &General, constraint: &General, budget: &mut Budget<'_>) -> Check<bool> {
    budget.work(name.owned_bytes().saturating_add(constraint.owned_bytes()))?;
    Ok(match (name, constraint) {
        (General::Directory(name), General::Directory(base)) => {
            budget.work(name.0.iter().map(Vec::len).sum())?;
            name.subtree(base, budget)?
        }
        (General::Dns(name), General::Dns(base)) => suffix(name, base, true),
        (General::Mail(name), General::Mail(base))
        | (General::Uri(Some(name)), General::Uri(Some(base))) => suffix(name, base, false),
        (General::Uri(None), General::Uri(_)) => return Err(bad()),
        (General::Ip(name), General::Ip(base)) => {
            let half = base.len() / 2;
            name.len() == half
                && name
                    .iter()
                    .zip(&base[..half])
                    .zip(&base[half..])
                    .all(|((&n, &b), &mask)| n & mask == b & mask)
        }
        _ => return Err(unknown()),
    })
}
fn suffix(name: &str, base: &str, dns: bool) -> bool {
    if base.starts_with('.') {
        name.len() > base.len() && name.ends_with(base)
    } else {
        name == base
            || (dns
                && name
                    .strip_suffix(base)
                    .is_some_and(|prefix| prefix.ends_with('.')))
    }
}
