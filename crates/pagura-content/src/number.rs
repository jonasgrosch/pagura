//! Number operands read in one pass (7.3.3), converted exactly as the
//! object parser converts them. Content operands are never indirect
//! references, so an integer is read without looking at the tokens after
//! it; `N G R` is recognised at its `R` instead.

use crate::token::is_regular;
use pagura_core::repairs::NUMBER_SIGN;
use pagura_core::{Error, Position, Result};

/// The most digit and point bytes, sign excluded, whose value is computed
/// from an integer mantissa; longer numbers are converted from their text.
const SHORT_DIGITS: usize = 15;

/// `10^n` for the decimal places of a short number, each exact in `f64`.
const POWERS: [f64; SHORT_DIGITS] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14,
];

/// A number operand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Number {
    Integer(i64),
    Real(f64),
}

/// The number token at `start` of `data`, which begins with a sign, point
/// or digit, and the offset after it. The token is the run of regular
/// bytes there, as for the object parser.
///
/// A short real is `mantissa / 10^decimals`: both are integers below 2^53
/// and exact in `f64`, so the one correctly rounded division gives the
/// correctly rounded decimal value, as text parsing does.
///
/// # Errors
///
/// The object parser's errors and positions: `invalid number` at the
/// token, the strict error of its lone-sign repair, and out-of-range
/// integers and reals after it.
pub(crate) fn read(data: &[u8], start: usize) -> Result<(Number, usize)> {
    let token = data.get(start..).unwrap_or_default();
    let negative = token.first() == Some(&b'-');
    let sign = usize::from(negative || token.first() == Some(&b'+'));
    // The mantissa is meaningful only for short numbers, whose at most
    // SHORT_DIGITS digits stay below 10^15; longer ones wrap harmlessly and
    // are converted from their text.
    let mut mantissa = 0_i64;
    let mut index = sign;
    let whole = digits(token, &mut index, &mut mantissa);
    let (dots, decimals) = if token.get(index) == Some(&b'.') {
        index += 1;
        (1, digits(token, &mut index, &mut mantissa))
    } else {
        (0, 0)
    };
    // The token is the run of regular bytes: anything regular after the
    // digits (a second point, a letter, a sign) makes it invalid, as does a
    // token without digits. A lone sign is the parser's to repair.
    if whole + decimals == 0 || token.get(index).copied().is_some_and(is_regular) {
        if sign == index && !token.get(index).copied().is_some_and(is_regular) {
            return Err(Error::syntax("sign without digits read as 0")
                .at(Position::Buffer(start))
                .with_repair(NUMBER_SIGN));
        }
        return Err(Error::syntax("invalid number").at(Position::Buffer(start)));
    }
    let length = index;
    let digits = length - sign;
    let end = start + length;
    if digits <= SHORT_DIGITS {
        let number = if dots == 1 {
            let magnitude = mantissa as f64 / POWERS.get(decimals).copied().unwrap_or(1.0);
            Number::Real(if negative { -magnitude } else { magnitude })
        } else {
            Number::Integer(if negative { -mantissa } else { mantissa })
        };
        return Ok((number, end));
    }
    let text = token
        .get(..length)
        .and_then(|token| std::str::from_utf8(token).ok())
        .unwrap_or_default();
    let number = if dots == 1 {
        text.parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(Number::Real)
            .ok_or_else(|| Error::syntax("real number out of range").at(Position::Buffer(end)))?
    } else {
        text.parse()
            .map(Number::Integer)
            .map_err(|_| Error::syntax("integer out of range").at(Position::Buffer(end)))?
    };
    Ok((number, end))
}

/// Reads the run of digits at `index` into `mantissa`, returning their
/// count.
fn digits(token: &[u8], index: &mut usize, mantissa: &mut i64) -> usize {
    let first = *index;
    while let Some(digit) = token
        .get(*index)
        .map(|byte| byte.wrapping_sub(b'0'))
        .filter(|&digit| digit <= 9)
    {
        *mantissa = mantissa.wrapping_mul(10).wrapping_add(i64::from(digit));
        *index += 1;
    }
    *index - first
}

#[cfg(test)]
mod tests {
    use super::{Number, read};
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{Limits, Position, Value};

    /// The object parser's result for the token at the start: the value
    /// and the offset after it, or the error's message and position.
    fn parsed(data: &[u8]) -> Result<(Value, usize), (&'static str, Position)> {
        let mut parser = Parser::new(data, Origin::Buffer, Limits::default());
        parser
            .parse()
            .map(|value| (value, parser.offset()))
            .map_err(|error| (error.message, error.position))
    }

    fn ours(data: &[u8]) -> Result<(Value, usize), (&'static str, Position)> {
        read(data, 0)
            .map(|(number, end)| {
                let value = match number {
                    Number::Integer(n) => Value::Integer(n),
                    Number::Real(n) => Value::Real(n),
                };
                (value, end)
            })
            .map_err(|error| (error.message, error.position))
    }

    #[test]
    fn numbers_convert_and_fail_as_the_object_parser_does() {
        for token in [
            &b"0 "[..],
            b"12 m",
            b"-12)",
            b"+7/",
            b"3.25 ",
            b"-.5[",
            b"+4.",
            b"0.1 ",
            b"007 ",
            b"123456789012345 ",
            b"1234567890123456 ",
            b"1.23456789012345 ",
            b"1.234567890123456 ",
            b"0.30000000000000004 ",
            b"-0.0 ",
            b"7 % comment\n",
            b"99999999999999999999 ",
            b"-9223372036854775808 ",
            b"1.2.3 ",
            b"12a ",
            b". ",
            b"- ",
            b"+",
            b"--1 ",
            b"+.",
        ] {
            assert_eq!(ours(token), parsed(token), "{token:?}");
        }
    }

    #[test]
    fn integers_never_look_for_a_reference() {
        assert_eq!(ours(b"12 0 R"), Ok((Value::Integer(12), 2)));
        assert_eq!(ours(b"0 0 R"), Ok((Value::Integer(0), 1)));
    }
}
