//! RFC 3161 2.4.2 TSTInfo. These are asserted, unverified timestamp fields.

use crate::der::{self, Element, Fields, Form, SEQUENCE, Tag, single};
use crate::x509::extensions::{GeneralName, implicit_nonnegative, nonnegative};
use crate::x509::{AlgorithmIdentifier, Extension, optional_true, parse_extensions};
use crate::{Integer, ObjectIdentifier, Time, types};
use pagura_core::Result;

/// id-ct-TSTInfo, 1.2.840.113549.1.9.16.1.4.
pub const ID_TST_INFO: ObjectIdentifier<'static> =
    ObjectIdentifier::from_static(&[42, 134, 72, 134, 247, 13, 1, 9, 16, 1, 4]);

/// Accuracy's IMPLICIT fractions (RFC 3161 2.4.2).
const MILLIS: Tag = Tag::context(0, Form::Primitive);
const MICROS: Tag = Tag::context(1, Form::Primitive);
/// TSTInfo's `tsa [0]` (a tagged CHOICE, so explicit) and IMPLICIT
/// `extensions [1]` (RFC 3161 2.4.2).
const TSA: Tag = Tag::context(0, Form::Constructed);
const EXTENSIONS: Tag = Tag::context(1, Form::Constructed);
/// The only TSTInfo version, `v1` (RFC 3161 2.4.2).
const VERSION_1: u64 = 1;
/// Accuracy's millis and micros range (RFC 3161 2.4.2).
const FRACTION: std::ops::RangeInclusive<u16> = 1..=999;

/// A MessageImprint, used by timestamp structures (RFC 3161 2.4.1).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
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

/// Asserted timestamp accuracy, never a clock-trust claim.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Accuracy<'a> {
    /// Whole seconds; absent means zero.
    pub seconds: Option<Integer<'a>>,
    /// Milliseconds, 1–999 when present; absent means zero.
    pub millis: Option<u16>,
    /// Microseconds, 1–999 when present; absent means zero.
    pub micros: Option<u16>,
}

impl<'a> Accuracy<'a> {
    /// Parses the optional accuracy components.
    ///
    /// # Errors
    /// Rejects negative seconds, out-of-range fractions and malformed ordering.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let mut fields = Fields::sequence(element)?;
        let seconds = fields.optional(der::INTEGER).map(nonnegative).transpose()?;
        let millis = fields.optional(MILLIS).map(fraction).transpose()?;
        let micros = fields.optional(MICROS).map(fraction).transpose()?;
        fields.finish()?;
        Ok(Self {
            seconds,
            millis,
            micros,
        })
    }
}

fn fraction(element: Element<'_>) -> Result<u16> {
    let integer = implicit_nonnegative(element)?;
    integer
        .to_u64()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| FRACTION.contains(value))
        .ok_or_else(|| integer.error("timestamp accuracy fraction outside 1..999"))
}

/// Parsed timestamp information, without signature, imprint or trust validation.
#[derive(Debug)]
#[non_exhaustive]
pub struct TstInfo<'a> {
    /// TSA policy identifier.
    pub policy: ObjectIdentifier<'a>,
    /// Claimed digest of the timestamped data.
    pub message_imprint: MessageImprint<'a>,
    /// TSA serial number, retained at arbitrary size and sign.
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
    /// Rejects wrong versions, malformed fields, explicit defaults and
    /// noncanonical timestamp times or accuracy fractions.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.require_der()?;
        let mut fields = Fields::sequence(element)?;
        let version = fields.take()?;
        if Integer::parse(version)?.to_u64() != Some(VERSION_1) {
            return Err(version.error("unsupported TSTInfo version"));
        }
        let policy = ObjectIdentifier::parse(fields.take()?)?;
        let message_imprint = MessageImprint::parse(fields.take()?)?;
        // RFC 3161 2.4.2 restricts neither the serial number's nor the nonce's sign.
        let serial = Integer::parse(fields.take()?)?;
        let gen_time = Time::generalized(fields.take()?)?;
        let accuracy = fields.optional(SEQUENCE).map(Accuracy::parse).transpose()?;
        let ordering = optional_true(&mut fields, "explicit timestamp DEFAULT FALSE")?;
        let nonce = fields
            .optional(der::INTEGER)
            .map(Integer::parse)
            .transpose()?;
        let tsa = fields
            .optional(TSA)
            .map(|value| GeneralName::parse(single(value)?))
            .transpose()?;
        let extensions = fields
            .optional(EXTENSIONS)
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
