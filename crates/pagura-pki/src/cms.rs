//! CMS SignedData (RFC 5652 3 and 5).
//!
//! Signatures and digests are unverified data. BER containers are admitted only
//! through [`crate::der::Document::parse_ber`]; signed attributes are always DER.

use crate::der::{Class, Document, Element, Fields, Form, SEQUENCE, SET, Tag, single, universal};
use crate::x509::{AlgorithmIdentifier, Certificate, CertificateList, Name};
use crate::{Integer, ObjectIdentifier, types};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostics, Error, ErrorKind, Meter, Position, Recovery, Result, ResultExt};

/// id-data, 1.2.840.113549.1.7.1.
pub const ID_DATA: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[42, 134, 72, 134, 247, 13, 1, 7, 1]);
/// id-signedData, 1.2.840.113549.1.7.2.
pub const ID_SIGNED_DATA: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[42, 134, 72, 134, 247, 13, 1, 7, 2]);
/// The content-type attribute, 1.2.840.113549.1.9.3.
pub const CONTENT_TYPE: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[42, 134, 72, 134, 247, 13, 1, 9, 3]);
/// The message-digest attribute, 1.2.840.113549.1.9.4.
pub const MESSAGE_DIGEST: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[42, 134, 72, 134, 247, 13, 1, 9, 4]);

/// ContentInfo's and EncapsulatedContentInfo's `[0] EXPLICIT` content
/// (RFC 5652 3, 5.2).
const EXPLICIT_CONTENT: Tag = Tag::context(0, Form::Constructed);
/// SignedData's IMPLICIT certificate and revocation sets (RFC 5652 5.1).
const CERTIFICATES: Tag = Tag::context(0, Form::Constructed);
const CRLS: Tag = Tag::context(1, Form::Constructed);
/// SignerInfo's IMPLICIT attribute sets and subject key identifier
/// (RFC 5652 5.3).
const SIGNED_ATTRIBUTES: Tag = Tag::context(0, Form::Constructed);
const UNSIGNED_ATTRIBUTES: Tag = Tag::context(1, Form::Constructed);
const SUBJECT_KEY_IDENTIFIER: Tag = Tag::context(0, Form::Primitive);
/// RevocationInfoChoice's `other [1] IMPLICIT` (RFC 5652 10.2.1).
const OTHER_REVOCATION: Tag = Tag::context(1, Form::Constructed);
/// The universal constructed SET identifier octet that replaces signed
/// attributes' `[0]` in the signature input (RFC 5652 5.4).
const SET_IDENTIFIER: u8 = 0x31;

/// CertificateChoices' IMPLICIT alternatives (RFC 5652 10.2.2).
mod choice {
    pub(super) const EXTENDED_CERTIFICATE: u32 = 0;
    pub(super) const V1_ATTRIBUTE_CERTIFICATE: u32 = 1;
    pub(super) const V2_ATTRIBUTE_CERTIFICATE: u32 = 2;
    pub(super) const OTHER: u32 = 3;
}

/// A CMSVersion of SignedData or SignerInfo (RFC 5652 5.1, 5.3, 10.2.5),
/// ordered by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum CmsVersion {
    /// `v1`.
    V1,
    /// `v3`.
    V3,
    /// `v4`.
    V4,
    /// `v5`.
    V5,
}

impl CmsVersion {
    const fn from_encoded(value: Option<u64>) -> Option<Self> {
        match value {
            Some(1) => Some(Self::V1),
            Some(3) => Some(Self::V3),
            Some(4) => Some(Self::V4),
            Some(5) => Some(Self::V5),
            _ => None,
        }
    }
}

/// A primitive or constructed OCTET STRING in a CMS BER container.
#[derive(Debug, Clone, Copy)]
pub struct OctetString<'a> {
    element: Element<'a>,
}

impl<'a> OctetString<'a> {
    /// Checks that every component is an OCTET STRING (X.690 8.7).
    ///
    /// # Errors
    /// Rejects other tags at any depth.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        for value in element.walk() {
            if !value.tag().is_universal(universal::OCTET_STRING) {
                return Err(value.error("non-OCTET STRING component"));
            }
        }
        Ok(Self { element })
    }
    /// Borrows data when the encoding is primitive.
    pub fn as_primitive(self) -> Option<&'a [u8]> {
        (!self.element.tag().is_constructed()).then(|| self.element.content())
    }
    /// Concatenates BER components in encoded order, under `asn1_bytes`.
    ///
    /// # Errors
    /// Rejects a decoded size above the byte budget or allocation failure.
    pub fn to_bytes(self) -> Result<Vec<u8>> {
        const BUDGET: &str = "CMS OCTET STRING byte budget exceeded";
        let at = Position::Buffer(self.element.range().start);
        let mut meter = Meter::new(
            u32_to_usize(self.element.limits().signatures.asn1_bytes),
            BUDGET,
        );
        for part in self.parts() {
            meter.charge(part.len()).at(at)?;
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(meter.used())
            .map_err(|_| Error::limit(BUDGET).at(at))?;
        for part in self.parts() {
            output.extend_from_slice(part);
        }
        Ok(output)
    }
    /// Original encoding, including constructed components.
    pub const fn element(self) -> Element<'a> {
        self.element
    }
    fn parts(self) -> impl Iterator<Item = &'a [u8]> {
        self.element
            .walk()
            .filter(|value| !value.tag().is_constructed())
            .map(Element::content)
    }
}

/// One attribute, with values left available as original encodings
/// (RFC 5652 5.3).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Attribute<'a> {
    /// Attribute identifier.
    pub id: ObjectIdentifier<'a>,
    /// Nonempty, DER-ordered value SET OF.
    pub values: Element<'a>,
    /// Original Attribute SEQUENCE.
    pub element: Element<'a>,
}

/// Signed attributes are DER (RFC 5652 5.3); unsigned ones may keep a BER
/// container's set order, reported through `diagnostics`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetOrder {
    Der,
    Reported,
}

fn attributes<'a>(
    element: Element<'a>,
    order: SetOrder,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<Attribute<'a>>> {
    let mut set_of = |element: Element<'a>| match order {
        SetOrder::Der => element.set_of(),
        SetOrder::Reported => element.set_of_with_diagnostics(diagnostics),
    };
    let children = set_of(element)?;
    let mut result = Vec::new();
    for attribute in children {
        let mut fields = Fields::sequence(attribute)?;
        let id = ObjectIdentifier::parse(fields.take()?)?;
        let values = fields.take()?.expect(SET)?;
        if set_of(values)?.next().is_none() {
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
    /// Reads signedAttrs, replacing only its `[0]` identifier with SET
    /// (RFC 5652 5.3-5.4). Lengths and contents must already be canonical DER.
    ///
    /// # Errors
    /// Rejects BER deviations, unordered attributes/values, empty sets and a
    /// copy above `asn1_bytes`.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.expect(SIGNED_ATTRIBUTES)?;
        let encoded = element.encoded();
        let mut signature_input = Vec::new();
        Meter::new(
            u32_to_usize(element.limits().signatures.asn1_bytes),
            "CMS signed attribute byte budget exceeded",
        )
        .reserve(&mut signature_input, encoded.len())
        .at(Position::Buffer(element.range().start))?;
        signature_input.extend_from_slice(encoded);
        if let Some(identifier) = signature_input.first_mut() {
            *identifier = SET_IDENTIFIER;
        }
        // A BER outer tree must not confer tolerance on signed attributes.
        Document::parse_at(&signature_input, element.range().start, element.limits())?;
        let attributes = attributes(
            element,
            SetOrder::Der,
            &mut Diagnostics::new(Recovery::Strict),
        )?;
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
    /// Finds the unique attribute of type `id`.
    ///
    /// # Errors
    /// Rejects duplicate occurrences rather than choosing one.
    pub fn unique(&self, id: ObjectIdentifier<'_>) -> Result<Option<Attribute<'a>>> {
        let mut matches = self.attributes.iter().filter(|value| value.id == id);
        let first = matches.next().copied();
        if let Some(second) = matches.next() {
            return Err(second.element.error("duplicate CMS attribute"));
        }
        Ok(first)
    }
    /// Checks the mandatory content-type and message-digest attribute shapes
    /// (RFC 5652 11.1-11.2) and returns the message digest. No digest
    /// comparison is performed.
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

/// Signer identifier alternatives (RFC 5652 5.3).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
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

impl<'a> SignerIdentifier<'a> {
    fn parse(identifier: Element<'a>) -> Result<Self> {
        if identifier.tag() == SEQUENCE {
            let mut fields = Fields::sequence(identifier)?;
            let issuer = Name::parse(fields.take()?)?;
            let serial = Integer::parse(fields.take()?)?;
            fields.finish()?;
            Ok(Self::IssuerAndSerial { issuer, serial })
        } else {
            identifier.expect(SUBJECT_KEY_IDENTIFIER)?;
            Ok(Self::SubjectKeyIdentifier(identifier.content()))
        }
    }

    /// The SignerInfo version this identifier requires (RFC 5652 5.3).
    const fn version(self) -> (CmsVersion, &'static str) {
        match self {
            Self::IssuerAndSerial { .. } => (
                CmsVersion::V1,
                "issuerAndSerialNumber requires SignerInfo version 1",
            ),
            Self::SubjectKeyIdentifier(_) => (
                CmsVersion::V3,
                "subjectKeyIdentifier requires SignerInfo version 3",
            ),
        }
    }
}

/// An unverified CMS signer (RFC 5652 5.3).
#[derive(Debug)]
#[non_exhaustive]
pub struct SignerInfo<'a> {
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
        Self::parse_with_diagnostics(element, &mut Diagnostics::new(Recovery::Strict))
    }

    /// Reads a SignerInfo whose unsigned attributes may keep a BER container's
    /// set order, reported through `diagnostics`. Signed attributes stay DER.
    ///
    /// # Errors
    /// As [`Self::parse`], except that unsigned SET OF ordering is reported.
    pub fn parse_with_diagnostics(
        element: Element<'a>,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let version_element = fields.take()?;
        let version = CmsVersion::from_encoded(Integer::parse(version_element)?.to_u64());
        let sid = SignerIdentifier::parse(fields.take()?)?;
        let (required, message) = sid.version();
        if version != Some(required) {
            return Err(version_element.error(message));
        }
        let digest_algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        let signed_attributes = fields
            .optional(SIGNED_ATTRIBUTES)
            .map(SignedAttributes::parse)
            .transpose()?;
        let signature_algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        let signature = OctetString::parse(fields.take()?)?;
        let unsigned_attributes = fields
            .optional(UNSIGNED_ATTRIBUTES)
            .map(|value| attributes(value, SetOrder::Reported, diagnostics))
            .transpose()?
            .unwrap_or_default();
        fields.finish()?;
        Ok(Self {
            sid,
            digest_algorithm,
            signed_attributes,
            signature_algorithm,
            signature,
            unsigned_attributes,
        })
    }
    /// Version, 1 or 3 as the identifier requires.
    pub const fn version(&self) -> CmsVersion {
        self.sid.version().0
    }
}

/// A CertificateChoices value (RFC 5652 10.2.2).
#[derive(Debug)]
#[non_exhaustive]
pub enum CertificateChoice<'a> {
    /// A parsed X.509 certificate; it is not a trust anchor.
    X509(Box<Certificate<'a>>),
    /// A framed certificate whose X.509 schema is malformed, kept for
    /// diagnostics; it can never be selected as a signer's certificate.
    Malformed {
        /// Original certificate encoding.
        element: Element<'a>,
        /// The positioned parsing failure.
        error: Error,
    },
    /// Extended, attribute or other certificate form, retained without acceptance.
    Other(Element<'a>),
}

/// A RevocationInfoChoice (RFC 5652 10.2.1), without a freshness or
/// validity assertion.
#[derive(Debug)]
#[non_exhaustive]
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

/// CMS SignedData with detached or encapsulated content (RFC 5652 5.1).
#[derive(Debug)]
#[non_exhaustive]
pub struct SignedData<'a> {
    /// Original SignedData SEQUENCE.
    pub element: Element<'a>,
    /// Syntax version, checked against the contents.
    pub version: CmsVersion,
    /// Declared digest algorithm set.
    pub digest_algorithms: Vec<AlgorithmIdentifier<'a>>,
    /// Encapsulated content type, not necessarily id-data.
    pub content_type: ObjectIdentifier<'a>,
    /// Encapsulated content; absent for detached signatures.
    pub content: Option<OctetString<'a>>,
    /// Embedded certificate choices; none becomes trusted by being embedded.
    pub certificates: Vec<CertificateChoice<'a>>,
    /// Original revocation evidence.
    pub revocation: Vec<RevocationInfo<'a>>,
    /// Signer structures, each still unverified.
    pub signers: Vec<SignerInfo<'a>>,
}

impl<'a> SignedData<'a> {
    /// Reads a ContentInfo containing id-signedData (RFC 5652 3).
    ///
    /// # Errors
    /// Rejects wrong content type, absent content or malformed SignedData.
    pub fn from_content_info(element: Element<'a>) -> Result<Self> {
        Self::from_content_info_with_diagnostics(element, &mut Diagnostics::new(Recovery::Strict))
    }

    /// Reads a ContentInfo as [`Self::from_content_info`] does, reporting a
    /// BER container's set order through `diagnostics`.
    ///
    /// # Errors
    /// As [`Self::from_content_info`], except that BER SET OF ordering is
    /// reported.
    pub fn from_content_info_with_diagnostics(
        element: Element<'a>,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let content_type = ObjectIdentifier::parse(fields.take()?)?;
        if content_type != ID_SIGNED_DATA {
            return Err(element.error("ContentInfo is not signedData"));
        }
        let explicit = fields.take()?.expect(EXPLICIT_CONTENT)?;
        fields.finish()?;
        Self::parse_with_diagnostics(single(explicit)?, diagnostics)
    }

    /// Reads a bare SignedData SEQUENCE (RFC 5652 5.1). A certificate whose
    /// X.509 schema is malformed is kept as [`CertificateChoice::Malformed`]
    /// rather than failing the whole SignedData.
    ///
    /// # Errors
    /// Rejects malformed fields, incorrect version, noncanonical DER sets,
    /// absent/mistyped mandatory signed attributes and exceeded budgets. No
    /// signature is verified.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        Self::parse_with_diagnostics(element, &mut Diagnostics::new(Recovery::Strict))
    }

    /// Reads a bare SignedData as [`Self::parse`] does, reporting a BER
    /// container's set order (digest algorithms, certificates, revocation
    /// evidence, signers and unsigned attributes) through `diagnostics`.
    ///
    /// # Errors
    /// As [`Self::parse`], except that BER SET OF ordering is reported.
    pub fn parse_with_diagnostics(
        element: Element<'a>,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let version_element = fields.take()?;
        let version = CmsVersion::from_encoded(Integer::parse(version_element)?.to_u64());
        let digest_algorithms = fields
            .take()?
            .expect(SET)?
            .set_of_with_diagnostics(diagnostics)?
            .map(AlgorithmIdentifier::parse)
            .collect::<Result<Vec<_>>>()?;
        let mut content_fields = Fields::sequence(fields.take()?)?;
        let content_type = ObjectIdentifier::parse(content_fields.take()?)?;
        let content = content_fields
            .optional(EXPLICIT_CONTENT)
            .map(|value| OctetString::parse(single(value)?))
            .transpose()?;
        content_fields.finish()?;
        // RFC 5652 5.1 derives the version from the contents.
        let mut required = if content_type == ID_DATA {
            CmsVersion::V1
        } else {
            CmsVersion::V3
        };
        let mut certificates = Vec::new();
        if let Some(set) = fields.optional(CERTIFICATES) {
            for choice in set.set_of_with_diagnostics(diagnostics)? {
                let (certificate, version) = certificate_choice(choice)?;
                required = required.max(version);
                certificates.push(certificate);
            }
        }
        let mut revocation = Vec::new();
        if let Some(set) = fields.optional(CRLS) {
            for choice in set.set_of_with_diagnostics(diagnostics)? {
                let info = if choice.tag() == SEQUENCE {
                    RevocationInfo::Crl(Box::new(CertificateList::parse(choice)?))
                } else {
                    choice.expect(OTHER_REVOCATION)?;
                    let mut fields = Fields::new(choice);
                    let format = ObjectIdentifier::parse(fields.take()?)?;
                    let value = fields.take()?;
                    fields.finish()?;
                    required = CmsVersion::V5;
                    RevocationInfo::Other { format, value }
                };
                revocation.push(info);
            }
        }
        let signers = fields
            .take()?
            .expect(SET)?
            .set_of_with_diagnostics(diagnostics)?
            .map(|value| SignerInfo::parse_with_diagnostics(value, diagnostics))
            .collect::<Result<Vec<_>>>()?;
        fields.finish()?;
        for signer in &signers {
            required = required.max(signer.version());
            if let Some(attributes) = &signer.signed_attributes {
                attributes.content_digest(content_type)?;
            } else if content_type != ID_DATA {
                return Err(element.error("non-data CMS content requires signed attributes"));
            }
        }
        if version != Some(required) {
            return Err(version_element.error("SignedData version does not match contents"));
        }
        Ok(Self {
            element,
            version: required,
            digest_algorithms,
            content_type,
            content,
            certificates,
            revocation,
            signers,
        })
    }
}

/// Reads one CertificateChoices value and the SignedData version it requires
/// (RFC 5652 5.1). A malformed X.509 schema is kept, not fatal; a budget
/// failure still aborts.
fn certificate_choice(choice: Element<'_>) -> Result<(CertificateChoice<'_>, CmsVersion)> {
    if choice.tag() == SEQUENCE {
        let certificate = match Certificate::parse(choice) {
            Ok(certificate) => CertificateChoice::X509(Box::new(certificate)),
            // Out-of-range values are Syntax errors, so a Limit is a budget.
            Err(error) if error.kind == ErrorKind::Limit => return Err(error),
            Err(error) => CertificateChoice::Malformed {
                element: choice,
                error,
            },
        };
        return Ok((certificate, CmsVersion::V1));
    }
    let tag = choice.tag();
    let version = match (tag.class, tag.form, tag.number) {
        (Class::Context, Form::Constructed, choice::EXTENDED_CERTIFICATE) => CmsVersion::V1,
        (Class::Context, Form::Constructed, choice::V1_ATTRIBUTE_CERTIFICATE) => CmsVersion::V3,
        (Class::Context, Form::Constructed, choice::V2_ATTRIBUTE_CERTIFICATE) => CmsVersion::V4,
        (Class::Context, Form::Constructed, choice::OTHER) => CmsVersion::V5,
        _ => return Err(choice.error("invalid CertificateChoices tag")),
    };
    Ok((CertificateChoice::Other(choice), version))
}
