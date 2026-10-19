//! Extension value views for subsequent RFC 5280 path validation.
//!
//! Call [`super::Extension::value_der`] first, then pass its root to the
//! appropriate parser. Unknown extensions and policy qualifiers stay inert.

use super::{Name, optional_true};
use crate::der::{self, Class, Element, Fields, Form, SEQUENCE, Tag, single};
use crate::{BitString, Integer, ObjectIdentifier};
use pagura_core::Result;
use std::collections::BTreeSet;
use std::ops::BitOr;

/// authorityKeyIdentifier, 2.5.29.35.
pub const AUTHORITY_KEY_IDENTIFIER: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 35]);
/// subjectKeyIdentifier, 2.5.29.14.
pub const SUBJECT_KEY_IDENTIFIER: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 14]);
/// basicConstraints, 2.5.29.19.
pub const BASIC_CONSTRAINTS: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 19]);
/// keyUsage, 2.5.29.15.
pub const KEY_USAGE: ObjectIdentifier<'static> = ObjectIdentifier::from_static(&[85, 29, 15]);
/// extendedKeyUsage, 2.5.29.37.
pub const EXTENDED_KEY_USAGE: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 37]);
/// certificatePolicies, 2.5.29.32.
pub const CERTIFICATE_POLICIES: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 32]);
/// policyMappings, 2.5.29.33.
pub const POLICY_MAPPINGS: ObjectIdentifier<'static> = ObjectIdentifier::from_static(&[85, 29, 33]);
/// nameConstraints, 2.5.29.30.
pub const NAME_CONSTRAINTS: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 30]);
/// policyConstraints, 2.5.29.36.
pub const POLICY_CONSTRAINTS: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 36]);
/// inhibitAnyPolicy, 2.5.29.54.
pub const INHIBIT_ANY_POLICY: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 54]);
/// subjectAltName, 2.5.29.17.
pub const SUBJECT_ALT_NAME: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 17]);
/// cRLDistributionPoints, 2.5.29.31.
pub const CRL_DISTRIBUTION_POINTS: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 31]);
/// authorityInfoAccess, 1.3.6.1.5.5.7.1.1.
pub const AUTHORITY_INFO_ACCESS: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 1, 1]);
/// issuerAltName, 2.5.29.18.
pub const ISSUER_ALT_NAME: ObjectIdentifier<'static> = ObjectIdentifier::from_static(&[85, 29, 18]);
/// cRLNumber, a CRL extension, 2.5.29.20.
pub const CRL_NUMBER: ObjectIdentifier<'static> = ObjectIdentifier::from_static(&[85, 29, 20]);
/// deltaCRLIndicator, a CRL extension, 2.5.29.27.
pub const DELTA_CRL_INDICATOR: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 27]);
/// freshestCRL, 2.5.29.46.
pub const FRESHEST_CRL: ObjectIdentifier<'static> = ObjectIdentifier::from_static(&[85, 29, 46]);
/// issuingDistributionPoint, a CRL extension, 2.5.29.28.
pub const ISSUING_DISTRIBUTION_POINT: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 28]);
/// reasonCode, a CRL entry extension, 2.5.29.21.
pub const REASON_CODE: ObjectIdentifier<'static> = ObjectIdentifier::from_static(&[85, 29, 21]);
/// invalidityDate, a CRL entry extension, 2.5.29.24.
pub const INVALIDITY_DATE: ObjectIdentifier<'static> = ObjectIdentifier::from_static(&[85, 29, 24]);
/// certificateIssuer, a CRL entry extension, 2.5.29.29.
pub const CERTIFICATE_ISSUER: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 29]);
/// anyPolicy, 2.5.29.32.0 (RFC 5280 4.2.1.4).
pub const ANY_POLICY: ObjectIdentifier<'static> = ObjectIdentifier::from_static(&[85, 29, 32, 0]);
/// The CPS pointer policy qualifier, id-qt-cps, 1.3.6.1.5.5.7.2.1.
pub const QUALIFIER_CPS: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 2, 1]);
/// The user notice policy qualifier, id-qt-unotice, 1.3.6.1.5.5.7.2.2.
pub const QUALIFIER_USER_NOTICE: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 2, 2]);
/// anyExtendedKeyUsage, 2.5.29.37.0 (RFC 5280 4.2.1.12).
pub const ANY_EXTENDED_KEY_USAGE: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[85, 29, 37, 0]);
/// id-kp-serverAuth, 1.3.6.1.5.5.7.3.1.
pub const SERVER_AUTH: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 3, 1]);
/// id-kp-clientAuth, 1.3.6.1.5.5.7.3.2.
pub const CLIENT_AUTH: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 3, 2]);
/// id-kp-codeSigning, 1.3.6.1.5.5.7.3.3.
pub const CODE_SIGNING: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 3, 3]);
/// id-kp-emailProtection, 1.3.6.1.5.5.7.3.4.
pub const EMAIL_PROTECTION: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 3, 4]);
/// id-kp-timeStamping, 1.3.6.1.5.5.7.3.8.
pub const TIME_STAMPING: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 3, 8]);
/// id-kp-OCSPSigning, 1.3.6.1.5.5.7.3.9.
pub const OCSP_SIGNING: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 3, 9]);
/// id-on-SmtpUTF8Mailbox, an otherName type, 1.3.6.1.5.5.7.8.9 (RFC 9598 3).
pub const SMTP_UTF8_MAILBOX: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[43, 6, 1, 5, 5, 7, 8, 9]);

/// Basic constraints (RFC 5280 4.2.1.9).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct BasicConstraints<'a> {
    /// Whether the subject is asserted to be a CA.
    pub ca: bool,
    /// Nonnegative path length constraint, without a machine-width
    /// restriction; present only for a CA.
    pub path_len: Option<Integer<'a>>,
}

impl<'a> BasicConstraints<'a> {
    /// Parses basicConstraints; a DEFAULT FALSE must be absent in DER.
    ///
    /// # Errors
    /// Rejects defaults, negative constraints and path lengths without cA TRUE.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let ca = optional_true(&mut fields, "explicit basicConstraints DEFAULT FALSE")?;
        let path_len = fields.optional(der::INTEGER).map(nonnegative).transpose()?;
        fields.finish()?;
        if !ca && path_len.is_some() {
            return Err(element.error("path length requires cA TRUE"));
        }
        Ok(Self { ca, path_len })
    }
}

/// Key usage (RFC 5280 4.2.1.3): the set of named bits a certificate
/// asserts, or a set of usages to test for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct KeyUsage(u16);

impl KeyUsage {
    /// digitalSignature (0).
    pub const DIGITAL_SIGNATURE: Self = Self(1 << 0);
    /// contentCommitment (1), formerly nonRepudiation.
    pub const CONTENT_COMMITMENT: Self = Self(1 << 1);
    /// keyEncipherment (2).
    pub const KEY_ENCIPHERMENT: Self = Self(1 << 2);
    /// dataEncipherment (3).
    pub const DATA_ENCIPHERMENT: Self = Self(1 << 3);
    /// keyAgreement (4).
    pub const KEY_AGREEMENT: Self = Self(1 << 4);
    /// keyCertSign (5).
    pub const KEY_CERT_SIGN: Self = Self(1 << 5);
    /// cRLSign (6).
    pub const CRL_SIGN: Self = Self(1 << 6);
    /// encipherOnly (7).
    pub const ENCIPHER_ONLY: Self = Self(1 << 7);
    /// decipherOnly (8).
    pub const DECIPHER_ONLY: Self = Self(1 << 8);

    /// The number of named bits.
    const NAMED_BITS: usize = 9;

    /// Parses keyUsage with DER named-bit truncation (X.690 11.2.2).
    ///
    /// # Errors
    /// Rejects unknown usage bits, an empty usage, or redundant trailing zeros.
    pub fn parse(element: Element<'_>) -> Result<Self> {
        let bits = BitString::parse(element)?;
        let invalid = || element.error("invalid keyUsage named bits");
        let (&last, rest) = bits.as_bytes().split_last().ok_or_else(invalid)?;
        // Nine named bits fit two octets; the second can only hold decipherOnly.
        let fits = match rest {
            [] => true,
            [_] => last == 0x80,
            _ => false,
        };
        // X.690 11.2.2: trailing zero bits are removed, so the last bit is set.
        if !fits || last == 0 || last.trailing_zeros() != u32::from(bits.unused_bits()) {
            return Err(invalid());
        }
        let usage = (0..Self::NAMED_BITS)
            .filter(|&bit| bits.bit(bit))
            .fold(0, |usage, bit| usage | (1 << bit));
        Ok(Self(usage))
    }
    /// The usages as a mask, bit `n` holding named bit `n`.
    pub const fn bits(self) -> u16 {
        self.0
    }
    /// Whether every usage in `usages` is asserted.
    pub const fn contains(self, usages: Self) -> bool {
        self.0 & usages.0 == usages.0
    }
    /// Whether no usage is asserted.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
    /// The usages of both sets, usable in constants.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    /// Whether any usage in `usages` is asserted.
    pub const fn intersects(self, usages: Self) -> bool {
        self.0 & usages.0 != 0
    }
}

impl BitOr for KeyUsage {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

/// AuthorityKeyIdentifier's IMPLICIT fields (RFC 5280 4.2.1.1).
const KEY_IDENTIFIER: Tag = Tag::context(0, Form::Primitive);
const AUTHORITY_CERT_ISSUER: Tag = Tag::context(1, Form::Constructed);
const AUTHORITY_CERT_SERIAL: Tag = Tag::context(2, Form::Primitive);

/// Authority key identifier (RFC 5280 4.2.1.1).
#[derive(Debug)]
#[non_exhaustive]
pub struct AuthorityKeyIdentifier<'a> {
    /// Key identifier octets.
    pub key_identifier: Option<&'a [u8]>,
    /// The authority certificate's issuer and serial, if supplied.
    pub certificate: Option<AuthorityCertificate<'a>>,
}

/// The issuer names and serial number that identify the authority's
/// certificate; the schema requires both or neither.
#[derive(Debug)]
#[non_exhaustive]
pub struct AuthorityCertificate<'a> {
    /// The authority certificate's issuer names.
    pub issuer: Vec<GeneralName<'a>>,
    /// The authority certificate's serial number.
    pub serial: Integer<'a>,
}

impl<'a> AuthorityKeyIdentifier<'a> {
    /// Parses the IMPLICIT key, issuer and serial fields.
    ///
    /// # Errors
    /// Rejects incorrect structure and an unpaired issuer or serial.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let key_identifier = fields.optional(KEY_IDENTIFIER).map(Element::content);
        let issuer = fields
            .optional(AUTHORITY_CERT_ISSUER)
            .map(general_names)
            .transpose()?;
        let serial = fields
            .optional(AUTHORITY_CERT_SERIAL)
            .map(|value| Integer::from_bytes(value.content(), value.content_offset()))
            .transpose()?;
        fields.finish()?;
        let certificate = match (issuer, serial) {
            (Some(issuer), Some(serial)) => Some(AuthorityCertificate { issuer, serial }),
            (None, None) => None,
            _ => return Err(element.error("unpaired authority certificate issuer and serial")),
        };
        Ok(Self {
            key_identifier,
            certificate,
        })
    }
}

/// GeneralName choices (RFC 5280 4.2.1.6).
pub(crate) mod choice {
    pub(crate) const OTHER_NAME: u32 = 0;
    pub(crate) const RFC822_NAME: u32 = 1;
    pub(crate) const DNS_NAME: u32 = 2;
    pub(crate) const X400_ADDRESS: u32 = 3;
    pub(crate) const DIRECTORY_NAME: u32 = 4;
    pub(crate) const EDI_PARTY_NAME: u32 = 5;
    pub(crate) const URI: u32 = 6;
    pub(crate) const IP_ADDRESS: u32 = 7;
    pub(crate) const REGISTERED_ID: u32 = 8;
}

/// A GeneralName, retaining choices not interpreted by this layer
/// (RFC 5280 4.2.1.6).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum GeneralName<'a> {
    /// RFC 822 mailbox (IA5).
    Mailbox(&'a [u8]),
    /// DNS name (IA5).
    Dns(&'a [u8]),
    /// Distinguished name.
    Directory(Name<'a>),
    /// URI (IA5). It is never dereferenced.
    Uri(&'a [u8]),
    /// Address or name-constraint address/mask octets; context decides length.
    Ip(&'a [u8]),
    /// Registered ID, an IMPLICIT OBJECT IDENTIFIER.
    RegisteredId(ObjectIdentifier<'a>),
    /// otherName, x400Address or ediPartyName, retained for explicit policy.
    Other(Element<'a>),
}

impl<'a> GeneralName<'a> {
    /// Reads a GeneralName choice; no URI or directory lookup occurs.
    ///
    /// # Errors
    /// Rejects unknown choices, wrong forms and non-ASCII IA5 values.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let tag = element.tag();
        if tag.class != Class::Context {
            return Err(element.error("expected GeneralName context tag"));
        }
        match (tag.number, tag.form) {
            (choice::RFC822_NAME, Form::Primitive) => ia5(element).map(Self::Mailbox),
            (choice::DNS_NAME, Form::Primitive) => ia5(element).map(Self::Dns),
            (choice::URI, Form::Primitive) => ia5(element).map(Self::Uri),
            (choice::DIRECTORY_NAME, Form::Constructed) => {
                Ok(Self::Directory(Name::parse(single(element)?)?))
            }
            (choice::IP_ADDRESS, Form::Primitive) => Ok(Self::Ip(element.content())),
            (choice::REGISTERED_ID, Form::Primitive) => {
                ObjectIdentifier::from_implicit(element).map(Self::RegisteredId)
            }
            (
                choice::OTHER_NAME | choice::X400_ADDRESS | choice::EDI_PARTY_NAME,
                Form::Constructed,
            ) => Ok(Self::Other(element)),
            _ => Err(element.error("invalid GeneralName choice")),
        }
    }
}

fn ia5(element: Element<'_>) -> Result<&[u8]> {
    if element.content().is_ascii() {
        Ok(element.content())
    } else {
        Err(element.error("non-IA5 GeneralName"))
    }
}

/// Reads GeneralNames, including an IMPLICIT wrapper in AKI or constraints.
///
/// # Errors
/// Rejects empty, primitive or malformed GeneralNames.
pub fn general_names(element: Element<'_>) -> Result<Vec<GeneralName<'_>>> {
    if !element.tag().is_constructed() {
        return Err(element.error("primitive GeneralNames"));
    }
    let names = element
        .children()
        .map(GeneralName::parse)
        .collect::<Result<Vec<_>>>()?;
    if names.is_empty() {
        return Err(element.error("empty GeneralNames"));
    }
    Ok(names)
}

/// Parses a SEQUENCE OF object identifiers, e.g. extendedKeyUsage
/// (RFC 5280 4.2.1.12).
///
/// # Errors
/// Rejects empty sequences, wrong tags or invalid identifiers.
pub fn object_identifiers(element: Element<'_>) -> Result<Vec<ObjectIdentifier<'_>>> {
    element.expect(SEQUENCE)?;
    let values = element
        .children()
        .map(ObjectIdentifier::parse)
        .collect::<Result<Vec<_>>>()?;
    if values.is_empty() {
        return Err(element.error("empty OID sequence"));
    }
    Ok(values)
}

/// A certificate policy and unmodified qualifiers (RFC 5280 4.2.1.4).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct PolicyInformation<'a> {
    /// Policy identifier.
    pub id: ObjectIdentifier<'a>,
    /// Nonempty qualifier SEQUENCE, if present; no qualifier is implicitly
    /// accepted.
    pub qualifiers: Option<Element<'a>>,
}

/// Parses certificatePolicies; interpretation of qualifiers is policy work.
///
/// # Errors
/// Rejects empty policy lists, a repeated policy (RFC 5280 4.2.1.4) and
/// malformed policy/qualifier envelopes.
pub fn certificate_policies(element: Element<'_>) -> Result<Vec<PolicyInformation<'_>>> {
    element.expect(SEQUENCE)?;
    let mut seen = BTreeSet::new();
    let mut policies = Vec::new();
    for value in element.children() {
        let mut fields = Fields::sequence(value)?;
        let id = ObjectIdentifier::parse(fields.take()?)?;
        let qualifiers = fields.optional(SEQUENCE);
        fields.finish()?;
        if !seen.insert(id) {
            return Err(value.error("repeated certificate policy"));
        }
        if let Some(qualifiers) = qualifiers {
            let mut empty = true;
            for qualifier in qualifiers.children() {
                let mut fields = Fields::sequence(qualifier)?;
                ObjectIdentifier::parse(fields.take()?)?;
                fields.take()?;
                fields.finish()?;
                empty = false;
            }
            if empty {
                return Err(qualifiers.error("empty policy qualifiers"));
            }
        }
        policies.push(PolicyInformation { id, qualifiers });
    }
    if policies.is_empty() {
        return Err(element.error("empty certificatePolicies"));
    }
    Ok(policies)
}

/// PolicyConstraints' IMPLICIT counters (RFC 5280 4.2.1.11).
const REQUIRE_EXPLICIT_POLICY: Tag = Tag::context(0, Form::Primitive);
const INHIBIT_POLICY_MAPPING: Tag = Tag::context(1, Form::Primitive);

/// Policy constraints (RFC 5280 4.2.1.11), retained without a machine-width
/// limit. At least one counter is present.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct PolicyConstraints<'a> {
    /// Required explicit-policy skip count.
    pub require_explicit_policy: Option<Integer<'a>>,
    /// Inhibited mapping skip count.
    pub inhibit_policy_mapping: Option<Integer<'a>>,
}

impl<'a> PolicyConstraints<'a> {
    /// Parses the ordered optional IMPLICIT counters.
    ///
    /// # Errors
    /// Rejects negative counts, empty constraints and malformed structure.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let require_explicit_policy = fields
            .optional(REQUIRE_EXPLICIT_POLICY)
            .map(implicit_nonnegative)
            .transpose()?;
        let inhibit_policy_mapping = fields
            .optional(INHIBIT_POLICY_MAPPING)
            .map(implicit_nonnegative)
            .transpose()?;
        fields.finish()?;
        if require_explicit_policy.is_none() && inhibit_policy_mapping.is_none() {
            return Err(element.error("empty policyConstraints"));
        }
        Ok(Self {
            require_explicit_policy,
            inhibit_policy_mapping,
        })
    }
}

/// GeneralSubtree's IMPLICIT distances (RFC 5280 4.2.1.10).
const MINIMUM: Tag = Tag::context(0, Form::Primitive);
const MAXIMUM: Tag = Tag::context(1, Form::Primitive);

/// GeneralSubtree with exact base and distance values (RFC 5280 4.2.1.10).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct GeneralSubtree<'a> {
    /// Name or address/mask constraint.
    pub base: GeneralName<'a>,
    /// Minimum distance; `None` is the DEFAULT zero, so a present value is
    /// positive.
    pub minimum: Option<Integer<'a>>,
    /// Maximum distance, if present.
    pub maximum: Option<Integer<'a>>,
}

/// NameConstraints' IMPLICIT subtree lists (RFC 5280 4.2.1.10).
const PERMITTED_SUBTREES: Tag = Tag::context(0, Form::Constructed);
const EXCLUDED_SUBTREES: Tag = Tag::context(1, Form::Constructed);

/// Name constraints; accepting a supported name form is path-policy work.
/// At least one list is nonempty.
#[derive(Debug)]
#[non_exhaustive]
pub struct NameConstraints<'a> {
    /// Permitted subtrees, or an empty list when the field is absent.
    pub permitted: Vec<GeneralSubtree<'a>>,
    /// Excluded subtrees, or an empty list when the field is absent.
    pub excluded: Vec<GeneralSubtree<'a>>,
}

impl<'a> NameConstraints<'a> {
    /// Parses nameConstraints and the subtree distance encodings.
    ///
    /// # Errors
    /// Rejects empty constraints, negative distances and explicit default zero.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let permitted = fields
            .optional(PERMITTED_SUBTREES)
            .map(subtrees)
            .transpose()?
            .unwrap_or_default();
        let excluded = fields
            .optional(EXCLUDED_SUBTREES)
            .map(subtrees)
            .transpose()?
            .unwrap_or_default();
        fields.finish()?;
        if permitted.is_empty() && excluded.is_empty() {
            return Err(element.error("empty nameConstraints"));
        }
        Ok(Self {
            permitted,
            excluded,
        })
    }
}

fn subtrees(element: Element<'_>) -> Result<Vec<GeneralSubtree<'_>>> {
    let mut values = Vec::new();
    for value in element.children() {
        let mut fields = Fields::sequence(value)?;
        let base = GeneralName::parse(fields.take()?)?;
        let minimum = fields
            .optional(MINIMUM)
            .map(implicit_nonnegative)
            .transpose()?;
        if minimum.is_some_and(Integer::is_zero) {
            return Err(value.error("explicit GeneralSubtree DEFAULT zero"));
        }
        let maximum = fields
            .optional(MAXIMUM)
            .map(implicit_nonnegative)
            .transpose()?;
        fields.finish()?;
        values.push(GeneralSubtree {
            base,
            minimum,
            maximum,
        });
    }
    if values.is_empty() {
        return Err(element.error("empty GeneralSubtrees"));
    }
    Ok(values)
}

/// One policy mapping (RFC 5280 4.2.1.5).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct PolicyMapping<'a> {
    /// The issuing CA's policy.
    pub issuer_domain: ObjectIdentifier<'a>,
    /// The subject CA's policy it is considered equivalent to.
    pub subject_domain: ObjectIdentifier<'a>,
}

/// Parses policyMappings (RFC 5280 4.2.1.5).
///
/// # Errors
/// Rejects empty mappings or pairs other than two OIDs.
pub fn policy_mappings(element: Element<'_>) -> Result<Vec<PolicyMapping<'_>>> {
    element.expect(SEQUENCE)?;
    let mut mappings = Vec::new();
    for mapping in element.children() {
        let mut fields = Fields::sequence(mapping)?;
        let issuer_domain = ObjectIdentifier::parse(fields.take()?)?;
        let subject_domain = ObjectIdentifier::parse(fields.take()?)?;
        fields.finish()?;
        mappings.push(PolicyMapping {
            issuer_domain,
            subject_domain,
        });
    }
    if mappings.is_empty() {
        return Err(element.error("empty policyMappings"));
    }
    Ok(mappings)
}

/// One authority information access description (RFC 5280 4.2.2.1).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct AccessDescription<'a> {
    /// Access method, such as caIssuers or OCSP.
    pub method: ObjectIdentifier<'a>,
    /// Access location; it is data and never followed.
    pub location: GeneralName<'a>,
}

/// Parses authorityInfoAccess descriptions (RFC 5280 4.2.2.1).
///
/// # Errors
/// Rejects an empty list or malformed access descriptions.
pub fn access_descriptions(element: Element<'_>) -> Result<Vec<AccessDescription<'_>>> {
    element.expect(SEQUENCE)?;
    let mut descriptions = Vec::new();
    for description in element.children() {
        let mut fields = Fields::sequence(description)?;
        let method = ObjectIdentifier::parse(fields.take()?)?;
        let location = GeneralName::parse(fields.take()?)?;
        fields.finish()?;
        descriptions.push(AccessDescription { method, location });
    }
    if descriptions.is_empty() {
        return Err(element.error("empty authorityInfoAccess"));
    }
    Ok(descriptions)
}

pub(crate) fn nonnegative(element: Element<'_>) -> Result<Integer<'_>> {
    let integer = Integer::parse(element)?;
    integer.unsigned()?;
    Ok(integer)
}

pub(crate) fn implicit_nonnegative(element: Element<'_>) -> Result<Integer<'_>> {
    let integer = Integer::from_bytes(element.content(), element.content_offset())?;
    integer.unsigned()?;
    Ok(integer)
}
