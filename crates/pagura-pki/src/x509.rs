//! X.509 certificate structures (RFC 5280 §4).
//!
//! This module checks encodings and field structure. Certificate policy,
//! algorithm policy, name matching and RFC 5280 §6 path validation are separate.
//! Unknown extensions remain visible, including their critical bit.

mod crl;
pub mod extensions;
pub use crl::{CertificateList, RevokedCertificate};

use crate::der::{Document, Element, Fields, SEQUENCE, SET, Tag, single};
use crate::{BitString, Integer, ObjectIdentifier, Time, types};
use pagura_core::Result;
use std::collections::BTreeSet;
use std::ops::Range;

/// Algorithm identifier and unmodified optional parameters (RFC 5280 §4.1.1.2).
#[derive(Debug, Clone, Copy)]
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
        let parameters = fields.rest();
        fields.finish()?;
        Ok(Self {
            algorithm,
            parameters,
            element,
        })
    }
}

/// A distinguished name retaining its RDNs and attribute string encodings.
/// No byte equality is claimed to implement RFC 5280 §7.1 name matching.
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
            let mut count = 0;
            for attribute in rdn.set_of()? {
                let mut fields = Fields::sequence(attribute)?;
                ObjectIdentifier::parse(fields.take()?)?;
                fields.take()?; // AttributeValue is ANY, not necessarily DirectoryString.
                fields.finish()?;
                count += 1;
            }
            if count == 0 {
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

/// One v3 extension (RFC 5280 §4.1.2.9).
#[derive(Debug, Clone, Copy)]
pub struct Extension<'a> {
    /// Extension type.
    pub id: ObjectIdentifier<'a>,
    /// Unknown critical extensions must prevent path acceptance later.
    pub critical: bool,
    /// DER octets inside extnValue, not the OCTET STRING wrapper.
    pub value: &'a [u8],
    element: Element<'a>,
    offset: usize,
}

impl<'a> Extension<'a> {
    /// Reads the Extension envelope and rejects an explicit DEFAULT FALSE.
    ///
    /// # Errors
    /// Rejects missing, mistyped or excess fields and non-DER defaults.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let id = ObjectIdentifier::parse(fields.take()?)?;
        let critical = if let Some(value) = fields.optional(Tag::universal(1, false)) {
            if !types::boolean(value)? {
                return Err(value.error("explicit extension DEFAULT FALSE"));
            }
            true
        } else {
            false
        };
        let value = fields.take()?;
        let offset = value.content_offset();
        let bytes = types::octets(value)?;
        fields.finish()?;
        Ok(Self {
            id,
            critical,
            value: bytes,
            element,
            offset,
        })
    }
    /// Parses extnValue's single DER value with original-buffer error offsets.
    /// Use [`extensions`] for common typed values. Even unknown values remain
    /// accessible, and parsing one never marks an extension as understood.
    ///
    /// # Errors
    /// Rejects malformed, noncanonical, unsupported or over-budget contents.
    pub fn value_der(self) -> Result<Document<'a>> {
        Document::parse_at(self.value, self.offset, self.element.limits())
    }
}

/// Reads an Extensions SEQUENCE, or its schema's IMPLICIT constructed wrapper.
/// Empty lists and repeated OIDs are rejected (RFC 5280 §4.2).
///
/// # Errors
/// Rejects malformed envelopes, duplicate OIDs and empty extension lists.
pub fn parse_extensions(element: Element<'_>) -> Result<Vec<Extension<'_>>> {
    if !element.tag().constructed {
        return Err(element.error("primitive Extensions"));
    }
    let mut seen = BTreeSet::new();
    let mut extensions = Vec::new();
    for child in element.children() {
        let extension = Extension::parse(child)?;
        if !seen.insert(extension.id.as_bytes()) {
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
pub struct SubjectPublicKeyInfo<'a> {
    /// Key algorithm and parameters.
    pub algorithm: AlgorithmIdentifier<'a>,
    /// Encoded public key bits; interpretation belongs to the algorithm.
    pub key: BitString<'a>,
    /// Exact SubjectPublicKeyInfo SEQUENCE.
    pub element: Element<'a>,
}

impl<'a> SubjectPublicKeyInfo<'a> {
    /// Reads SubjectPublicKeyInfo (RFC 5280 §4.1.2.7).
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
pub struct Certificate<'a> {
    /// Original certificate encoding.
    pub element: Element<'a>,
    /// Exact signed TBSCertificate, including its SEQUENCE tag and length.
    pub tbs: Element<'a>,
    /// Version number expressed as 1, 2 or 3.
    pub version: u8,
    /// Serial number, retaining sign and width for later profile checks.
    pub serial: Integer<'a>,
    /// Signature algorithm within TBSCertificate.
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
    /// Outer signature algorithm, structurally matched against the TBS value.
    pub signature_algorithm: AlgorithmIdentifier<'a>,
    /// Signature bits, unverified.
    pub signature: BitString<'a>,
}

impl<'a> Certificate<'a> {
    /// Reads an X.509 certificate from a DER tree (RFC 5280 §4).
    ///
    /// # Errors
    /// Rejects malformed fields, inconsistent version use, noncanonical defaults,
    /// duplicate extensions or unequal signature algorithm encodings.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        // Recheck BER-origin certificates without normalizing any signed bytes.
        if element.is_ber() {
            Document::parse_at(element.encoded(), element.range().start, element.limits())?;
        }
        let mut outer = Fields::sequence(element)?;
        let tbs = outer.take()?.expect(SEQUENCE)?;
        let signature_algorithm = AlgorithmIdentifier::parse(outer.take()?)?;
        let signature = BitString::parse(outer.take()?)?;
        outer.finish()?;
        let mut fields = Fields::sequence(tbs)?;
        let version = if let Some(explicit) = fields.optional(Tag::context(0, true)) {
            let value = Integer::parse(single(explicit)?)?.to_u64()?;
            match value {
                1 => 2,
                2 => 3,
                _ => return Err(explicit.error("invalid or explicit default certificate version")),
            }
        } else {
            1
        };
        let serial = Integer::parse(fields.take()?)?;
        let tbs_signature_algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        if tbs_signature_algorithm.element.encoded() != signature_algorithm.element.encoded() {
            return Err(signature_algorithm
                .element
                .error("certificate signature algorithms differ"));
        }
        let issuer = Name::parse(fields.take()?)?;
        let validity = fields.take()?;
        let mut times = Fields::sequence(validity)?;
        let not_before = Time::parse(times.take()?)?;
        let not_after = Time::parse(times.take()?)?;
        times.finish()?;
        let subject = Name::parse(fields.take()?)?;
        let public_key = SubjectPublicKeyInfo::parse(fields.take()?)?;
        let issuer_unique_id = fields
            .optional(Tag::context(1, false))
            .map(|e| BitString::from_bytes(e.content(), e.content_offset()))
            .transpose()?;
        let subject_unique_id = fields
            .optional(Tag::context(2, false))
            .map(|e| BitString::from_bytes(e.content(), e.content_offset()))
            .transpose()?;
        if version == 1 && (issuer_unique_id.is_some() || subject_unique_id.is_some()) {
            return Err(tbs.error("unique identifiers require certificate v2 or v3"));
        }
        let extensions = if let Some(explicit) = fields.optional(Tag::context(3, true)) {
            if version != 3 {
                return Err(explicit.error("extensions require certificate v3"));
            }
            parse_extensions(single(explicit)?.expect(SEQUENCE)?)?
        } else {
            Vec::new()
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
    /// Finds an extension by canonical OID content bytes.
    pub fn extension(&self, oid: &[u8]) -> Option<Extension<'a>> {
        self.extensions
            .iter()
            .find(|extension| extension.id.is(oid))
            .copied()
    }
}
