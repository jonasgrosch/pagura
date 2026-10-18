//! CRL and distribution-point semantics (RFC 5280 §§4.2.1.13, 5.2–5.3).
use super::{RevocationReason, UnknownReason};
use crate::der::{Document, Element, Fields, SEQUENCE, Tag, single};
use crate::validation::budget::Budget;
use crate::validation::names::Dn;
use crate::validation::{Check, Issue, Uncertainty};
use crate::x509::extensions::{self, BasicConstraints, GeneralName};
use crate::x509::{Certificate, CertificateList, Extension, Name};
use crate::{BitString, Integer, Time};

pub(super) const ALL: u16 = 0x1fe;
pub(super) const AKI: &[u8] = extensions::AUTHORITY_KEY_IDENTIFIER;
pub(super) const FRESHEST: &[u8] = &[85, 29, 46];

pub(super) fn unknown(reason: UnknownReason) -> Issue {
    Issue::Unknown(Uncertainty::RevocationData(reason))
}
pub(super) fn malformed() -> Issue {
    unknown(UnknownReason::Malformed)
}
pub(super) fn document<'a>(
    extension: Extension<'a>,
    budget: &mut Budget<'_>,
) -> Check<Document<'a>> {
    budget.work(extension.value.len())?;
    budget.bytes(extension.value.len().saturating_mul(64))?;
    let doc = Document::parse(extension.value, &budget.der_limits()).map_err(|error| {
        if matches!(error.kind, pagura_core::ErrorKind::Limit) {
            Issue::Unknown(Uncertainty::Limit("pki.der"))
        } else {
            malformed()
        }
    })?;
    budget.nodes(doc.root().walk().count())?;
    Ok(doc)
}

#[derive(Debug)]
pub(super) enum DpName {
    Directory(Dn),
    Other(Vec<u8>),
}
impl DpName {
    pub fn equal(&self, other: &Self, budget: &mut Budget<'_>) -> Check<bool> {
        match (self, other) {
            (Self::Directory(a), Self::Directory(b)) => a.equal(b, budget),
            (Self::Other(a), Self::Other(b)) => {
                budget.work(a.len())?;
                Ok(a == b)
            }
            _ => Ok(false),
        }
    }
}
pub(super) fn names(value: Element<'_>, budget: &mut Budget<'_>) -> Check<Vec<DpName>> {
    let mut result = Vec::new();
    for name in value.children() {
        budget.node()?;
        let parsed = GeneralName::parse(name).map_err(|_| malformed())?;
        let name = if let GeneralName::Directory(dn) = parsed {
            DpName::Directory(Dn::read(dn, budget)?)
        } else {
            // Distribution point URIs identify a complete location, including
            // its path. Name-constraint URI host matching is inappropriate here.
            budget.string(name.encoded().len())?;
            DpName::Other(name.encoded().to_vec())
        };
        budget.push(&mut result, name)?;
    }
    if result.is_empty() {
        return Err(malformed());
    }
    Ok(result)
}
pub(super) fn intersects(a: &[DpName], b: &[DpName], budget: &mut Budget<'_>) -> Check<bool> {
    for x in a {
        for y in b {
            if x.equal(y, budget)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn point(value: Element<'_>, base: Name<'_>, budget: &mut Budget<'_>) -> Check<Vec<DpName>> {
    let choice = single(value).map_err(|_| malformed())?;
    if choice.tag() == Tag::context(0, true) {
        return names(choice, budget);
    }
    if choice.tag() != Tag::context(1, true) || choice.children().next().is_none() {
        return Err(malformed());
    }
    let mut previous = None;
    for attribute in choice.children() {
        budget.work(attribute.encoded().len())?;
        if previous.is_some_and(|p| p > attribute.encoded()) {
            return Err(malformed());
        }
        previous = Some(attribute.encoded());
    }
    budget.bytes(size_of::<DpName>())?;
    Ok(vec![DpName::Directory(Dn::relative(base, choice, budget)?)])
}
fn reasons(value: Element<'_>) -> Check<u16> {
    let bits =
        BitString::from_bytes(value.content(), value.content_offset()).map_err(|_| malformed())?;
    if bits
        .as_bytes()
        .last()
        .is_none_or(|&last| last == 0 || last.trailing_zeros() != u32::from(bits.unused_bits()))
    {
        return Err(malformed());
    }
    let mut mask = 0;
    for bit in 0..bits.as_bytes().len().saturating_mul(8) {
        if bits.bit(bit) {
            if bit == 0 {
                continue;
            } // The named unused bit contributes no reason coverage.
            if bit > 8 {
                return Err(malformed());
            }
            mask |= 1 << bit;
        }
    }
    if mask == 0 {
        return Err(malformed());
    }
    Ok(mask)
}
fn boolean(value: Option<Element<'_>>) -> Check<bool> {
    match value.map(Element::content) {
        None => Ok(false),
        Some([255]) => Ok(true),
        _ => Err(malformed()), // DEFAULT FALSE is omitted in DER.
    }
}

pub(super) struct Point {
    pub names: Option<Vec<DpName>>,
    pub reasons: u16,
    pub issuers: Option<Vec<DpName>>,
}
pub(super) fn points(
    extension: Extension<'_>,
    base: Name<'_>,
    budget: &mut Budget<'_>,
) -> Check<Vec<Point>> {
    let doc = document(extension, budget)?;
    doc.root().expect(SEQUENCE).map_err(|_| malformed())?;
    let mut result = Vec::new();
    for value in doc.root().children() {
        let mut fields = Fields::sequence(value).map_err(|_| malformed())?;
        let name = fields.optional(Tag::context(0, true));
        let reasons = fields
            .optional(Tag::context(1, false))
            .map(reasons)
            .transpose()?
            .unwrap_or(ALL);
        let issuers = fields.optional(Tag::context(2, true));
        fields.finish().map_err(|_| malformed())?;
        if name.is_none() && issuers.is_none() {
            return Err(malformed());
        }
        let issuer_names = issuers.map(|i| names(i, budget)).transpose()?;
        if issuer_names
            .as_ref()
            .is_some_and(|names| !names.iter().any(|n| matches!(n, DpName::Directory(_))))
        {
            return Err(malformed());
        }
        let relative_base = if let Some(issuers) = issuers {
            if name.is_some_and(|n| single(n).is_ok_and(|c| c.tag() == Tag::context(1, true))) {
                if issuer_names.as_ref().is_some_and(|names| names.len() != 1) {
                    return Err(malformed());
                }
                let mut directory = issuers
                    .children()
                    .filter(|n| n.tag() == Tag::context(4, true));
                let name = directory.next().ok_or_else(malformed)?;
                if directory.next().is_some() {
                    return Err(malformed());
                }
                Name::parse(single(name).map_err(|_| malformed())?).map_err(|_| malformed())?
            } else {
                base
            }
        } else {
            base
        };
        let names = name.map(|n| point(n, relative_base, budget)).transpose()?;
        budget.node()?;
        budget.push(
            &mut result,
            Point {
                names,
                reasons,
                issuers: issuer_names,
            },
        )?;
    }
    if result.is_empty() {
        return Err(malformed());
    }
    Ok(result)
}

#[derive(Default, PartialEq, Eq)]
pub(super) enum CertificateClass {
    #[default]
    All,
    User,
    Ca,
    Attribute,
}
#[derive(Default)]
pub(super) struct Scope {
    pub names: Option<Vec<DpName>>,
    pub reasons: u16,
    pub class: CertificateClass,
    pub indirect: bool,
}
impl Scope {
    fn read(value: Element<'_>, base: Name<'_>, budget: &mut Budget<'_>) -> Check<Self> {
        let mut fields = Fields::sequence(value).map_err(|_| malformed())?;
        let names = fields
            .optional(Tag::context(0, true))
            .map(|v| point(v, base, budget))
            .transpose()?;
        let user = boolean(fields.optional(Tag::context(1, false)))?;
        let ca = boolean(fields.optional(Tag::context(2, false)))?;
        let reason_field = fields.optional(Tag::context(3, false));
        let reasons = reason_field.map(reasons).transpose()?.unwrap_or(ALL);
        let indirect = boolean(fields.optional(Tag::context(4, false)))?;
        let attribute = boolean(fields.optional(Tag::context(5, false)))?;
        fields.finish().map_err(|_| malformed())?;
        if u8::from(user) + u8::from(ca) + u8::from(attribute) > 1
            || (names.is_none()
                && reason_field.is_none()
                && !user
                && !ca
                && !attribute
                && !indirect)
        {
            return Err(malformed());
        }
        Ok(Self {
            names,
            reasons,
            class: if user {
                CertificateClass::User
            } else if ca {
                CertificateClass::Ca
            } else if attribute {
                CertificateClass::Attribute
            } else {
                CertificateClass::All
            },
            indirect,
        })
    }
}
pub(super) struct Info {
    pub issuer: Dn,
    pub scope: Scope,
    pub number: Option<Vec<u8>>,
    pub delta: Option<Vec<u8>>,
    pub idp: Option<Vec<u8>>,
    pub aki: Option<Vec<u8>>,
    pub freshest: bool,
}
impl Info {
    pub fn read(crl: &CertificateList<'_>, budget: &mut Budget<'_>) -> Check<Self> {
        let mut info = Self {
            issuer: Dn::read(crl.issuer, budget)?,
            scope: Scope {
                reasons: ALL,
                ..Scope::default()
            },
            number: None,
            delta: None,
            idp: None,
            aki: None,
            freshest: false,
        };
        for &ext in &crl.extensions {
            budget.node()?;
            let oid = ext.id.as_bytes();
            match oid {
                [85, 29, 20 | 27] => {
                    if (oid[2] == 27) != ext.critical {
                        return Err(malformed());
                    }
                    let doc = document(ext, budget)?;
                    let n = Integer::parse(doc.root())
                        .map_err(|_| malformed())?
                        .unsigned()
                        .map_err(|_| malformed())?;
                    budget.string(n.len())?;
                    if oid[2] == 27 {
                        info.delta = Some(n.to_vec());
                    } else {
                        info.number = Some(n.to_vec());
                    }
                }
                [85, 29, 28] => {
                    if !ext.critical {
                        return Err(malformed());
                    }
                    let doc = document(ext, budget)?;
                    info.scope = Scope::read(doc.root(), crl.issuer, budget)?;
                    info.idp = Some(ext.value.to_vec());
                }
                [85, 29, 35] => {
                    if ext.critical {
                        return Err(malformed());
                    }
                    let doc = document(ext, budget)?;
                    extensions::AuthorityKeyIdentifier::parse(doc.root())
                        .map_err(|_| malformed())?;
                    info.aki = Some(ext.value.to_vec());
                }
                [85, 29, 46] => {
                    if ext.critical {
                        return Err(malformed());
                    }
                    freshest(ext, crl.issuer, budget)?;
                    info.freshest = true;
                }
                _ if ext.critical => {
                    return Err(unknown(UnknownReason::UnsupportedCriticalExtension));
                }
                _ => {}
            }
        }
        if info.issuer.is_empty()
            || (info.delta.is_some() && (info.number.is_none() || info.freshest))
        {
            return Err(malformed());
        }
        Ok(info)
    }
}

fn freshest(extension: Extension<'_>, base: Name<'_>, budget: &mut Budget<'_>) -> Check<()> {
    let doc = document(extension, budget)?;
    doc.root().expect(SEQUENCE).map_err(|_| malformed())?;
    if doc.root().children().next().is_none() {
        return Err(malformed());
    }
    for value in doc.root().children() {
        budget.node()?;
        let mut fields = Fields::sequence(value).map_err(|_| malformed())?;
        point(
            fields
                .take()
                .map_err(|_| malformed())?
                .expect(Tag::context(0, true))
                .map_err(|_| malformed())?,
            base,
            budget,
        )?;
        fields.finish().map_err(|_| malformed())?;
    }
    Ok(())
}

pub(super) fn certificate_points(
    certificate: &Certificate<'_>,
    budget: &mut Budget<'_>,
) -> Check<Vec<Point>> {
    let mut result = if let Some(ext) = certificate.extension(extensions::CRL_DISTRIBUTION_POINTS) {
        points(ext, certificate.issuer, budget)?
    } else {
        Vec::new()
    };
    // RFC 5280 §6.3.3 final fallback, independent of advertised locations.
    let mut fallback = Vec::new();
    let issuer = Dn::read(certificate.issuer, budget)?;
    budget.push(&mut fallback, DpName::Directory(issuer))?;
    if let Some(ext) = certificate.extension(&[85, 29, 18]) {
        let doc = document(ext, budget)?;
        for name in names(doc.root(), budget)? {
            budget.push(&mut fallback, name)?;
        }
    }
    if let Some(ext) = certificate.extension(FRESHEST) {
        if ext.critical {
            return Err(malformed());
        }
        points(ext, certificate.issuer, budget)?;
    }
    budget.push(
        &mut result,
        Point {
            names: Some(fallback),
            reasons: ALL,
            issuers: None,
        },
    )?;
    Ok(result)
}
pub(super) fn scope(
    certificate: &Certificate<'_>,
    point: &Point,
    info: &Info,
    budget: &mut Budget<'_>,
) -> Check<u16> {
    let issuer = Dn::read(certificate.issuer, budget)?;
    if let Some(issuers) = &point.issuers {
        let mut matched = false;
        for name in issuers {
            if let DpName::Directory(dn) = name {
                matched |= dn.equal(&info.issuer, budget)?;
            }
        }
        if !matched || !info.scope.indirect {
            return Ok(0);
        }
    } else if !issuer.equal(&info.issuer, budget)? {
        return Ok(0);
    }
    if let Some(names) = &info.scope.names {
        let expected = point
            .names
            .as_ref()
            .or(point.issuers.as_ref())
            .ok_or_else(malformed)?;
        if !intersects(names, expected, budget)? {
            return Ok(0);
        }
    }
    let ca = if let Some(ext) = certificate.extension(extensions::BASIC_CONSTRAINTS) {
        let doc = document(ext, budget)?;
        BasicConstraints::parse(doc.root())
            .map_err(|_| malformed())?
            .ca
    } else {
        false
    };
    if info.scope.class == CertificateClass::Attribute
        || (info.scope.class == CertificateClass::User && ca)
        || (info.scope.class == CertificateClass::Ca && !ca)
        || (matches!(
            info.scope.class,
            CertificateClass::User | CertificateClass::Ca
        ) && certificate.version < 3)
    {
        return Ok(0);
    }
    Ok(point.reasons & info.scope.reasons)
}

pub(super) fn reason(value: Element<'_>) -> Check<RevocationReason> {
    value
        .expect(Tag::universal(10, false))
        .map_err(|_| malformed())?;
    let [n] = value.content() else {
        return Err(malformed());
    };
    RevocationReason::from_number(*n).ok_or_else(malformed)
}
pub(super) fn entry_extensions(
    crl: &CertificateList<'_>,
    info: &Info,
    budget: &mut Budget<'_>,
) -> Check<()> {
    for entry in &crl.revoked {
        budget.node()?;
        budget.string(entry.serial.as_bytes().len())?;
        for &ext in &entry.extensions {
            budget.node()?;
            match ext.id.as_bytes() {
                [85, 29, 21] => {
                    if ext.critical {
                        return Err(malformed());
                    }
                    let doc = document(ext, budget)?;
                    let reason = reason(doc.root())?;
                    if reason == RevocationReason::RemoveFromCrl && info.delta.is_none() {
                        return Err(malformed());
                    }
                }
                [85, 29, 24] => {
                    if ext.critical {
                        return Err(malformed());
                    }
                    let doc = document(ext, budget)?;
                    doc.root()
                        .expect(Tag::universal(24, false))
                        .map_err(|_| malformed())?;
                    Time::parse(doc.root()).map_err(|_| malformed())?;
                }
                [85, 29, 29] => {
                    if !ext.critical || !info.scope.indirect {
                        return Err(malformed());
                    }
                    let doc = document(ext, budget)?;
                    doc.root().expect(SEQUENCE).map_err(|_| malformed())?;
                    if !names(doc.root(), budget)?
                        .iter()
                        .any(|n| matches!(n, DpName::Directory(_)))
                    {
                        return Err(malformed());
                    }
                }
                _ if ext.critical => {
                    return Err(unknown(UnknownReason::UnsupportedCriticalExtension));
                }
                _ => {}
            }
        }
    }
    Ok(())
}
