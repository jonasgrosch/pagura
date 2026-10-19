//! X.509 certificate structures (RFC 5280 4).
//!
//! This module checks encodings and field structure. Certificate policy,
//! algorithm policy, name matching and RFC 5280 6 path validation are
//! separate. Unknown extensions remain visible, including their critical bit.

mod crl;
pub mod extensions;

pub use crl::{CertificateList, CrlVersion, RevokedCertificate};

use crate::der::{self, Document, Element, Fields, Form, SEQUENCE, SET, Tag, single};
use crate::{BitString, Integer, ObjectIdentifier, Time, types};
use pagura_core::Result;
use std::collections::BTreeSet;
use std::ops::Range;

/// TBSCertificate's context-specific fields (RFC 5280 4.1).
const VERSION: Tag = Tag::context(0, Form::Constructed);
const ISSUER_UNIQUE_ID: Tag = Tag::context(1, Form::Primitive);
const SUBJECT_UNIQUE_ID: Tag = Tag::context(2, Form::Primitive);
const EXTENSIONS: Tag = Tag::context(3, Form::Constructed);

/// Certificate version (RFC 5280 4.1.2.1). The standard defines exactly
/// three, so the enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Version {
    /// Version 1, the DEFAULT, encoded by omission.
    V1,
    /// Version 2, which adds unique identifiers.
    V2,
    /// Version 3, which adds extensions.
    V3,
}

impl Version {
    /// The version an explicit INTEGER encodes (`v2(1)`, `v3(2)`); DER omits
    /// the DEFAULT `v1(0)` (X.690 11.5), so it is not accepted here.
    const fn explicit(value: Option<u64>) -> Option<Self> {
        match value {
            Some(1) => Some(Self::V2),
            Some(2) => Some(Self::V3),
            _ => None,
        }
    }
}

/// Algorithm identifier and unmodified optional parameters (RFC 5280 4.1.1.2).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct AlgorithmIdentifier<'a> {
    /// Algorithm OID; unsupported algorithms are still representable data.
    pub algorithm: ObjectIdentifier<'a>,
    /// Exact optional parameters, whose meaning is algorithm-specific.
    pub parameters: Option<Element<'a>>,
    /// Exact original SEQUENCE.
    pub element: Element<'a>,
}

impl<'a> AlgorithmIdentifier<'a> {
    /// Reads an algorithm OID and at most one parameter.
    ///
    /// # Errors
    /// Rejects malformed structure or additional fields.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let algorithm = ObjectIdentifier::parse(fields.take()?)?;
        let parameters = fields.optional_any();
        fields.finish()?;
        Ok(Self {
            algorithm,
            parameters,
            element,
        })
    }
}

/// A distinguished name retaining its RDNs and attribute string encodings.
/// No byte equality is claimed to implement RFC 5280 7.1 name matching.
#[derive(Debug, Clone, Copy)]
pub struct Name<'a> {
    element: Element<'a>,
}

impl<'a> Name<'a> {
    /// Checks the RDNSequence structure and each RDN's SET OF ordering.
    ///
    /// # Errors
    /// Rejects malformed names, empty RDNs and noncanonical set ordering.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.expect(SEQUENCE)?;
        for rdn in element.children() {
            rdn.expect(SET)?;
            let mut empty = true;
            for attribute in rdn.set_of()? {
                let mut fields = Fields::sequence(attribute)?;
                ObjectIdentifier::parse(fields.take()?)?;
                fields.take()?; // AttributeValue is ANY, not necessarily DirectoryString.
                fields.finish()?;
                empty = false;
            }
            if empty {
                return Err(rdn.error("empty relative distinguished name"));
            }
        }
        Ok(Self { element })
    }
    /// Original RDNSequence; children are RDN SET OF values.
    pub const fn element(self) -> Element<'a> {
        self.element
    }
}

/// One v3 extension (RFC 5280 4.1.2.9).
#[derive(Debug, Clone, Copy)]
pub struct Extension<'a> {
    id: ObjectIdentifier<'a>,
    critical: bool,
    value: Element<'a>,
    element: Element<'a>,
}

impl<'a> Extension<'a> {
    /// Reads the Extension envelope and rejects an explicit DEFAULT FALSE.
    ///
    /// # Errors
    /// Rejects missing, mistyped or excess fields and non-DER defaults.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let id = ObjectIdentifier::parse(fields.take()?)?;
        let critical = optional_true(&mut fields, "explicit extension DEFAULT FALSE")?;
        let value = fields.take()?;
        types::octets(value)?;
        fields.finish()?;
        Ok(Self {
            id,
            critical,
            value,
            element,
        })
    }
    /// Extension type.
    pub const fn id(self) -> ObjectIdentifier<'a> {
        self.id
    }
    /// Whether the extension is critical. An unknown critical extension must
    /// prevent path acceptance (RFC 5280 4.2).
    pub const fn is_critical(self) -> bool {
        self.critical
    }
    /// DER octets inside extnValue, not the OCTET STRING wrapper.
    pub fn value(self) -> &'a [u8] {
        self.value.content()
    }
    /// Original Extension SEQUENCE.
    pub const fn element(self) -> Element<'a> {
        self.element
    }
    /// Parses extnValue's single DER value with original-buffer error offsets.
    /// Use [`extensions`] for common typed values. Even unknown values remain
    /// accessible, and parsing one never marks an extension as understood.
    ///
    /// # Errors
    /// Rejects malformed, noncanonical, unsupported or over-budget contents.
    pub fn value_der(self) -> Result<Document<'a>> {
        Document::parse_at(
            self.value.content(),
            self.value.content_offset(),
            self.element.limits(),
        )
    }
}

/// Reads a BOOLEAN DEFAULT FALSE field, which DER encodes only when TRUE
/// (X.690 11.5).
pub(crate) fn optional_true(
    fields: &mut Fields<'_>,
    explicit_default: &'static str,
) -> Result<bool> {
    match fields.optional(der::BOOLEAN) {
        Some(value) if types::boolean(value)? => Ok(true),
        Some(value) => Err(value.error(explicit_default)),
        None => Ok(false),
    }
}

/// Reads an Extensions SEQUENCE, or its schema's IMPLICIT constructed wrapper.
/// Empty lists and repeated OIDs are rejected (RFC 5280 4.2).
///
/// # Errors
/// Rejects malformed envelopes, duplicate OIDs and empty extension lists.
pub fn parse_extensions(element: Element<'_>) -> Result<Vec<Extension<'_>>> {
    if !element.tag().is_constructed() {
        return Err(element.error("primitive Extensions"));
    }
    let mut seen = BTreeSet::new();
    let mut extensions = Vec::new();
    for child in element.children() {
        let extension = Extension::parse(child)?;
        if !seen.insert(extension.id) {
            return Err(child.error("duplicate X.509 extension"));
        }
        extensions.push(extension);
    }
    if extensions.is_empty() {
        return Err(element.error("empty Extensions"));
    }
    Ok(extensions)
}

/// A subject public key, kept in the algorithm's original bit representation.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct SubjectPublicKeyInfo<'a> {
    /// Key algorithm and parameters.
    pub algorithm: AlgorithmIdentifier<'a>,
    /// Encoded public key bits; interpretation belongs to the algorithm.
    pub key: BitString<'a>,
    /// Exact SubjectPublicKeyInfo SEQUENCE.
    pub element: Element<'a>,
}

impl<'a> SubjectPublicKeyInfo<'a> {
    /// Reads SubjectPublicKeyInfo (RFC 5280 4.1.2.7).
    ///
    /// # Errors
    /// Rejects malformed field structure and invalid bit strings.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        let key = BitString::parse(fields.take()?)?;
        fields.finish()?;
        Ok(Self {
            algorithm,
            key,
            element,
        })
    }
}

/// A certificate's parsed fields. Parsing never establishes signer trust.
#[derive(Debug)]
#[non_exhaustive]
pub struct Certificate<'a> {
    /// Original certificate encoding.
    pub element: Element<'a>,
    /// Exact signed TBSCertificate, including its SEQUENCE tag and length.
    pub tbs: Element<'a>,
    /// Certificate version.
    pub version: Version,
    /// Serial number, retaining sign and width for later profile checks.
    pub serial: Integer<'a>,
    /// Signature algorithm within TBSCertificate, encoded as the outer one.
    pub tbs_signature_algorithm: AlgorithmIdentifier<'a>,
    /// Issuer distinguished name.
    pub issuer: Name<'a>,
    /// First asserted validity time, without a host-clock comparison.
    pub not_before: Time<'a>,
    /// Last asserted validity time.
    pub not_after: Time<'a>,
    /// Subject distinguished name.
    pub subject: Name<'a>,
    /// Subject key and its algorithm.
    pub public_key: SubjectPublicKeyInfo<'a>,
    /// Optional issuer unique identifier.
    pub issuer_unique_id: Option<BitString<'a>>,
    /// Optional subject unique identifier.
    pub subject_unique_id: Option<BitString<'a>>,
    /// Original extension envelopes, including unknown critical extensions.
    pub extensions: Vec<Extension<'a>>,
    /// Outer signature algorithm.
    pub signature_algorithm: AlgorithmIdentifier<'a>,
    /// Signature bits, unverified.
    pub signature: BitString<'a>,
}

impl<'a> Certificate<'a> {
    /// Reads an X.509 certificate from a DER tree (RFC 5280 4).
    ///
    /// # Errors
    /// Rejects malformed fields, inconsistent version use, noncanonical defaults,
    /// duplicate extensions or unequal signature algorithm encodings.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.require_der()?;
        let mut outer = Fields::sequence(element)?;
        let tbs = outer.take()?.expect(SEQUENCE)?;
        let signature_algorithm = AlgorithmIdentifier::parse(outer.take()?)?;
        let signature = BitString::parse(outer.take()?)?;
        outer.finish()?;
        let mut fields = Fields::sequence(tbs)?;
        let version = match fields.optional(VERSION) {
            Some(explicit) => Version::explicit(Integer::parse(single(explicit)?)?.to_u64())
                .ok_or_else(|| explicit.error("invalid or explicit default certificate version"))?,
            None => Version::V1,
        };
        let serial = Integer::parse(fields.take()?)?;
        let tbs_signature_algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        if tbs_signature_algorithm.element.encoded() != signature_algorithm.element.encoded() {
            return Err(signature_algorithm
                .element
                .error("certificate signature algorithms differ"));
        }
        let issuer = Name::parse(fields.take()?)?;
        let mut times = Fields::sequence(fields.take()?)?;
        let not_before = Time::parse(times.take()?)?;
        let not_after = Time::parse(times.take()?)?;
        times.finish()?;
        let subject = Name::parse(fields.take()?)?;
        let public_key = SubjectPublicKeyInfo::parse(fields.take()?)?;
        let issuer_unique_id = implicit_bits(fields.optional(ISSUER_UNIQUE_ID))?;
        let subject_unique_id = implicit_bits(fields.optional(SUBJECT_UNIQUE_ID))?;
        if version == Version::V1 && (issuer_unique_id.is_some() || subject_unique_id.is_some()) {
            return Err(tbs.error("unique identifiers require certificate v2 or v3"));
        }
        let extensions = match fields.optional(EXTENSIONS) {
            Some(explicit) if version == Version::V3 => {
                parse_extensions(single(explicit)?.expect(SEQUENCE)?)?
            }
            Some(explicit) => return Err(explicit.error("extensions require certificate v3")),
            None => Vec::new(),
        };
        fields.finish()?;
        Ok(Self {
            element,
            tbs,
            version,
            serial,
            tbs_signature_algorithm,
            issuer,
            not_before,
            not_after,
            subject,
            public_key,
            issuer_unique_id,
            subject_unique_id,
            extensions,
            signature_algorithm,
            signature,
        })
    }
    /// Exact signed byte range in the original input buffer.
    pub fn tbs_range(&self) -> Range<usize> {
        self.tbs.range()
    }
    /// Finds an extension by type.
    pub fn extension(&self, id: ObjectIdentifier<'_>) -> Option<Extension<'a>> {
        self.extensions
            .iter()
            .find(|extension| extension.id == id)
            .copied()
    }
}

fn implicit_bits(element: Option<Element<'_>>) -> Result<Option<BitString<'_>>> {
    element
        .map(|element| BitString::from_bytes(element.content(), element.content_offset()))
        .transpose()
}
