//! A bounded reader for just ECDSA-Sig-Value, not a general ASN.1 parser.

use pagura_core::bytes::Cursor;
use pagura_core::{Error, Position, Result};

use super::{Curve, Uint};

/// X.690 8.9 and 8.3: the universal SEQUENCE and INTEGER tags.
const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
/// X.690 8.1.3.5: a long-form length with one subsequent length byte.
const LONG_LENGTH_ONE_BYTE: u8 = 0x81;
/// The longest valid encoding: a SEQUENCE header with a long-form length and
/// two 66-byte P-521 INTEGERs. Their top byte is at most 1, so they need no
/// sign byte; the shorter P-256 and P-384 integers may.
const MAX_BYTES: usize = 3 + 2 * (2 + 66);

const MESSAGE: &str = "invalid ECDSA signature DER";

/// A tag and its definite length; X.690 10.1 requires the shortest form.
fn header(cursor: &mut Cursor<'_>, tag: u8) -> Result<usize> {
    if cursor.u8()? != tag {
        return Err(cursor.error(MESSAGE));
    }
    match cursor.u8()? {
        short @ 0..=0x7f => Ok(usize::from(short)),
        LONG_LENGTH_ONE_BYTE => match cursor.u8()? {
            long @ 0x80..=0xff => Ok(usize::from(long)),
            _ => Err(cursor.error(MESSAGE)),
        },
        // At most MAX_BYTES exist, so longer lengths are never minimal.
        _ => Err(cursor.error(MESSAGE)),
    }
}

/// A minimally encoded positive INTEGER (X.690 8.3.2) in `1..n`.
fn integer(cursor: &mut Cursor<'_>, curve: Curve) -> Result<Uint> {
    let length = header(cursor, INTEGER)?;
    let start = cursor.position();
    let bytes = cursor.take(length)?;
    let unsigned = match bytes {
        // Empty, negative, or a leading zero that the sign does not need.
        [] | [0x80..=0xff, ..] | [0, 0..=0x7f, ..] => return Err(Error::syntax(MESSAGE).at(start)),
        [0, rest @ ..] => rest,
        _ => bytes,
    };
    if unsigned.len() > curve.coordinate_bytes() {
        return Err(Error::syntax("ECDSA signature integer exceeds curve order").at(start));
    }
    let value = Uint::from_be_bytes(unsigned).ok_or_else(|| Error::syntax(MESSAGE).at(start))?;
    if value.is_zero() || value >= curve.parameters().scalar.value {
        return Err(Error::syntax("ECDSA signature integer outside 1..n").at(start));
    }
    Ok(value)
}

pub(super) fn parse(curve: Curve, bytes: &[u8]) -> Result<(Uint, Uint)> {
    if bytes.len() > MAX_BYTES {
        return Err(
            Error::limit("ECDSA signature DER exceeds 139 bytes").at(Position::Buffer(MAX_BYTES))
        );
    }
    let mut cursor = Cursor::new(bytes).with_message(MESSAGE);
    let length = header(&mut cursor, SEQUENCE)?;
    if length != cursor.remaining() {
        return Err(cursor.error(MESSAGE));
    }
    let r = integer(&mut cursor, curve)?;
    let s = integer(&mut cursor, curve)?;
    if !cursor.at_end() {
        return Err(cursor.error(MESSAGE));
    }
    Ok((r, s))
}
