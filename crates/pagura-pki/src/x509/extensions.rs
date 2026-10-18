//! Extension value views for subsequent RFC 5280 path validation.
//!
//! Call [`super::Extension::value_der`] first, then pass its root to the
//! appropriate parser. Unknown extensions and policy qualifiers stay inert.

use super::Name;
use crate::der::{Class, Element, Fields, SEQUENCE, Tag, single};
use crate::{BitString, Integer, ObjectIdentifier, types};
use pagura_core::Result;

/// OID contents for authorityKeyIdentifier (2.5.29.35).
pub const AUTHORITY_KEY_IDENTIFIER: &[u8] = &[85, 29, 35];
/// OID contents for subjectKeyIdentifier (2.5.29.14).
pub const SUBJECT_KEY_IDENTIFIER: &[u8] = &[85, 29, 14];
/// OID contents for basicConstraints (2.5.29.19).
pub const BASIC_CONSTRAINTS: &[u8] = &[85, 29, 19];
/// OID contents for keyUsage (2.5.29.15).
pub const KEY_USAGE: &[u8] = &[85, 29, 15];
/// OID contents for extendedKeyUsage (2.5.29.37).
pub const EXTENDED_KEY_USAGE: &[u8] = &[85, 29, 37];
/// OID contents for certificatePolicies (2.5.29.32).
pub const CERTIFICATE_POLICIES: &[u8] = &[85, 29, 32];
/// OID contents for policyMappings (2.5.29.33).
pub const POLICY_MAPPINGS: &[u8] = &[85, 29, 33];
/// OID contents for nameConstraints (2.5.29.30).
pub const NAME_CONSTRAINTS: &[u8] = &[85, 29, 30];
/// OID contents for policyConstraints (2.5.29.36).
pub const POLICY_CONSTRAINTS: &[u8] = &[85, 29, 36];
/// OID contents for inhibitAnyPolicy (2.5.29.54).
pub const INHIBIT_ANY_POLICY: &[u8] = &[85, 29, 54];
/// OID contents for subjectAltName (2.5.29.17).
pub const SUBJECT_ALT_NAME: &[u8] = &[85, 29, 17];
/// OID contents for cRLDistributionPoints (2.5.29.31).
pub const CRL_DISTRIBUTION_POINTS: &[u8] = &[85, 29, 31];
/// OID contents for authorityInfoAccess (1.3.6.1.5.5.7.1.1).
pub const AUTHORITY_INFO_ACCESS: &[u8] = &[43, 6, 1, 5, 5, 7, 1, 1];

/// Basic constraints (RFC 5280 §4.2.1.9).
#[derive(Debug, Clone, Copy)]
pub struct BasicConstraints<'a> {
    /// Whether the subject is asserted to be a CA.
    pub ca: bool,
    /// Nonnegative path length constraint, without a machine-width restriction.
    pub path_len: Option<Integer<'a>>,
}

impl<'a> BasicConstraints<'a> {
    /// Parses basicConstraints; a DEFAULT FALSE must be absent in DER.
    ///
    /// # Errors
    /// Rejects defaults, negative constraints and path lengths without cA TRUE.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let ca = if let Some(value) = fields.optional(Tag::universal(1, false)) {
            if !types::boolean(value)? {
                return Err(value.error("explicit basicConstraints DEFAULT FALSE"));
            }
            true
        } else {
            false
        };
        let path_len = fields
            .optional(Tag::universal(2, false))
            .map(nonnegative)
            .transpose()?;
        fields.finish()?;
        if !ca && path_len.is_some() {
            return Err(element.error("path length requires cA TRUE"));
        }
        Ok(Self { ca, path_len })
    }
}

/// Key usage named bits (RFC 5280 §4.2.1.3).
#[derive(Debug, Clone, Copy)]
pub struct KeyUsage<'a>(BitString<'a>);

impl<'a> KeyUsage<'a> {
    /// Parses keyUsage with DER named-bit truncation (X.690 §11.2.2).
    ///
    /// # Errors
    /// Rejects unknown usage bits, an empty usage, or redundant trailing zeros.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let bits = BitString::parse(element)?;
        let bytes = bits.as_bytes();
        let last = bytes
            .last()
            .ok_or_else(|| element.error("empty keyUsage"))?;
        if bytes.len() > 2
            || (bytes.len() == 2 && (*last != 128 || bits.unused_bits() != 7))
            || *last == 0
            || last.trailing_zeros() != u32::from(bits.unused_bits())
        {
            return Err(element.error("invalid keyUsage named bits"));
        }
        Ok(Self(bits))
    }
    /// Returns one of the nine named usage bits, numbered as in §4.2.1.3.
    pub fn permits(self, bit: usize) -> bool {
        bit < 9 && self.0.bit(bit)
    }
    /// Original bit representation.
    pub const fn bits(self) -> BitString<'a> {
        self.0
    }
}

/// Authority key identifier (RFC 5280 §4.2.1.1).
#[derive(Debug)]
pub struct AuthorityKeyIdentifier<'a> {
    /// Key identifier octets.
    pub key_identifier: Option<&'a [u8]>,
    /// Issuer names if supplied.
    pub issuer: Option<Vec<GeneralName<'a>>>,
    /// Serial paired with the issuer names.
    pub serial: Option<Integer<'a>>,
}

impl<'a> AuthorityKeyIdentifier<'a> {
    /// Parses the IMPLICIT key, issuer and serial fields.
    ///
    /// # Errors
    /// Rejects incorrect structure and an unpaired issuer or serial.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let key_identifier = fields
            .optional(Tag::context(0, false))
            .map(Element::content);
        let issuer = fields
            .optional(Tag::context(1, true))
            .map(general_names)
            .transpose()?;
        let serial = fields
            .optional(Tag::context(2, false))
            .map(|value| Integer::from_bytes(value.content(), value.content_offset()))
            .transpose()?;
        fields.finish()?;
        if issuer.is_some() != serial.is_some() {
            return Err(element.error("unpaired authority certificate issuer and serial"));
        }
        Ok(Self {
            key_identifier,
            issuer,
            serial,
        })
    }
}

/// A GeneralName, retaining choices not interpreted by this layer (§4.2.1.6).
#[derive(Debug, Clone, Copy)]
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
    /// Registered ID, encoded as an IMPLICIT OBJECT IDENTIFIER.
    RegisteredId(Element<'a>),
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
        match (tag.number, tag.constructed) {
            (1 | 2 | 6, false) => {
                if !element.content().is_ascii() {
                    return Err(element.error("non-IA5 GeneralName"));
                }
                Ok(match tag.number {
                    1 => Self::Mailbox(element.content()),
                    2 => Self::Dns(element.content()),
                    _ => Self::Uri(element.content()),
                })
            }
            (4, true) => Ok(Self::Directory(Name::parse(single(element)?)?)),
            (7, false) => Ok(Self::Ip(element.content())),
            (8, false) => {
                types::validate_primitive(
                    Tag::universal(6, false),
                    element.content(),
                    element.content_offset(),
                )?;
                Ok(Self::RegisteredId(element))
            }
            (0 | 3 | 5, true) => Ok(Self::Other(element)),
            _ => Err(element.error("invalid GeneralName choice")),
        }
    }
}

/// Reads GeneralNames, including an IMPLICIT wrapper in AKI or constraints.
///
/// # Errors
/// Rejects empty, primitive or malformed GeneralNames.
pub fn general_names(element: Element<'_>) -> Result<Vec<GeneralName<'_>>> {
    if !element.tag().constructed {
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

/// Parses a SEQUENCE OF object identifiers, e.g. extendedKeyUsage (§4.2.1.12).
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

/// A certificate policy and unmodified qualifiers (§4.2.1.4).
#[derive(Debug, Clone, Copy)]
pub struct PolicyInformation<'a> {
    /// Policy identifier.
    pub id: ObjectIdentifier<'a>,
    /// Optional qualifier SEQUENCE; no qualifier is implicitly accepted.
    pub qualifiers: Option<Element<'a>>,
}

/// Parses certificatePolicies; interpretation of qualifiers is policy work.
///
/// # Errors
/// Rejects empty policy lists and malformed policy/qualifier envelopes.
pub fn certificate_policies(element: Element<'_>) -> Result<Vec<PolicyInformation<'_>>> {
    element.expect(SEQUENCE)?;
    let mut policies = Vec::new();
    for value in element.children() {
        let mut fields = Fields::sequence(value)?;
        let id = ObjectIdentifier::parse(fields.take()?)?;
        let qualifiers = fields.optional(SEQUENCE);
        fields.finish()?;
        if let Some(qualifiers) = qualifiers {
            if qualifiers.children().next().is_none() {
                return Err(qualifiers.error("empty policy qualifiers"));
            }
            for qualifier in qualifiers.children() {
                let mut fields = Fields::sequence(qualifier)?;
                ObjectIdentifier::parse(fields.take()?)?;
                fields.take()?;
                fields.finish()?;
            }
        }
        policies.push(PolicyInformation { id, qualifiers });
    }
    if policies.is_empty() {
        return Err(element.error("empty certificatePolicies"));
    }
    Ok(policies)
}

/// Policy constraints (§4.2.1.11), retained without a machine-width limit.
#[derive(Debug, Clone, Copy)]
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
            .optional(Tag::context(0, false))
            .map(implicit_nonnegative)
            .transpose()?;
        let inhibit_policy_mapping = fields
            .optional(Tag::context(1, false))
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

/// GeneralSubtree with exact base and distance values (§4.2.1.10).
#[derive(Debug, Clone, Copy)]
pub struct GeneralSubtree<'a> {
    /// Name or address/mask constraint.
    pub base: GeneralName<'a>,
    /// Minimum distance, absent for DEFAULT zero.
    pub minimum: Option<Integer<'a>>,
    /// Maximum distance, if present.
    pub maximum: Option<Integer<'a>>,
}

/// Name constraints; accepting a supported name form is path-policy work.
#[derive(Debug)]
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
            .optional(Tag::context(0, true))
            .map(subtrees)
            .transpose()?
            .unwrap_or_default();
        let excluded = fields
            .optional(Tag::context(1, true))
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
            .optional(Tag::context(0, false))
            .map(implicit_nonnegative)
            .transpose()?;
        if minimum.is_some_and(Integer::is_zero) {
            return Err(value.error("explicit GeneralSubtree DEFAULT zero"));
        }
        let maximum = fields
            .optional(Tag::context(1, false))
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

/// Parses policyMappings as issuer/subject policy OID pairs (§4.2.1.5).
///
/// # Errors
/// Rejects empty mappings or pairs other than two OIDs.
pub fn policy_mappings(
    element: Element<'_>,
) -> Result<Vec<(ObjectIdentifier<'_>, ObjectIdentifier<'_>)>> {
    element.expect(SEQUENCE)?;
    let mut mappings = Vec::new();
    for mapping in element.children() {
        let mut fields = Fields::sequence(mapping)?;
        let issuer = ObjectIdentifier::parse(fields.take()?)?;
        let subject = ObjectIdentifier::parse(fields.take()?)?;
        fields.finish()?;
        mappings.push((issuer, subject));
    }
    if mappings.is_empty() {
        return Err(element.error("empty policyMappings"));
    }
    Ok(mappings)
}

/// Parses AIA method/location pairs (§4.2.2.1); locations are never followed.
///
/// # Errors
/// Rejects an empty list or malformed access descriptions.
pub fn access_descriptions(
    element: Element<'_>,
) -> Result<Vec<(ObjectIdentifier<'_>, GeneralName<'_>)>> {
    element.expect(SEQUENCE)?;
    let mut descriptions = Vec::new();
    for description in element.children() {
        let mut fields = Fields::sequence(description)?;
        let method = ObjectIdentifier::parse(fields.take()?)?;
        let location = GeneralName::parse(fields.take()?)?;
        fields.finish()?;
        descriptions.push((method, location));
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
