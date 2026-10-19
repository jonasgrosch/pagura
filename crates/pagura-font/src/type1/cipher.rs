//! Type 1 eexec and charstring encryption (Type 1 chapter 7) and the
//! hexadecimal form of eexec data.
use crate::budget::Work;
use crate::read::syntax;
use pagura_core::syntax::hex_digit;
use pagura_core::{Position, Result};

/// The eexec and charstring keys and the cipher constants (Type 1 7.1).
pub(super) const EEXEC_KEY: u16 = 55665;
pub(super) const CHARSTRING_KEY: u16 = 4330;
const C1: u16 = 52845;
const C2: u16 = 22719;
/// eexec plaintext begins after four random bytes (Type 1 7.2).
pub(super) const EEXEC_DISCARD: usize = 4;
/// Hexadecimal eexec data is recognized by its first four digits (Type 1 7.2).
const HEX_PROBE: usize = 4;

/// Decrypts `data`, dropping its first `discard` plaintext bytes. `at`
/// locates `data` for the error when it is shorter than that.
pub(super) fn decrypt(data: &[u8], key: u16, discard: usize, at: Position) -> Result<Vec<u8>> {
    let len = data
        .len()
        .checked_sub(discard)
        .ok_or_else(|| syntax(at, "truncated Type 1 cipher prefix"))?;
    let mut state = key;
    let mut result = Vec::with_capacity(len);
    for (i, &byte) in data.iter().enumerate() {
        let [high, _] = state.to_be_bytes();
        state = u16::from(byte)
            .wrapping_add(state)
            .wrapping_mul(C1)
            .wrapping_add(C2);
        if i >= discard {
            result.push(byte ^ high);
        }
    }
    Ok(result)
}

/// Hexadecimal eexec data decoded to bytes.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Hex {
    pub(super) bytes: Vec<u8>,
    /// The decoded index and flat offset of each digit pair that does not
    /// directly follow the previous one.
    pub(super) pairs: Vec<(usize, usize)>,
}

/// Hexadecimal eexec `data` (at flat offset `start`) decoded, or `None` if
/// the data is binary.
pub(super) fn hex(data: &[u8], start: usize, work: &Work<'_>) -> Result<Option<Hex>> {
    let mut digits = data
        .iter()
        .filter(|b| !b.is_ascii_whitespace())
        .take(HEX_PROBE);
    if !digits.all(u8::is_ascii_hexdigit) {
        return Ok(None);
    }
    let mut decoded = Vec::new();
    let mut pairs = Vec::new();
    let mut high = None;
    for (i, &byte) in data.iter().enumerate() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let Some(digit) = hex_digit(byte) else {
            break;
        };
        let at = start + i;
        if let Some(high) = high.take() {
            work.decoded(decoded.len() + 1, Position::Buffer(at))?;
            decoded.push(high * 16 + digit);
        } else {
            let expected = pairs
                .last()
                .map(|&(index, flat)| flat + 2 * (decoded.len() - index));
            if expected != Some(at) {
                pairs.push((decoded.len(), at));
            }
            high = Some(digit);
        }
    }
    if high.is_some() {
        return Err(syntax(
            Position::Buffer(start + data.len()),
            "odd eexec hexadecimal data",
        ));
    }
    Ok(Some(Hex {
        bytes: decoded,
        pairs,
    }))
}

#[cfg(test)]
mod tests {
    use super::{CHARSTRING_KEY, EEXEC_KEY, decrypt, hex};
    use crate::budget::{FontBudget, Work};
    use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};

    fn encrypt(plain: &[u8], key: u16) -> Vec<u8> {
        let mut state = key;
        plain
            .iter()
            .map(|&p| {
                let [high, _] = state.to_be_bytes();
                let c = p ^ high;
                state = u16::from(c)
                    .wrapping_add(state)
                    .wrapping_mul(52845)
                    .wrapping_add(22719);
                c
            })
            .collect()
    }

    #[test]
    fn decryption_inverts_type1_encryption_and_drops_the_prefix() {
        let plain = b"abcd/Private 8 dict";
        for key in [EEXEC_KEY, CHARSTRING_KEY] {
            let cipher = encrypt(plain, key);
            assert_eq!(
                decrypt(&cipher, key, 4, Position::Unknown).unwrap(),
                &plain[4..]
            );
        }
        assert_eq!(
            decrypt(b"ab", EEXEC_KEY, 4, Position::Buffer(3))
                .unwrap_err()
                .position,
            Position::Buffer(3)
        );
    }

    #[test]
    fn hex_data_records_where_digit_pairs_restart() {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        let decoded = hex(b"0aFF\n 12 x", 50, &work).unwrap().unwrap();
        assert_eq!(decoded.bytes, [0x0a, 0xff, 0x12]);
        assert_eq!(decoded.pairs, [(0, 50), (2, 56)]);
        assert_eq!(hex(b"\x80abc", 0, &work).unwrap(), None);
        assert_eq!(hex(b"abc", 0, &work).unwrap_err().kind, ErrorKind::Syntax);
    }
}
