//! CertificateList (RFC 5280 5.1), retaining original signed bytes.

use super::{AlgorithmIdentifier, Extension, Name, parse_extensions};
use crate::der::{self, Element, Fields, Form, SEQUENCE, Tag, single};
use crate::{BitString, Integer, Time};
use pagura_core::Result;

/// TBSCertList's `crlExtensions [0] EXPLICIT` (RFC 5280 5.1).
const CRL_EXTENSIONS: Tag = Tag::context(0, Form::Constructed);

/// CRL version (RFC 5280 5.1.2.1). The standard defines exactly two, so the
/// enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CrlVersion {
    /// Version 1, encoded by omitting the field.
    V1,
    /// Version 2, encoded as the INTEGER `v2(1)`; it admits extensions.
    V2,
}

/// One unverified CRL entry (RFC 5280 5.1.2.6).
#[derive(Debug)]
#[non_exhaustive]
pub struct RevokedCertificate<'a> {
    /// Claimed revoked serial number.
    pub serial: Integer<'a>,
    /// Claimed revocation time.
    pub date: Time<'a>,
    /// Entry extensions, including unknown critical values; empty in v1.
    pub extensions: Vec<Extension<'a>>,
}

/// A parsed CRL; neither its signature nor its freshness has been checked.
#[derive(Debug)]
#[non_exhaustive]
pub struct CertificateList<'a> {
    /// Original CertificateList SEQUENCE.
    pub element: Element<'a>,
    /// Exact signed TBSCertList, including its SEQUENCE header.
    pub tbs: Element<'a>,
    /// CRL version.
    pub version: CrlVersion,
    /// Signature algorithm, identical in the outer and TBS envelopes.
    pub signature_algorithm: AlgorithmIdentifier<'a>,
    /// Issuer distinguished name.
    pub issuer: Name<'a>,
    /// Asserted issue time.
    pub this_update: Time<'a>,
    /// Asserted next issue time, if present.
    pub next_update: Option<Time<'a>>,
    /// Claimed revoked certificates.
    pub revoked: Vec<RevokedCertificate<'a>>,
    /// CRL extensions, including unknown critical values; empty in v1.
    pub extensions: Vec<Extension<'a>>,
    /// Unverified CRL signature bits.
    pub signature: BitString<'a>,
}

impl<'a> CertificateList<'a> {
    /// Reads a DER CertificateList (RFC 5280 5.1).
    ///
    /// # Errors
    /// Rejects malformed fields, non-DER encodings, mismatched algorithms,
    /// unsupported versions and extensions in v1 CRLs.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.require_der()?;
        let mut outer = Fields::sequence(element)?;
        let tbs = outer.take()?.expect(SEQUENCE)?;
        let signature_algorithm = AlgorithmIdentifier::parse(outer.take()?)?;
        let signature = BitString::parse(outer.take()?)?;
        outer.finish()?;
        let mut fields = Fields::sequence(tbs)?;
        let version = match fields.optional(der::INTEGER) {
            Some(value) if Integer::parse(value)?.to_u64() == Some(1) => CrlVersion::V2,
            Some(value) => return Err(value.error("invalid CRL version")),
            None => CrlVersion::V1,
        };
        let inner_algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        if inner_algorithm.element.encoded() != signature_algorithm.element.encoded() {
            return Err(signature_algorithm
                .element
                .error("CRL signature algorithms differ"));
        }
        let issuer = Name::parse(fields.take()?)?;
        let this_update = Time::parse(fields.take()?)?;
        let next_update = fields
            .optional(der::UTC_TIME)
            .or_else(|| fields.optional(der::GENERALIZED_TIME))
            .map(Time::parse)
            .transpose()?;
        let mut revoked = Vec::new();
        if let Some(sequence) = fields.optional(SEQUENCE) {
            for entry in sequence.children() {
                revoked.push(revoked_certificate(entry, version)?);
            }
        }
        let extensions = match fields.optional(CRL_EXTENSIONS) {
            Some(explicit) if version == CrlVersion::V2 => {
                parse_extensions(single(explicit)?.expect(SEQUENCE)?)?
            }
            Some(explicit) => return Err(explicit.error("CRL extensions require v2")),
            None => Vec::new(),
        };
        fields.finish()?;
        Ok(Self {
            element,
            tbs,
            version,
            signature_algorithm,
            issuer,
            this_update,
            next_update,
            revoked,
            extensions,
            signature,
        })
    }
}

fn revoked_certificate(entry: Element<'_>, version: CrlVersion) -> Result<RevokedCertificate<'_>> {
    let mut fields = Fields::sequence(entry)?;
    let serial = Integer::parse(fields.take()?)?;
    let date = Time::parse(fields.take()?)?;
    let extensions = match fields.optional(SEQUENCE) {
        Some(_) if version == CrlVersion::V1 => {
            return Err(entry.error("CRL entry extensions require v2"));
        }
        Some(extensions) => parse_extensions(extensions)?,
        None => Vec::new(),
    };
    fields.finish()?;
    Ok(RevokedCertificate {
        serial,
        date,
        extensions,
    })
}
