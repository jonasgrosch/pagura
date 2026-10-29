//! Content tokens read in place (7.2, 7.3): white space, words, strings
//! and names, decoded exactly as the object parser decodes them. These
//! readers cover the well-formed tokens; anything the object parser must
//! report or repair (an unterminated string, an invalid hexadecimal digit,
//! a name escape, a length budget) yields `None` and is read by the object
//! parser instead, so errors and repairs stay the parser's.

use pagura_core::syntax::{hex_digit, is_delimiter, is_whitespace};

/// A byte's class: white space, delimiter, or regular (7.2.3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Regular,
    White,
    Delimiter,
}

/// The class of every byte, one lookup in place of a chain of tests.
const CLASSES: [Class; 256] = {
    let mut classes = [Class::Regular; 256];
    let mut byte = 0;
    while byte < 256 {
        // In range: the loop stops at 255.
        #[expect(clippy::cast_possible_truncation, reason = "byte < 256")]
        let value = byte as u8;
        if is_whitespace(value) {
            classes[byte] = Class::White;
        } else if is_delimiter(value) {
            classes[byte] = Class::Delimiter;
        }
        byte += 1;
    }
    classes
};

/// Whether `byte` is regular: neither white space nor a delimiter.
pub(crate) fn is_regular(byte: u8) -> bool {
    matches!(CLASSES[usize::from(byte)], Class::Regular)
}

fn is_white(byte: u8) -> bool {
    matches!(CLASSES[usize::from(byte)], Class::White)
}

/// The offset of the first byte at or after `offset` that is neither white
/// space nor in a comment.
pub(crate) fn skip(data: &[u8], mut offset: usize) -> usize {
    loop {
        while data.get(offset).copied().is_some_and(is_white) {
            offset += 1;
        }
        if data.get(offset) != Some(&b'%') {
            return offset;
        }
        while data.get(offset).is_some_and(|&b| b != b'\r' && b != b'\n') {
            offset += 1;
        }
    }
}

/// The run of regular bytes at `offset`: a keyword, operator or number.
pub(crate) fn word(data: &[u8], offset: usize) -> &[u8] {
    let rest = data.get(offset..).unwrap_or_default();
    let length = rest.iter().take_while(|&&b| is_regular(b)).count();
    rest.get(..length).unwrap_or_default()
}

/// Appends the literal string at `start` (its `(`) to `out` and returns
/// the offset after its `)`, with at most `limit` bytes decoded.
pub(crate) fn literal(data: &[u8], start: usize, limit: usize, out: &mut Vec<u8>) -> Option<usize> {
    let base = out.len();
    let mut position = start + 1;
    let mut open = 1_usize;
    loop {
        // Copy the run of bytes that need no decoding at once.
        let rest = data.get(position..)?;
        let plain = rest
            .iter()
            .take_while(|&&b| !matches!(b, b'(' | b')' | b'\\' | b'\r'))
            .count();
        if out.len() - base + plain > limit {
            return None;
        }
        out.extend_from_slice(rest.get(..plain)?);
        position += plain;
        let byte = match *data.get(position)? {
            b'(' => {
                open += 1;
                b'('
            }
            b')' => {
                open -= 1;
                if open == 0 {
                    return Some(position + 1);
                }
                b')'
            }
            b'\\' => {
                position += 1;
                match escape(data, &mut position)? {
                    Escape::Byte(byte) => byte,
                    Escape::Continuation => continue,
                }
            }
            _ => {
                // An unescaped end-of-line marker of any kind reads as LF.
                if data.get(position + 1) == Some(&b'\n') {
                    position += 1;
                }
                b'\n'
            }
        };
        position += 1;
        if out.len() - base >= limit {
            return None;
        }
        out.push(byte);
    }
}

/// What an escape in a literal string stands for.
enum Escape {
    Byte(u8),
    /// A backslash before an end of line: nothing.
    Continuation,
}

/// Decodes the escape whose byte is at `position`, leaving `position` at
/// its last byte, or after it for a line continuation.
fn escape(data: &[u8], position: &mut usize) -> Option<Escape> {
    let byte = match *data.get(*position)? {
        b'n' => b'\n',
        b'r' => b'\r',
        b't' => b'\t',
        b'b' => 0x08,
        b'f' => 0x0c,
        b'\r' => {
            if data.get(*position + 1) == Some(&b'\n') {
                *position += 1;
            }
            *position += 1;
            return Some(Escape::Continuation);
        }
        b'\n' => {
            *position += 1;
            return Some(Escape::Continuation);
        }
        first @ b'0'..=b'7' => {
            let mut code = u16::from(first - b'0');
            for _ in 0..2 {
                match data.get(*position + 1) {
                    Some(&digit @ b'0'..=b'7') => {
                        *position += 1;
                        code = code * 8 + u16::from(digit - b'0');
                    }
                    _ => break,
                }
            }
            // Clause 7.3.4.2: high-order overflow of \ddd is ignored.
            code.to_le_bytes()[0]
        }
        other => other,
    };
    Some(Escape::Byte(byte))
}

/// Appends the hexadecimal string at `start` (its `<`) to `out` and
/// returns the offset after its `>`, with at most `limit` bytes decoded.
pub(crate) fn hex(data: &[u8], start: usize, limit: usize, out: &mut Vec<u8>) -> Option<usize> {
    let base = out.len();
    let mut position = start + 1;
    let mut high = None;
    loop {
        let byte = *data.get(position)?;
        position += 1;
        if byte == b'>' {
            break;
        }
        if is_whitespace(byte) {
            continue;
        }
        let digit = hex_digit(byte)?;
        match high.take() {
            Some(high) => {
                if out.len() - base >= limit {
                    return None;
                }
                out.push((high << 4) | digit);
            }
            None => high = Some(digit),
        }
    }
    // An odd final digit behaves as if followed by 0.
    if let Some(high) = high {
        if out.len() - base >= limit {
            return None;
        }
        out.push(high << 4);
    }
    Some(position)
}

/// The bytes of the name at `start` (its `/`) when it has no `#` escape
/// and at most `limit` bytes, and the offset after it.
pub(crate) fn name(data: &[u8], start: usize, limit: usize) -> Option<(&[u8], usize)> {
    let body = word(data, start + 1);
    if body.len() > limit || body.contains(&b'#') {
        return None;
    }
    Some((body, start + 1 + body.len()))
}

#[cfg(test)]
mod tests {
    use super::{hex, literal, name, skip, word};
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Limits, Value};

    /// The object parser's string or name and the offset after it.
    fn parsed(data: &[u8], limits: Limits) -> Option<(Value, usize)> {
        let mut parser = Parser::new(data, Origin::Buffer, limits);
        parser.parse().ok().map(|value| (value, parser.offset()))
    }

    fn string(data: &[u8], limit: usize) -> Option<(Vec<u8>, usize)> {
        let mut out = b"kept".to_vec();
        let end = match data.first() {
            Some(b'(') => literal(data, 0, limit, &mut out)?,
            _ => hex(data, 0, limit, &mut out)?,
        };
        assert_eq!(out.get(..4), Some(&b"kept"[..]));
        Some((out.split_off(4), end))
    }

    #[test]
    fn strings_decode_as_the_object_parser_decodes_them() {
        let limits = Limits {
            string_bytes: 6,
            ..Limits::default()
        };
        for data in [
            &b"(abc) Tj"[..],
            b"(a(b)c)",
            b"(a\\(b\\)\\\\c)",
            b"(\\n\\r\\t\\b\\f\\q)",
            b"(\\101\\7\\0612\\777)",
            b"(a\\\r\nb\\\nc\\\rd)",
            b"(a\r\nb\rc\nd)",
            b"(abcdef)",
            b"(abcdefg)",
            b"(abc\\\ndefg)",
            b"(abcde\\nf)",
            b"(abcdef\\n)",
            b"(unterminated",
            b"(\\",
            b"<41 42 4>",
            b"<414243444546>",
            b"<41424344454647>",
            b"<4142434445464>",
            b"<41x>",
            b"<41",
            b"<>",
        ] {
            let expected = parsed(data, limits).map(|(value, end)| match value {
                Value::String(bytes) => (bytes, end),
                other => panic!("{other:?}"),
            });
            assert_eq!(string(data, limits.string_bytes), expected, "{data:?}");
        }
    }

    #[test]
    fn names_without_escapes_are_read_in_place() {
        for data in [&b"/F1 12"[..], b"/A/B", b"/ ", b"/abcdef"] {
            let (bytes, end) = name(data, 0, 6).unwrap();
            assert_eq!(
                Some((Value::name(bytes), end)),
                parsed(data, Limits::default()),
                "{data:?}"
            );
        }
        assert_eq!(name(b"/A#20B", 0, 64), None);
        assert_eq!(name(b"/abcdefg", 0, 6), None);
    }

    #[test]
    fn white_space_comments_and_words() {
        let data = b"  % note\r\n\t% more\nBT/F";
        let start = skip(data, 0);
        assert_eq!(word(data, start), b"BT");
        assert_eq!(skip(data, data.len()), data.len());
        assert_eq!(word(data, data.len()), b"");
    }
}
