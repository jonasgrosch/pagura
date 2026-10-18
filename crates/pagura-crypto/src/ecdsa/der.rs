//! A bounded reader for just ECDSA-Sig-Value, not a general ASN.1 parser.

use super::{Curve, Uint};
use pagura_core::{Error, Position, Result};

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Reader<'_> {
    fn error(&self) -> Error {
        Error::syntax("invalid ECDSA signature DER").at(Position::Buffer(self.offset))
    }

    fn byte(&mut self) -> Result<u8> {
        let byte = self
            .bytes
            .get(self.offset)
            .copied()
            .ok_or_else(|| self.error())?;
        self.offset += 1;
        Ok(byte)
    }

    fn header(&mut self, tag: u8) -> Result<usize> {
        if self.byte()? != tag {
            return Err(self.error());
        }
        match self.byte()? {
            short @ 0..=127 => Ok(usize::from(short)),
            129 => {
                let length = self.byte()?;
                if length < 128 {
                    return Err(self.error());
                }
                Ok(usize::from(length))
            }
            // At most 139 bytes exist, so longer lengths are never minimal.
            _ => Err(self.error()),
        }
    }

    fn integer(&mut self, curve: Curve) -> Result<Uint> {
        let length = self.header(2)?;
        let start = self.offset;
        let bytes = self
            .bytes
            .get(start..start + length)
            .ok_or_else(|| self.error())?;
        let first = *bytes.first().ok_or_else(|| self.error())?;
        if first & 0x80 != 0 || (bytes.len() > 1 && first == 0 && bytes[1] & 0x80 == 0) {
            return Err(self.error());
        }
        self.offset += length;
        let unsigned = if first == 0 { &bytes[1..] } else { bytes };
        if unsigned.len() > curve.coordinate_bytes() {
            return Err(Error::syntax("ECDSA signature integer exceeds curve order")
                .at(Position::Buffer(start)));
        }
        let value = Uint::from_be_bytes(unsigned).ok_or_else(|| self.error())?;
        if value.is_zero() || value >= curve.parameters().scalar.value {
            return Err(
                Error::syntax("ECDSA signature integer outside 1..n").at(Position::Buffer(start))
            );
        }
        Ok(value)
    }
}

pub(super) fn parse(curve: Curve, bytes: &[u8]) -> Result<(Uint, Uint)> {
    if bytes.len() > 139 {
        return Err(Error::limit("ECDSA signature DER exceeds 139 bytes").at(Position::Buffer(0)));
    }
    let mut reader = Reader { bytes, offset: 0 };
    let length = reader.header(0x30)?;
    if length != bytes.len() - reader.offset {
        return Err(reader.error());
    }
    let r = reader.integer(curve)?;
    let s = reader.integer(curve)?;
    if reader.offset != bytes.len() {
        return Err(reader.error());
    }
    Ok((r, s))
}
