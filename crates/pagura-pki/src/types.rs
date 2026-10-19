//! Borrowed primitive values (X.690 8 and 10-11).

mod time;

pub use time::Time;
pub(crate) use time::TimeSyntax;

use crate::der::{self, Class, Element, Fields, Form, Tag, universal};
use pagura_core::{Error, Position, Result};
use std::hash::{Hash, Hasher};

/// Bit 8 of an OBJECT IDENTIFIER subidentifier octet: more octets follow
/// (X.690 8.19.2).
const CONTINUATION: u8 = 0x80;
/// The sign bit of an INTEGER's first content octet (X.690 8.3.3).
const SIGN: u8 = 0x80;
/// The largest unused-bit count of a BIT STRING (X.690 8.6.2.2).
const MAX_UNUSED_BITS: u8 = 7;
const BOOLEAN_FALSE: u8 = 0x00;
/// DER's only TRUE encoding (X.690 11.1).
const BOOLEAN_TRUE: u8 = 0xff;

/// DER ECDSA signature envelope (RFC 3279 2.2.3), without curve arithmetic.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct EcdsaSignature<'a> {
    /// Nonnegative r; its range must still be checked against the curve.
    pub r: Integer<'a>,
    /// Nonnegative s; its range must still be checked against the curve.
    pub s: Integer<'a>,
}

impl<'a> EcdsaSignature<'a> {
    /// Reads exactly two nonnegative DER INTEGERs in a SEQUENCE.
    ///
    /// # Errors
    /// Rejects BER encodings, incorrect tags, negative integers and extra fields.
    /// Zero and values above a curve's order remain representable unverified data.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.require_der()?;
        let mut fields = Fields::sequence(element)?;
        let r = Integer::parse(fields.take()?)?;
        let s = Integer::parse(fields.take()?)?;
        fields.finish()?;
        r.unsigned()?;
        s.unsigned()?;
        Ok(Self { r, s })
    }
}

/// An arbitrary-size, minimal, two's-complement INTEGER.
#[derive(Debug, Clone, Copy)]
pub struct Integer<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl PartialEq for Integer<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for Integer<'_> {}

impl Hash for Integer<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

impl<'a> Integer<'a> {
    /// Reads an INTEGER without converting it to a machine integer.
    ///
    /// # Errors
    /// Rejects a wrong tag or nonminimal/empty contents (X.690 8.3).
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.expect(der::INTEGER)?;
        Self::from_bytes(element.content(), element.content_offset())
    }
    /// Reads the content of a schema's IMPLICIT INTEGER found at `offset`.
    ///
    /// # Errors
    /// Rejects empty or nonminimal two's-complement contents.
    pub fn from_bytes(bytes: &'a [u8], offset: usize) -> Result<Self> {
        let (&first, rest) = bytes
            .split_first()
            .ok_or_else(|| syntax(offset, "empty ASN.1 INTEGER"))?;
        // X.690 8.3.2: the first nine bits are neither all ones nor all zeros.
        if rest.first().is_some_and(|&second| {
            (first == 0 && second & SIGN == 0) || (first == u8::MAX && second & SIGN != 0)
        }) {
            return Err(syntax(offset, "nonminimal ASN.1 INTEGER"));
        }
        Ok(Self { bytes, offset })
    }
    /// Original big-endian two's-complement octets, including any sign octet.
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
    /// Whether the value is negative.
    pub fn is_negative(self) -> bool {
        self.bytes.first().is_some_and(|value| value & SIGN != 0)
    }
    /// Whether the value is zero.
    pub fn is_zero(self) -> bool {
        self.bytes == [0]
    }
    /// Unsigned magnitude, removing a positive sign octet where present.
    ///
    /// # Errors
    /// Rejects negative values.
    pub fn unsigned(self) -> Result<&'a [u8]> {
        if self.is_negative() {
            return Err(syntax(
                self.offset,
                "negative ASN.1 integer where unsigned required",
            ));
        }
        Ok(match self.bytes {
            [0, magnitude @ ..] if !magnitude.is_empty() => magnitude,
            bytes => bytes,
        })
    }
    /// The value as a `u64`, or `None` when it is negative or wider. A schema
    /// that bounds the value reports `None` as malformed input.
    pub fn to_u64(self) -> Option<u64> {
        let magnitude = self.unsigned().ok()?;
        if magnitude.len() > size_of::<u64>() {
            return None;
        }
        Some(
            magnitude
                .iter()
                .fold(0, |value, &byte| (value << 8) | u64::from(byte)),
        )
    }
    /// A syntax error at the value, for a schema rejecting it.
    pub(crate) fn error(self, message: &'static str) -> Error {
        syntax(self.offset, message)
    }
}

/// A canonical OBJECT IDENTIFIER, kept as base-128 subidentifier octets.
///
/// Equality compares these canonical bytes and needs no machine-sized arcs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectIdentifier<'a> {
    bytes: &'a [u8],
}

impl<'a> ObjectIdentifier<'a> {
    /// Reads an OBJECT IDENTIFIER (X.690 8.19).
    ///
    /// # Errors
    /// Rejects a wrong tag, empty identifier, nonminimal arc or truncated arc.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.expect(der::OBJECT_IDENTIFIER)?;
        Self::from_implicit(element)
    }
    /// Reads the content of a schema's IMPLICIT OBJECT IDENTIFIER.
    pub(crate) fn from_implicit(element: Element<'a>) -> Result<Self> {
        validate_oid(element.content(), element.content_offset())?;
        Ok(Self {
            bytes: element.content(),
        })
    }
    /// Encoded contents, without the tag or length.
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
}

impl ObjectIdentifier<'static> {
    /// An identifier from canonical content octets fixed in the program, for
    /// named constants such as `x509::extensions::KEY_USAGE`.
    ///
    /// # Panics
    /// Panics if `content` is not a canonical identifier; in a constant that
    /// is a compile-time error.
    pub const fn from_static(content: &'static [u8]) -> Self {
        assert!(
            oid_fault(content).is_none(),
            "invalid OBJECT IDENTIFIER content"
        );
        Self { bytes: content }
    }
}

/// A BIT STRING with zero padding validated (X.690 8.6 and 11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BitString<'a> {
    bytes: &'a [u8],
    unused: u8,
}

impl<'a> BitString<'a> {
    /// Reads a primitive BIT STRING.
    ///
    /// # Errors
    /// Rejects a wrong tag, invalid unused-bit count or nonzero padding.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.expect(der::BIT_STRING)?;
        Self::from_bytes(element.content(), element.content_offset())
    }
    /// Reads content octets of a schema's IMPLICIT BIT STRING found at `offset`.
    ///
    /// # Errors
    /// Rejects invalid counts, missing contents and nonzero unused bits.
    pub fn from_bytes(content: &'a [u8], offset: usize) -> Result<Self> {
        let (&unused, bytes) = content
            .split_first()
            .ok_or_else(|| syntax(offset, "missing BIT STRING padding count"))?;
        if unused > MAX_UNUSED_BITS
            || (bytes.is_empty() && unused != 0)
            || bytes
                .last()
                .is_some_and(|last| last & ((1u8 << unused) - 1) != 0)
        {
            return Err(syntax(offset, "invalid BIT STRING padding"));
        }
        Ok(Self { bytes, unused })
    }
    /// Data octets, excluding the unused-bit count.
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
    /// Number of unused low bits in the final octet.
    pub const fn unused_bits(self) -> u8 {
        self.unused
    }
    /// Whether a bit (numbered from the most significant bit) is set.
    pub fn bit(self, index: usize) -> bool {
        self.bytes
            .get(index / 8)
            .is_some_and(|byte| byte & (0x80 >> (index % 8)) != 0)
    }
}

/// A typed ASN.1 string; legacy repertoires remain encoded data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringValue<'a> {
    tag: Tag,
    bytes: &'a [u8],
}

impl<'a> StringValue<'a> {
    /// Reads a supported ASN.1 character string, validating its repertoire.
    ///
    /// # Errors
    /// Rejects unsupported string tags, bad Unicode and repertoire violations.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let tag = element.tag();
        if tag.class != Class::Universal
            || tag.form != Form::Primitive
            || !universal::is_character_string(tag.number)
        {
            return Err(element.error("expected ASN.1 character string"));
        }
        validate_string(tag.number, element.content(), element.content_offset())?;
        Ok(Self {
            tag,
            bytes: element.content(),
        })
    }
    /// Original universal string identifier.
    pub const fn tag(self) -> Tag {
        self.tag
    }
    /// Original content; Teletex, Videotex, Graphic and General strings are
    /// deliberately not guessed to be UTF-8 or Latin-1.
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
}

/// Reads a canonical BOOLEAN (X.690 11.1).
///
/// # Errors
/// Rejects a wrong identifier or any content other than one 00 or FF octet.
pub fn boolean(element: Element<'_>) -> Result<bool> {
    element.expect(der::BOOLEAN)?;
    match element.content() {
        [BOOLEAN_FALSE] => Ok(false),
        [BOOLEAN_TRUE] => Ok(true),
        _ => Err(element.error("noncanonical ASN.1 BOOLEAN")),
    }
}

/// Reads a primitive OCTET STRING. Constructed BER octets are exposed by
/// [`crate::cms::OctetString`] instead.
///
/// # Errors
/// Rejects a wrong tag or constructed form.
pub fn octets(element: Element<'_>) -> Result<&[u8]> {
    element.expect(der::OCTET_STRING)?;
    Ok(element.content())
}

pub(crate) fn syntax(offset: usize, message: &'static str) -> Error {
    Error::syntax(message).at(Position::Buffer(offset))
}

/// Checks a universal primitive's content against DER (X.690 8, 10-11).
pub(crate) fn validate_primitive(tag: Tag, bytes: &[u8], offset: usize) -> Result<()> {
    if tag.class != Class::Universal {
        return Ok(());
    }
    match tag.number {
        universal::BOOLEAN => {
            if matches!(bytes, [BOOLEAN_FALSE | BOOLEAN_TRUE]) {
                Ok(())
            } else {
                Err(syntax(offset, "noncanonical ASN.1 BOOLEAN"))
            }
        }
        universal::INTEGER | universal::ENUMERATED => {
            Integer::from_bytes(bytes, offset).map(|_| ())
        }
        universal::BIT_STRING => BitString::from_bytes(bytes, offset).map(|_| ()),
        universal::OCTET_STRING => Ok(()),
        universal::NULL => {
            if bytes.is_empty() {
                Ok(())
            } else {
                Err(syntax(offset, "nonempty ASN.1 NULL"))
            }
        }
        universal::OBJECT_IDENTIFIER | universal::RELATIVE_OID => validate_oid(bytes, offset),
        number if universal::is_character_string(number) => validate_string(number, bytes, offset),
        universal::UTC_TIME => Time::read(bytes, offset, TimeSyntax::Utc).map(|_| ()),
        universal::GENERALIZED_TIME => {
            Time::read(bytes, offset, TimeSyntax::Fractional).map(|_| ())
        }
        universal::END_OF_CONTENTS | universal::SEQUENCE | universal::SET => {
            Err(syntax(offset, "invalid primitive universal tag"))
        }
        _ => Err(Error::unsupported(
            crate::features::UNIVERSAL_TYPE,
            "unsupported universal ASN.1 type",
        )
        .at(Position::Buffer(offset))),
    }
}

/// The first fault of OBJECT IDENTIFIER content octets (X.690 8.19.2), as
/// its offset and message.
const fn oid_fault(bytes: &[u8]) -> Option<(usize, &'static str)> {
    let Some(&last) = bytes.last() else {
        return Some((0, "empty OBJECT IDENTIFIER"));
    };
    let mut index = 0;
    let mut starts_arc = true;
    while index < bytes.len() {
        let byte = bytes[index];
        if starts_arc && byte == CONTINUATION {
            return Some((index, "nonminimal OBJECT IDENTIFIER arc"));
        }
        starts_arc = byte & CONTINUATION == 0;
        index += 1;
    }
    if last & CONTINUATION != 0 {
        return Some((bytes.len() - 1, "unterminated OBJECT IDENTIFIER arc"));
    }
    None
}

fn validate_oid(bytes: &[u8], offset: usize) -> Result<()> {
    match oid_fault(bytes) {
        Some((index, message)) => Err(syntax(offset + index, message)),
        None => Ok(()),
    }
}

/// Checks a character string's repertoire (X.680 41, 43).
fn validate_string(number: u32, bytes: &[u8], offset: usize) -> Result<()> {
    let valid = match number {
        universal::UTF8_STRING => std::str::from_utf8(bytes).is_ok(),
        universal::NUMERIC_STRING => bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b' '),
        universal::PRINTABLE_STRING => bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b" '()+,-./:=?".contains(byte)),
        universal::IA5_STRING => bytes.is_ascii(),
        universal::VISIBLE_STRING => bytes.iter().all(|byte| (b' '..=b'~').contains(byte)),
        universal::UNIVERSAL_STRING => {
            let (chunks, rest) = bytes.as_chunks::<4>();
            rest.is_empty()
                && chunks
                    .iter()
                    .all(|chunk| char::from_u32(u32::from_be_bytes(*chunk)).is_some())
        }
        universal::BMP_STRING => {
            let (chunks, rest) = bytes.as_chunks::<2>();
            rest.is_empty()
                && chunks
                    .iter()
                    .all(|chunk| char::from_u32(u32::from(u16::from_be_bytes(*chunk))).is_some())
        }
        // Their registered legacy repertoires are preserved, never decoded by guesswork.
        universal::TELETEX_STRING
        | universal::VIDEOTEX_STRING
        | universal::GRAPHIC_STRING
        | universal::GENERAL_STRING => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(syntax(offset, "invalid ASN.1 character string"))
    }
}
