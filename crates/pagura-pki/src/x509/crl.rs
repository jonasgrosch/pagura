//! CertificateList (RFC 5280 §5.1), retaining original signed bytes.

use super::{AlgorithmIdentifier, Extension, Name, parse_extensions};
use crate::der::{Document, Element, Fields, SEQUENCE, Tag, single};
use crate::{BitString, Integer, Time};
use pagura_core::Result;

/// One unverified CRL entry (§5.1.2.6).
#[derive(Debug)]
pub struct RevokedCertificate<'a> {
    /// Claimed revoked serial number.
    pub serial: Integer<'a>,
    /// Claimed revocation time.
    pub date: Time<'a>,
    /// Entry extensions, including unknown critical values.
    pub extensions: Vec<Extension<'a>>,
}

/// A parsed CRL; neither its signature nor its freshness has been checked.
#[derive(Debug)]
pub struct CertificateList<'a> {
    /// Original CertificateList SEQUENCE.
    pub element: Element<'a>,
    /// Exact signed TBSCertList, including its SEQUENCE header.
    pub tbs: Element<'a>,
    /// Version, expressed as 1 or 2.
    pub version: u8,
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
    /// CRL extensions, including unknown critical values.
    pub extensions: Vec<Extension<'a>>,
    /// Unverified CRL signature bits.
    pub signature: BitString<'a>,
}

impl<'a> CertificateList<'a> {
    /// Reads a DER CertificateList (RFC 5280 §5.1).
    ///
    /// # Errors
    /// Rejects malformed fields, non-DER encodings, mismatched algorithms,
    /// unsupported versions and extensions in v1 CRLs.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        if element.is_ber() {
            Document::parse_at(element.encoded(), element.range().start, element.limits())?;
        }
        let mut outer = Fields::sequence(element)?;
        let tbs = outer.take()?.expect(SEQUENCE)?;
        let signature_algorithm = AlgorithmIdentifier::parse(outer.take()?)?;
        let signature = BitString::parse(outer.take()?)?;
        outer.finish()?;
        let mut fields = Fields::sequence(tbs)?;
        let version = if let Some(value) = fields.optional(Tag::universal(2, false)) {
            if Integer::parse(value)?.to_u64()? != 1 {
                return Err(value.error("invalid CRL version"));
            }
            2
        } else {
            1
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
            .optional(Tag::universal(23, false))
            .or_else(|| fields.optional(Tag::universal(24, false)))
            .map(Time::parse)
            .transpose()?;
        let mut revoked = Vec::new();
        if let Some(sequence) = fields.optional(SEQUENCE) {
            for entry in sequence.children() {
                let mut fields = Fields::sequence(entry)?;
                let serial = Integer::parse(fields.take()?)?;
                let date = Time::parse(fields.take()?)?;
                let extensions = fields
                    .optional(SEQUENCE)
                    .map(parse_extensions)
                    .transpose()?
                    .unwrap_or_default();
                if version == 1 && !extensions.is_empty() {
                    return Err(entry.error("CRL entry extensions require v2"));
                }
                fields.finish()?;
                revoked.push(RevokedCertificate {
                    serial,
                    date,
                    extensions,
                });
            }
        }
        let extensions = if let Some(explicit) = fields.optional(Tag::context(0, true)) {
            if version != 2 {
                return Err(explicit.error("CRL extensions require v2"));
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
