//! Canonical ASN.1 times and the PKIX time profile.

use super::syntax;
use crate::der::{Element, Tag};
use pagura_core::Result;

/// A calendar time in UTC, with exact decimal fractions retained.
/// No floating-point conversion or host clock is involved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time<'a> {
    /// Four-digit year (UTCTime's pivot is 1950/2050).
    pub year: u16,
    /// Month, 1–12.
    pub month: u8,
    /// Day, validated against month and leap year.
    pub day: u8,
    /// Hour, 0–23.
    pub hour: u8,
    /// Minute, 0–59.
    pub minute: u8,
    /// Second, 0–59.
    pub second: u8,
    /// Canonical fractional-second digits, or empty for whole seconds.
    pub fraction: &'a str,
}

impl<'a> Time<'a> {
    /// Reads a PKIX UTCTime or GeneralizedTime (RFC 5280 §4.1.2.5).
    ///
    /// # Errors
    /// Rejects missing seconds, offsets, fractions, impossible dates and wrong tags.
    pub fn parse(element: Element<'a>) -> Result<Self> {
        let generalized = match element.tag() {
            tag if tag == Tag::universal(23, false) => false,
            tag if tag == Tag::universal(24, false) => true,
            _ => return Err(element.error("expected PKIX time")),
        };
        Self::read(
            element.content(),
            element.content_offset(),
            generalized,
            false,
        )
    }

    /// Reads a timestamp GeneralizedTime including canonical fractions
    /// (RFC 3161 §2.4.2; X.690 §11.7).
    ///
    /// # Errors
    /// Rejects noncanonical syntax, wrong tags or invalid calendar fields.
    pub fn generalized(element: Element<'a>) -> Result<Self> {
        element.expect(Tag::universal(24, false))?;
        Self::read(element.content(), element.content_offset(), true, true)
    }

    pub(crate) fn read(
        bytes: &'a [u8],
        offset: usize,
        generalized: bool,
        fractions: bool,
    ) -> Result<Self> {
        let error = || syntax(offset, "invalid or noncanonical ASN.1 time");
        let year_len = if generalized { 4 } else { 2 };
        let integral_len = year_len + 10;
        let integral = bytes.get(..integral_len).ok_or_else(error)?;
        if !integral.iter().all(u8::is_ascii_digit) || bytes.last() != Some(&b'Z') {
            return Err(error());
        }
        let mut rest = integral;
        let mut digits = |count: usize| -> u16 {
            let (number, tail) = rest.split_at(count);
            rest = tail;
            number
                .iter()
                .fold(0u16, |n, digit| n * 10 + u16::from(digit - b'0'))
        };
        let mut year = digits(year_len);
        if !generalized {
            year += if year >= 50 { 1900 } else { 2000 };
        }
        let month = u8::try_from(digits(2)).map_err(|_| error())?;
        let day = u8::try_from(digits(2)).map_err(|_| error())?;
        let hour = u8::try_from(digits(2)).map_err(|_| error())?;
        let minute = u8::try_from(digits(2)).map_err(|_| error())?;
        let second = u8::try_from(digits(2)).map_err(|_| error())?;
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days = match month {
            4 | 6 | 9 | 11 => 30,
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            _ => return Err(error()),
        };
        if day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
            return Err(error());
        }
        let suffix = bytes.get(integral_len..).ok_or_else(error)?;
        let fraction = if suffix == b"Z" {
            ""
        } else {
            if !fractions || suffix.first() != Some(&b'.') || suffix.len() < 3 {
                return Err(error());
            }
            let digits = &suffix[1..suffix.len() - 1];
            if !digits.iter().all(u8::is_ascii_digit) || digits.last() == Some(&b'0') {
                return Err(error());
            }
            std::str::from_utf8(digits).map_err(|_| error())?
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
}
