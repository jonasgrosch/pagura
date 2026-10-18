//! CMS SignedData (RFC 5652 §§3 and 5).
//!
//! Signatures and digests are unverified data. BER containers are admitted only
//! through [`crate::der::Document::parse_ber`]; signed attributes are always DER.

use crate::der::{Class, Document, Element, Fields, SEQUENCE, SET, Tag, single};
use crate::x509::{AlgorithmIdentifier, Certificate, CertificateList, Name};
use crate::{Integer, ObjectIdentifier, types};
use pagura_core::{Error, Position, Result};

/// id-data OID contents, 1.2.840.113549.1.7.1.
pub const ID_DATA: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 7, 1];
/// id-signedData OID contents, 1.2.840.113549.1.7.2.
pub const ID_SIGNED_DATA: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 7, 2];
/// content-type attribute OID contents, 1.2.840.113549.1.9.3.
pub const CONTENT_TYPE: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 9, 3];
/// message-digest attribute OID contents, 1.2.840.113549.1.9.4.
pub const MESSAGE_DIGEST: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 9, 4];

/// A primitive or constructed OCTET STRING in a CMS BER container.
#[derive(Debug, Clone, Copy)]
pub struct OctetString<'a> {
    element: Element<'a>,
}

impl<'a> OctetString<'a> {
    /// Checks that every component is an OCTET STRING (X.690 §8.7).
    ///
    /// # Errors
    /// Rejects other tags at any depth.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        for value in element.walk() {
            if value.tag().class != Class::Universal || value.tag().number != 4 {
                return Err(value.error("non-OCTET STRING component"));
            }
        }
        Ok(Self { element })
    }
    /// Borrows data when the encoding is primitive.
    pub fn as_primitive(self) -> Option<&'a [u8]> {
        if self.element.tag().constructed {
            None
        } else {
            Some(self.element.content())
        }
    }
    /// Concatenates BER components in encoded order, under `decoded_bytes`.
    ///
    /// # Errors
    /// Rejects a decoded size above the byte budget or allocation failure.
    pub fn to_bytes(self) -> Result<Vec<u8>> {
        let limit_error = || {
            Error::limit("CMS OCTET STRING byte budget exceeded")
                .at(Position::Buffer(self.element.range().start))
        };
        let size = self
            .element
            .walk()
            .filter(|value| !value.tag().constructed)
            .try_fold(0usize, |size, value| {
                size.checked_add(value.content().len())
                    .filter(|&size| size <= self.element.limits().decoded_bytes)
                    .ok_or_else(limit_error)
            })?;
        let mut output = Vec::new();
        output.try_reserve_exact(size).map_err(|_| limit_error())?;
        for value in self.element.walk().filter(|value| !value.tag().constructed) {
            output.extend_from_slice(value.content());
        }
        Ok(output)
    }
    /// Original encoding, including constructed components.
    pub const fn element(self) -> Element<'a> {
        self.element
    }
}

/// One attribute, with values left available as original encodings (§5.3).
#[derive(Debug)]
pub struct Attribute<'a> {
    /// Attribute identifier.
    pub id: ObjectIdentifier<'a>,
    /// Nonempty value SET OF.
    pub values: Element<'a>,
    /// Original Attribute SEQUENCE.
    pub element: Element<'a>,
}

fn attributes(element: Element<'_>) -> Result<Vec<Attribute<'_>>> {
    let children = element.set_of()?;
    let mut result = Vec::new();
    for attribute in children {
        let mut fields = Fields::sequence(attribute)?;
        let id = ObjectIdentifier::parse(fields.take()?)?;
        let values = fields.take()?.expect(SET)?;
        values.set_of()?;
        if values.children().next().is_none() {
            return Err(values.error("empty CMS attribute values"));
        }
        fields.finish()?;
        result.push(Attribute {
            id,
            values,
            element: attribute,
        });
    }
    if result.is_empty() {
        return Err(element.error("empty CMS attributes"));
    }
    Ok(result)
}

/// Signed attributes with the exact DER SET encoding used as signature input.
#[derive(Debug)]
pub struct SignedAttributes<'a> {
    /// Original IMPLICIT `[0]` encoding. It is never hashed as-is.
    pub element: Element<'a>,
    /// Checked, sorted attribute envelopes, without semantic acceptance.
    pub attributes: Vec<Attribute<'a>>,
    signature_input: Vec<u8>,
}

impl<'a> SignedAttributes<'a> {
    /// Reads signedAttrs, replacing only its `[0]` identifier with SET (RFC 5652
    /// §§5.3–5.4). Lengths and contents must already be canonical DER.
    ///
    /// # Errors
    /// Rejects BER deviations, unordered attributes/values and empty sets.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.expect(Tag::context(0, true))?;
        let encoded = element.encoded();
        if encoded.len() > element.limits().decoded_bytes {
            return Err(Error::limit("CMS signed attribute byte budget exceeded")
                .at(Position::Buffer(element.range().start)));
        }
        let mut signature_input = encoded.to_vec();
        if let Some(tag) = signature_input.first_mut() {
            *tag = 0x31;
        }
        // A BER outer tree must not confer tolerance on signed attributes.
        Document::parse_at(&signature_input, element.range().start, element.limits())?;
        let attributes = attributes(element)?;
        Ok(Self {
            element,
            attributes,
            signature_input,
        })
    }
    /// DER SET encoding for signature verification, with the universal tag.
    pub fn signature_input(&self) -> &[u8] {
        &self.signature_input
    }
    /// Finds the unique attribute with this OID.
    ///
    /// # Errors
    /// Rejects duplicate occurrences rather than choosing one.
    pub fn unique(&self, oid: &[u8]) -> Result<Option<&Attribute<'a>>> {
        let mut matches = self.attributes.iter().filter(|value| value.id.is(oid));
        let first = matches.next();
        if let Some(second) = matches.next() {
            return Err(second.element.error("duplicate CMS attribute"));
        }
        Ok(first)
    }
    /// Checks the mandatory content-type and message-digest attribute shapes.
    /// No digest comparison is performed.
    ///
    /// # Errors
    /// Rejects absent, repeated, multivalued or wrongly typed required attributes
    /// and a content-type value different from encapContentInfo.
    pub fn content_digest(&self, content_type: ObjectIdentifier<'_>) -> Result<&'a [u8]> {
        let content = self
            .unique(CONTENT_TYPE)?
            .ok_or_else(|| self.element.error("missing CMS content-type attribute"))?;
        let value = ObjectIdentifier::parse(single(content.values)?)?;
        if value != content_type {
            return Err(content.element.error("CMS content-type attribute mismatch"));
        }
        let digest = self
            .unique(MESSAGE_DIGEST)?
            .ok_or_else(|| self.element.error("missing CMS message-digest attribute"))?;
        types::octets(single(digest.values)?)
    }
}

/// Signer identifier alternatives (§5.3).
#[derive(Debug, Clone, Copy)]
pub enum SignerIdentifier<'a> {
    /// Certificate issuer and serial number.
    IssuerAndSerial {
        /// Issuer name, without a claim of bytewise name matching.
        issuer: Name<'a>,
        /// Arbitrary-size serial number.
        serial: Integer<'a>,
    },
    /// Certificate subject key identifier octets.
    SubjectKeyIdentifier(&'a [u8]),
}

/// An unverified CMS signer (§5.3).
#[derive(Debug)]
pub struct SignerInfo<'a> {
    /// Version, restricted to the identifier's required 1 or 3.
    pub version: u8,
    /// Identifier of the claimed signer.
    pub sid: SignerIdentifier<'a>,
    /// Claimed content digest algorithm.
    pub digest_algorithm: AlgorithmIdentifier<'a>,
    /// Optional DER signed attributes and signature input.
    pub signed_attributes: Option<SignedAttributes<'a>>,
    /// Claimed signature algorithm and parameters.
    pub signature_algorithm: AlgorithmIdentifier<'a>,
    /// Unverified signature octets.
    pub signature: OctetString<'a>,
    /// Optional unsigned attributes; never treated as verified assertions.
    pub unsigned_attributes: Vec<Attribute<'a>>,
}

impl<'a> SignerInfo<'a> {
    /// Reads a SignerInfo without performing verification or accepting policy.
    ///
    /// # Errors
    /// Rejects structural errors, inconsistent version or non-DER signedAttrs.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let version_element = fields.take()?;
        let version = Integer::parse(version_element)?.to_u64()?;
        let identifier = fields.take()?;
        let sid = if identifier.tag() == SEQUENCE {
            let mut fields = Fields::sequence(identifier)?;
            let issuer = Name::parse(fields.take()?)?;
            let serial = Integer::parse(fields.take()?)?;
            fields.finish()?;
            if version != 1 {
                return Err(
                    version_element.error("issuerAndSerialNumber requires SignerInfo version 1")
                );
            }
            SignerIdentifier::IssuerAndSerial { issuer, serial }
        } else {
            identifier.expect(Tag::context(0, false))?;
            if version != 3 {
                return Err(
                    version_element.error("subjectKeyIdentifier requires SignerInfo version 3")
                );
            }
            SignerIdentifier::SubjectKeyIdentifier(identifier.content())
        };
        let version = u8::try_from(version)
            .map_err(|_| version_element.error("invalid SignerInfo version"))?;
        let digest_algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        let signed_attributes = fields
            .optional(Tag::context(0, true))
            .map(SignedAttributes::parse)
            .transpose()?;
        let signature_algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        let signature = OctetString::parse(fields.take()?)?;
        let unsigned_attributes = fields
            .optional(Tag::context(1, true))
            .map(|value| attributes(value))
            .transpose()?
            .unwrap_or_default();
        fields.finish()?;
        Ok(Self {
            version,
            sid,
            digest_algorithm,
            signed_attributes,
            signature_algorithm,
            signature,
            unsigned_attributes,
        })
    }
}

/// A CertificateChoices value (RFC 5652 §10.2.2).
#[derive(Debug)]
pub enum CertificateChoice<'a> {
    /// A parsed X.509 certificate; it is not a trust anchor.
    X509(Box<Certificate<'a>>),
    /// Extended, attribute or other certificate form, retained without acceptance.
    Other(Element<'a>),
}

/// A RevocationInfoChoice (§10.2.1), without a freshness or validity assertion.
#[derive(Debug)]
pub enum RevocationInfo<'a> {
    /// Exact CertificateList; interpretation and verification are separate.
    Crl(Box<CertificateList<'a>>),
    /// OtherRevocationInfoFormat, e.g. externally supplied OCSP evidence.
    Other {
        /// Revocation format identifier.
        format: ObjectIdentifier<'a>,
        /// Unmodified evidence value.
        value: Element<'a>,
    },
}

/// CMS SignedData with detached or encapsulated content (§5.1).
#[derive(Debug)]
pub struct SignedData<'a> {
    /// Original SignedData SEQUENCE.
    pub element: Element<'a>,
    /// Syntax version, checked against its contents.
    pub version: u8,
    /// Declared digest algorithm set.
    pub digest_algorithms: Vec<AlgorithmIdentifier<'a>>,
    /// Encapsulated content type, not necessarily id-data.
    pub content_type: ObjectIdentifier<'a>,
    /// Absent for detached signatures.
    pub content: Option<OctetString<'a>>,
    /// Embedded certificate choices; none becomes trusted by being embedded.
    pub certificates: Vec<CertificateChoice<'a>>,
    /// Original revocation evidence.
    pub revocation: Vec<RevocationInfo<'a>>,
    /// Signer structures, each still unverified.
    pub signers: Vec<SignerInfo<'a>>,
}

impl<'a> SignedData<'a> {
    /// Reads a ContentInfo containing id-signedData (§3).
    ///
    /// # Errors
    /// Rejects wrong content type, absent content or malformed SignedData.
    pub fn from_content_info(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let content_type = ObjectIdentifier::parse(fields.take()?)?;
        if !content_type.is(ID_SIGNED_DATA) {
            return Err(element.error("ContentInfo is not signedData"));
        }
        let explicit = fields.take()?.expect(Tag::context(0, true))?;
        fields.finish()?;
        Self::parse(single(explicit)?)
    }

    /// Reads a bare SignedData SEQUENCE (§5.1).
    ///
    /// # Errors
    /// Rejects malformed fields, incorrect version, noncanonical DER sets and
    /// absent/mistyped mandatory signed attributes. No signature is verified.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let version_element = fields.take()?;
        let version = Integer::parse(version_element)?.to_u64()?;
        let algorithms = fields.take()?.expect(SET)?;
        let digest_algorithms = algorithms
            .set_of()?
            .map(AlgorithmIdentifier::parse)
            .collect::<Result<Vec<_>>>()?;
        let encapsulated = fields.take()?;
        let mut content_fields = Fields::sequence(encapsulated)?;
        let content_type = ObjectIdentifier::parse(content_fields.take()?)?;
        let content = content_fields
            .optional(Tag::context(0, true))
            .map(|value| OctetString::parse(single(value)?))
            .transpose()?;
        content_fields.finish()?;
        let mut required_version = if content_type.is(ID_DATA) { 1 } else { 3 };
        let mut certificates = Vec::new();
        if let Some(set) = fields.optional(Tag::context(0, true)) {
            for choice in set.set_of()? {
                let certificate = if choice.tag() == SEQUENCE {
                    CertificateChoice::X509(Box::new(Certificate::parse(choice)?))
                } else {
                    let tag = choice.tag();
                    if tag.class != Class::Context || !tag.constructed || tag.number > 3 {
                        return Err(choice.error("invalid CertificateChoices tag"));
                    }
                    required_version = required_version.max(match tag.number {
                        1 => 3,
                        2 => 4,
                        3 => 5,
                        _ => 1,
                    });
                    CertificateChoice::Other(choice)
                };
                certificates.push(certificate);
            }
        }
        let mut revocation = Vec::new();
        if let Some(set) = fields.optional(Tag::context(1, true)) {
            for choice in set.set_of()? {
                let info = if choice.tag() == SEQUENCE {
                    RevocationInfo::Crl(Box::new(CertificateList::parse(choice)?))
                } else {
                    choice.expect(Tag::context(1, true))?;
                    let mut fields = Fields::new(choice);
                    let format = ObjectIdentifier::parse(fields.take()?)?;
                    let value = fields.take()?;
                    fields.finish()?;
                    required_version = 5;
                    RevocationInfo::Other { format, value }
                };
                revocation.push(info);
            }
        }
        let signer_set = fields.take()?.expect(SET)?;
        let signers = signer_set
            .set_of()?
            .map(SignerInfo::parse)
            .collect::<Result<Vec<_>>>()?;
        fields.finish()?;
        for signer in &signers {
            if signer.version == 3 {
                required_version = required_version.max(3);
            }
            if let Some(attributes) = &signer.signed_attributes {
                attributes.content_digest(content_type)?;
            } else if !content_type.is(ID_DATA) {
                return Err(element.error("non-data CMS content requires signed attributes"));
            }
        }
        if version != u64::from(required_version) {
            return Err(version_element.error("SignedData version does not match contents"));
        }
        Ok(Self {
            element,
            version: required_version,
            digest_algorithms,
            content_type,
            content,
            certificates,
            revocation,
            signers,
        })
    }
}
