//! Number operands read in one pass (7.3.3): the commonest content token,
//! converted exactly as the object parser converts it.

use pagura_core::Value;
use pagura_core::syntax::{is_regular, is_whitespace};

/// The most digit and point bytes, sign excluded, read here; longer
/// numbers take the object parser's general path.
const SHORT_DIGITS: usize = 15;

/// `10^n` for the decimal places of a short number, each exact in `f64`.
const POWERS: [f64; SHORT_DIGITS] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14,
];

/// The number token at the start of `data` and its length, when it is a
/// valid number of at most [`SHORT_DIGITS`] digit and point bytes that
/// cannot start an indirect reference `N G R`. Anything else, malformed
/// numbers included, is `None` and left to the object parser, so values,
/// errors and repairs are the parser's.
///
/// A real is `mantissa / 10^decimals`: both are integers below 2^53 and
/// exact in `f64`, so the one correctly rounded division gives the
/// correctly rounded decimal value, as text parsing does.
pub(crate) fn short(data: &[u8]) -> Option<(Value, usize)> {
    let negative = data.first() == Some(&b'-');
    let sign = usize::from(negative || data.first() == Some(&b'+'));
    let (mut mantissa, mut decimals, mut dots, mut count) = (0_i64, 0_usize, 0_usize, 0_usize);
    for &byte in data.get(sign..)? {
        match byte {
            b'0'..=b'9' => {
                // At most SHORT_DIGITS + 1 digits accumulate before the
                // length check returns: below 10^16, so no wrap is possible.
                mantissa = mantissa
                    .wrapping_mul(10)
                    .wrapping_add(i64::from(byte - b'0'));
                decimals += dots;
            }
            b'.' if dots == 0 => dots = 1,
            _ if is_regular(byte) => return None,
            _ => break,
        }
        count += 1;
        if count > SHORT_DIGITS {
            return None;
        }
    }
    if count == dots {
        return None;
    }
    let length = sign + count;
    if dots == 1 {
        let magnitude = mantissa as f64 / *POWERS.get(decimals)?;
        return Some((Value::Real(if negative { -magnitude } else { magnitude }), length));
    }
    if sign == 0 && reference_may_follow(data.get(length..)?) {
        return None;
    }
    Some((Value::Integer(if negative { -mantissa } else { mantissa }), length))
}

/// Whether `G R` could follow an unsigned integer: digits, then `R` as a
/// complete token, each after white space or comments. A superset of the
/// references the object parser accepts, which then decides.
fn reference_may_follow(data: &[u8]) -> bool {
    let data = skip(data);
    let digits = data.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return false;
    }
    let data = skip(data.get(digits..).unwrap_or_default());
    data.first() == Some(&b'R') && data.get(1).is_none_or(|&b| !is_regular(b))
}

/// `data` after leading white space and comments.
fn skip(mut data: &[u8]) -> &[u8] {
    loop {
        let blank = data.iter().take_while(|&&b| is_whitespace(b)).count();
        data = data.get(blank..).unwrap_or_default();
        if data.first() != Some(&b'%') {
            return data;
        }
        let comment = data.iter().take_while(|&&b| b != b'\r' && b != b'\n').count();
        data = data.get(comment..).unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::short;
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Limits, Value};

    /// The object parser's value and length for the token at the start.
    fn parsed(data: &[u8]) -> Option<(Value, usize)> {
        let mut parser = Parser::new(data, Origin::Buffer, Limits::default());
        parser.parse().ok().map(|value| (value, parser.offset()))
    }

    #[test]
    fn short_numbers_convert_as_the_object_parser_does() {
        for token in [
            &b"0 "[..],
            b"12 m",
            b"-12)",
            b"+7/",
            b"3.25 ",
            b"-.5[",
            b"+4.",
            b"0.1 ",
            b"123456789012345 ",
            b"1.23456789012345 ",
            b"0.30000000000000004 ",
            b"-0.0 ",
            b"7 % comment\n",
            b"12 0 Rx ",
        ] {
            if let Some((value, length)) = short(token) {
                assert_eq!(Some((value, length)), parsed(token), "{token:?}");
            }
        }
    }

    #[test]
    fn references_malformed_and_long_numbers_are_left_to_the_parser() {
        for token in [
            &b"12 0 R"[..],
            b"12 % c\n 0 % d\n R ",
            b"1234567890123456 ",
            b"1.2.3 ",
            b"12a ",
            b". ",
            b"- ",
            b"+",
        ] {
            assert_eq!(short(token), None, "{token:?}");
        }
        assert_eq!(short(b"-12 0 R"), Some((Value::Integer(-12), 3)));
        assert_eq!(short(b"1.5 0 R"), Some((Value::Real(1.5), 3)));
    }
}
