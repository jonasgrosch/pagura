//! Canonical ASN.1 times (X.690 11.7-11.8) and the PKIX time profile.

use super::syntax;
use crate::der::{self, Element};
use pagura_core::{Error, Result};

/// The time syntax being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeSyntax {
    /// UTCTime: two-digit year, whole seconds (X.690 11.8).
    Utc,
    /// GeneralizedTime with whole seconds, as RFC 5280 4.1.2.5.2 requires.
    Generalized,
    /// GeneralizedTime with an optional canonical fraction (X.690 11.7;
    /// RFC 3161 2.4.2).
    Fractional,
}

/// A calendar time in UTC, with exact decimal fractions retained.
/// No floating-point conversion or host clock is involved.
///
/// The fields are checked calendar values. The derived ordering is
/// chronological: fields compare from the year down, and fractions compare
/// as digit strings, which orders them numerically because canonical
/// fractions have no trailing zero (`"5"` > `"45"`, `"1"` < `"15"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time<'a> {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    fraction: &'a str,
}

/// The UTCTime pivot of RFC 5280 4.1.2.5.1: two-digit years below it are 20YY.
const UTC_PIVOT: u16 = 50;
/// The largest year GeneralizedTime's four digits encode.
const MAX_YEAR: u16 = 9999;

impl<'a> Time<'a> {
    /// A whole-second UTC time supplied by the host, such as an evaluation
    /// time for validation.
    ///
    /// # Errors
    /// An `InvalidArgument` error for a year above 9999, a month outside
    /// 1–12, a day outside its month, an hour above 23, a minute above 59 or
    /// a second above 59.
    pub fn from_utc(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Result<Time<'static>> {
        if year > MAX_YEAR || !valid_clock(year, month, day, hour, minute, second) {
            return Err(Error::invalid_argument("invalid UTC calendar time"));
        }
        Ok(Time {
            year,
            month,
            day,
            hour,
            minute,
            second,
            fraction: "",
        })
    }

    /// This time with fractional-second `digits`, such as a host's sub-second
    /// evaluation time. Canonical fractions have no trailing zero (X.690 11.7.3).
    ///
    /// # Errors
    /// An `InvalidArgument` error for an empty fraction, a non-digit or a
    /// trailing zero.
    pub fn with_fraction(self, digits: &'a str) -> Result<Self> {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.ends_with('0')
        {
            return Err(Error::invalid_argument("noncanonical fractional seconds"));
        }
        Ok(Self {
            fraction: digits,
            ..self
        })
    }

    /// Reads a PKIX UTCTime or GeneralizedTime (RFC 5280 4.1.2.5).
    ///
    /// # Errors
    /// Rejects missing seconds, offsets, fractions, impossible dates and wrong tags.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let syntax = match element.tag() {
            der::UTC_TIME => TimeSyntax::Utc,
            der::GENERALIZED_TIME => TimeSyntax::Generalized,
            _ => return Err(element.error("expected PKIX time")),
        };
        Self::read(element.content(), element.content_offset(), syntax)
    }

    /// Reads a timestamp GeneralizedTime including canonical fractions
    /// (RFC 3161 2.4.2; X.690 11.7).
    ///
    /// # Errors
    /// Rejects noncanonical syntax, wrong tags or invalid calendar fields.
    pub fn generalized(element: Element<'a>) -> Result<Self> {
        element.expect(der::GENERALIZED_TIME)?;
        Self::read(
            element.content(),
            element.content_offset(),
            TimeSyntax::Fractional,
        )
    }

    pub(crate) fn read(bytes: &'a [u8], offset: usize, syntax: TimeSyntax) -> Result<Self> {
        let error = || self::syntax(offset, "invalid or noncanonical ASN.1 time");
        let (year, rest) = match syntax {
            TimeSyntax::Utc => {
                let (digits, rest) = bytes.split_first_chunk::<2>().ok_or_else(error)?;
                let year = u16::from(decimal(*digits).ok_or_else(error)?);
                (year + if year < UTC_PIVOT { 2000 } else { 1900 }, rest)
            }
            TimeSyntax::Generalized | TimeSyntax::Fractional => {
                let (&[c0, c1, y0, y1], rest) = bytes.split_first_chunk::<4>().ok_or_else(error)?;
                let century = decimal([c0, c1]).ok_or_else(error)?;
                let years = decimal([y0, y1]).ok_or_else(error)?;
                (u16::from(century) * 100 + u16::from(years), rest)
            }
        };
        let (&[m0, m1, d0, d1, h0, h1, i0, i1, s0, s1], suffix) =
            rest.split_first_chunk::<10>().ok_or_else(error)?;
        let (Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
            decimal([m0, m1]),
            decimal([d0, d1]),
            decimal([h0, h1]),
            decimal([i0, i1]),
            decimal([s0, s1]),
        ) else {
            return Err(error());
        };
        if !valid_clock(year, month, day, hour, minute, second) {
            return Err(error());
        }
        let fraction = match (suffix, syntax) {
            (b"Z", _) => "",
            ([b'.', digits @ .., b'Z'], TimeSyntax::Fractional)
                if !digits.is_empty()
                    && digits.iter().all(u8::is_ascii_digit)
                    && digits.last() != Some(&b'0') =>
            {
                std::str::from_utf8(digits).map_err(|_| error())?
            }
            _ => return Err(error()),
        };
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            fraction,
        })
    }

    /// Four-digit year (UTCTime's two digits pivot at 1950/2050).
    pub const fn year(self) -> u16 {
        self.year
    }
    /// Month, 1–12.
    pub const fn month(self) -> u8 {
        self.month
    }
    /// Day of the month, valid for the month and year.
    pub const fn day(self) -> u8 {
        self.day
    }
    /// Hour, 0–23.
    pub const fn hour(self) -> u8 {
        self.hour
    }
    /// Minute, 0–59.
    pub const fn minute(self) -> u8 {
        self.minute
    }
    /// Second, 0–59.
    pub const fn second(self) -> u8 {
        self.second
    }
    /// Canonical fractional-second digits, or empty for whole seconds.
    pub const fn fraction(self) -> &'a str {
        self.fraction
    }
    /// This time truncated to whole seconds, independent of the input it was
    /// read from. PKIX times (RFC 5280 4.1.2.5) are already whole seconds.
    pub const fn whole_seconds(self) -> Time<'static> {
        Time {
            year: self.year,
            month: self.month,
            day: self.day,
            hour: self.hour,
            minute: self.minute,
            second: self.second,
            fraction: "",
        }
    }
}

/// Two ASCII decimal digits.
fn decimal(digits: [u8; 2]) -> Option<u8> {
    let [tens, units] = digits;
    (tens.is_ascii_digit() && units.is_ascii_digit()).then(|| (tens - b'0') * 10 + (units - b'0'))
}

fn valid_clock(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> bool {
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return false,
    };
    (1..=days).contains(&day) && hour <= 23 && minute <= 59 && second <= 59
}
