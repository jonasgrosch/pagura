//! Borrowed primitive values (X.690 §8 and §§10–11).

use crate::der::{Class, Element, Tag};
use pagura_core::{Error, Position, Result};
use std::hash::{Hash, Hasher};
mod time;
pub use time::Time;

/// DER ECDSA signature envelope (RFC 3279 §2.2.3), without curve arithmetic.
#[derive(Debug, Clone, Copy)]
pub struct EcdsaSignature<'a> {
    /// Nonnegative r; its mathematical range must be checked against the curve.
    pub r: Integer<'a>,
    /// Nonnegative s; its mathematical range must be checked against the curve.
    pub s: Integer<'a>,
}

impl<'a> EcdsaSignature<'a> {
    /// Reads exactly two nonnegative DER INTEGERs in a SEQUENCE.
    ///
    /// # Errors
    /// Rejects BER encodings, incorrect tags, negative integers and extra fields.
    /// Zero and values above a curve's order remain representable unverified data.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        if element.is_ber() {
            crate::der::Document::parse_at(
                element.encoded(),
                element.range().start,
                element.limits(),
            )?;
        }
        let mut fields = crate::der::Fields::sequence(element)?;
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
    /// Rejects a wrong tag or nonminimal/empty contents (X.690 §8.3).
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.expect(Tag::universal(2, false))?;
        Self::from_bytes(element.content(), element.content_offset())
    }
    /// Reads the content of a schema's IMPLICIT INTEGER.
    ///
    /// # Errors
    /// Rejects empty or nonminimal two's-complement contents.
    pub fn from_bytes(bytes: &'a [u8], offset: usize) -> Result<Self> {
        let first = bytes
            .first()
            .ok_or_else(|| syntax(offset, "empty ASN.1 INTEGER"))?;
        if bytes.get(1).is_some_and(|second| {
            (*first == 0 && second & 128 == 0) || (*first == 255 && second & 128 != 0)
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
        self.bytes.first().is_some_and(|value| value & 128 != 0)
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
        Ok(if self.bytes.len() > 1 && self.bytes.first() == Some(&0) {
            &self.bytes[1..]
        } else {
            self.bytes
        })
    }
    /// Converts a nonnegative integer to u64.
    ///
    /// # Errors
    /// Rejects negative values and values exceeding u64.
    pub fn to_u64(self) -> Result<u64> {
        self.unsigned()?.iter().try_fold(0u64, |n, &byte| {
            n.checked_mul(256)
                .and_then(|n| n.checked_add(u64::from(byte)))
                .ok_or_else(|| {
                    Error::limit("ASN.1 integer exceeds u64").at(Position::Buffer(self.offset))
                })
        })
    }
}

/// A canonical OBJECT IDENTIFIER, kept as base-128 subidentifier octets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectIdentifier<'a> {
    bytes: &'a [u8],
}

impl<'a> ObjectIdentifier<'a> {
    /// Reads an OBJECT IDENTIFIER (X.690 §8.19).
    ///
    /// # Errors
    /// Rejects a wrong tag, empty identifier, nonminimal arc or truncated arc.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        element.expect(Tag::universal(6, false))?;
        validate_oid(element.content(), element.content_offset())?;
        Ok(Self {
            bytes: element.content(),
        })
    }
    /// Encoded contents, without the tag or length. Equality compares these
    /// canonical bytes and does not require machine-sized arcs.
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
    /// Whether these are the specified canonical content octets.
    pub fn is(self, encoded: &[u8]) -> bool {
        self.bytes == encoded
    }
}

/// A BIT STRING with zero padding validated (X.690 §§8.6 and 11.2).
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
        element.expect(Tag::universal(3, false))?;
        Self::from_bytes(element.content(), element.content_offset())
    }
    /// Reads content octets of a schema's IMPLICIT BIT STRING.
    ///
    /// # Errors
    /// Rejects invalid counts, missing contents and nonzero unused bits.
    pub fn from_bytes(content: &'a [u8], offset: usize) -> Result<Self> {
        let (&unused, bytes) = content
            .split_first()
            .ok_or_else(|| syntax(offset, "missing BIT STRING padding count"))?;
        if unused > 7
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
            .is_some_and(|byte| byte & (128 >> (index % 8)) != 0)
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
            || tag.constructed
            || !matches!(tag.number, 12 | 18..=22 | 25..=28 | 30)
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

/// Reads a canonical BOOLEAN (X.690 §11.1).
///
/// # Errors
/// Rejects a wrong identifier or any content other than one 00 or FF octet.
pub fn boolean(element: Element<'_>) -> Result<bool> {
    element.expect(Tag::universal(1, false))?;
    match element.content() {
        [0] => Ok(false),
        [255] => Ok(true),
        _ => Err(element.error("noncanonical ASN.1 BOOLEAN")),
    }
}

/// Reads a primitive OCTET STRING. Constructed BER octets are exposed by
/// [`crate::cms::OctetString`] instead.
///
/// # Errors
/// Rejects a wrong tag or constructed form.
pub fn octets(element: Element<'_>) -> Result<&[u8]> {
    element.expect(Tag::universal(4, false))?;
    Ok(element.content())
}

pub(crate) fn syntax(offset: usize, message: &'static str) -> Error {
    Error::syntax(message).at(Position::Buffer(offset))
}

pub(crate) fn validate_primitive(tag: Tag, bytes: &[u8], offset: usize) -> Result<()> {
    if tag.class != Class::Universal {
        return Ok(());
    }
    match tag.number {
        1 => {
            if matches!(bytes, [0 | 255]) {
                Ok(())
            } else {
                Err(syntax(offset, "noncanonical ASN.1 BOOLEAN"))
            }
        }
        2 | 10 => Integer::from_bytes(bytes, offset).map(|_| ()),
        3 => BitString::from_bytes(bytes, offset).map(|_| ()),
        4 => Ok(()),
        5 => {
            if bytes.is_empty() {
                Ok(())
            } else {
                Err(syntax(offset, "nonempty ASN.1 NULL"))
            }
        }
        6 | 13 => validate_oid(bytes, offset),
        12 | 18..=22 | 25..=28 | 30 => validate_string(tag.number, bytes, offset),
        23 => Time::read(bytes, offset, false, false).map(|_| ()),
        24 => Time::read(bytes, offset, true, true).map(|_| ()),
        0 | 16 | 17 => Err(syntax(offset, "invalid primitive universal tag")),
        _ => Err(Error::unsupported(
            crate::features::UNIVERSAL_TYPE,
            "unsupported universal ASN.1 type",
        )
        .at(Position::Buffer(offset))),
    }
}

fn validate_oid(bytes: &[u8], offset: usize) -> Result<()> {
    if bytes.is_empty() {
        return Err(syntax(offset, "empty OBJECT IDENTIFIER"));
    }
    let mut first = true;
    for (index, &byte) in bytes.iter().enumerate() {
        if first && byte == 128 {
            return Err(syntax(offset + index, "nonminimal OBJECT IDENTIFIER arc"));
        }
        first = byte & 128 == 0;
    }
    if !first {
        return Err(syntax(
            offset + bytes.len() - 1,
            "unterminated OBJECT IDENTIFIER arc",
        ));
    }
    Ok(())
}

fn validate_string(tag: u32, bytes: &[u8], offset: usize) -> Result<()> {
    let valid = match tag {
        12 => std::str::from_utf8(bytes).is_ok(),
        18 => bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b' '),
        19 => bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b" '()+,-./:=?".contains(byte)),
        22 => bytes.is_ascii(),
        26 => bytes.iter().all(|byte| (32..=126).contains(byte)),
        28 => {
            bytes.len().is_multiple_of(4)
                && bytes.as_chunks::<4>().0.iter().all(|chunk| {
                    let value = chunk
                        .iter()
                        .fold(0u32, |value, &byte| (value << 8) | u32::from(byte));
                    char::from_u32(value).is_some()
                })
        }
        30 => {
            bytes.len().is_multiple_of(2)
                && bytes.as_chunks::<2>().0.iter().all(|chunk| {
                    let value = u16::from_be_bytes([chunk[0], chunk[1]]);
                    !(0xD800..=0xDFFF).contains(&value)
                })
        }
        // Their registered legacy repertoires are preserved, never decoded by guesswork.
        20 | 21 | 25 | 27 => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(syntax(offset, "invalid ASN.1 character string"))
    }
}
