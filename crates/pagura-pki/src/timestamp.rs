//! RFC 3161 §2.4.2 TSTInfo. These are asserted, unverified timestamp fields.

use crate::der::{Element, Fields, SEQUENCE, Tag, single};
use crate::x509::AlgorithmIdentifier;
use crate::x509::extensions::{GeneralName, implicit_nonnegative, nonnegative};
use crate::x509::{Extension, parse_extensions};
use crate::{Integer, ObjectIdentifier, Time, types};
use pagura_core::Result;

/// A MessageImprint, used by timestamp structures (RFC 3161 §2.4.1).
#[derive(Debug, Clone, Copy)]
pub struct MessageImprint<'a> {
    /// Digest algorithm and parameters.
    pub algorithm: AlgorithmIdentifier<'a>,
    /// Claimed digest octets; no algorithm or length is implicitly accepted.
    pub digest: &'a [u8],
}

impl<'a> MessageImprint<'a> {
    /// Parses the algorithm and hashedMessage pair.
    ///
    /// # Errors
    /// Rejects malformed structure and nonprimitive hashedMessage values.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let algorithm = AlgorithmIdentifier::parse(fields.take()?)?;
        let digest = types::octets(fields.take()?)?;
        fields.finish()?;
        Ok(Self { algorithm, digest })
    }
}

/// id-ct-TSTInfo OID contents, 1.2.840.113549.1.9.16.1.4.
pub const ID_TST_INFO: &[u8] = &[42, 134, 72, 134, 247, 13, 1, 9, 16, 1, 4];

/// Asserted timestamp accuracy, never a clock-trust claim.
#[derive(Debug, Clone, Copy)]
pub struct Accuracy<'a> {
    /// Whole seconds; absent means zero.
    pub seconds: Option<Integer<'a>>,
    /// Milliseconds, 1–999 when present.
    pub millis: Option<u16>,
    /// Microseconds, 1–999 when present.
    pub micros: Option<u16>,
}

impl<'a> Accuracy<'a> {
    /// Parses the optional accuracy components.
    ///
    /// # Errors
    /// Rejects negative seconds, out-of-range fractions and malformed ordering.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let seconds = fields
            .optional(Tag::universal(2, false))
            .map(nonnegative)
            .transpose()?;
        let millis = fields
            .optional(Tag::context(0, false))
            .map(fraction)
            .transpose()?;
        let micros = fields
            .optional(Tag::context(1, false))
            .map(fraction)
            .transpose()?;
        fields.finish()?;
        Ok(Self {
            seconds,
            millis,
            micros,
        })
    }
}

fn fraction(element: Element<'_>) -> Result<u16> {
    let value = implicit_nonnegative(element)?.to_u64()?;
    if !(1..=999).contains(&value) {
        return Err(element.error("timestamp accuracy fraction outside 1..999"));
    }
    u16::try_from(value).map_err(|_| element.error("timestamp accuracy fraction exceeds u16"))
}

/// Parsed timestamp information, without signature, imprint or trust validation.
#[derive(Debug)]
pub struct TstInfo<'a> {
    /// TSA policy identifier.
    pub policy: ObjectIdentifier<'a>,
    /// Claimed digest of the timestamped data.
    pub message_imprint: MessageImprint<'a>,
    /// TSA serial number, retained at arbitrary size.
    pub serial: Integer<'a>,
    /// Asserted generation time with exact decimal fractions.
    pub gen_time: Time<'a>,
    /// Optional asserted accuracy.
    pub accuracy: Option<Accuracy<'a>>,
    /// Claimed ordering flag, false when absent.
    pub ordering: bool,
    /// Optional request nonce, not compared with a request by this parser.
    pub nonce: Option<Integer<'a>>,
    /// Optional TSA name, not authenticated by its presence.
    pub tsa: Option<GeneralName<'a>>,
    /// Extensions, including unknown critical extensions.
    pub extensions: Vec<Extension<'a>>,
    /// Original DER TSTInfo SEQUENCE.
    pub element: Element<'a>,
}

impl<'a> TstInfo<'a> {
    /// Parses version 1 TSTInfo with optional fields in schema order.
    ///
    /// # Errors
    /// Rejects wrong versions, malformed fields, explicit defaults, negative
    /// serials/nonces and noncanonical timestamp times or accuracy fractions.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        if element.is_ber() {
            crate::der::Document::parse_at(
                element.encoded(),
                element.range().start,
                element.limits(),
            )?;
        }
        let mut fields = Fields::sequence(element)?;
        let version = fields.take()?;
        if Integer::parse(version)?.to_u64()? != 1 {
            return Err(version.error("unsupported TSTInfo version"));
        }
        let policy = ObjectIdentifier::parse(fields.take()?)?;
        let message_imprint = MessageImprint::parse(fields.take()?)?;
        let serial = nonnegative(fields.take()?)?;
        let gen_time = Time::generalized(fields.take()?)?;
        let accuracy = fields.optional(SEQUENCE).map(Accuracy::parse).transpose()?;
        let ordering = if let Some(value) = fields.optional(Tag::universal(1, false)) {
            if !types::boolean(value)? {
                return Err(value.error("explicit timestamp DEFAULT FALSE"));
            }
            true
        } else {
            false
        };
        let nonce = fields
            .optional(Tag::universal(2, false))
            .map(nonnegative)
            .transpose()?;
        let tsa = fields
            .optional(Tag::context(0, true))
            .map(|value| GeneralName::parse(single(value)?))
            .transpose()?;
        let extensions = fields
            .optional(Tag::context(1, true))
            .map(parse_extensions)
            .transpose()?
            .unwrap_or_default();
        fields.finish()?;
        Ok(Self {
            policy,
            message_imprint,
            serial,
            gen_time,
            accuracy,
            ordering,
            nonce,
            tsa,
            extensions,
            element,
        })
    }
}
