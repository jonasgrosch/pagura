//! Source-bound byte ranges (12.8.1, Table 255; 12.8.3.3.1).
use crate::{
    ObjectSource,
    view::{Context, Source},
};
use pagura_core::{Dictionary, Result, Value};
use std::ops::Range;

/// A failed byte-range structure check; cryptography has not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ByteRangeError {
    /// Missing ByteRange or anything other than four direct integers.
    ExpectedTwoRanges,
    /// A number is negative or cannot be represented as a file offset.
    InvalidInteger,
    /// The first range does not start at byte zero.
    DoesNotStartAtZero,
    /// Ranges overlap or are reversed.
    OverlappingRanges,
    /// An addition overflows or an endpoint exceeds the original file.
    OutsideFile,
    /// The excluded gap differs from the actual Contents token, delimiters included.
    ContentsGapMismatch,
    /// Contents is not a direct hexadecimal string.
    ContentsNotHex,
    /// Signature dictionary values must be direct objects (Table 255).
    IndirectValue,
    /// Physical source cannot be bound, including compressed signature dictionaries.
    SourceUnavailable,
    /// The source needs repair, so its token identity is ambiguous.
    AmbiguousSource,
    /// The signature's enclosing object ends beyond the last signed byte.
    SignatureOutsideSignedBytes,
}
/// Exactly two checked ranges borrowing original bytes. No digest is computed.
/// Private state prevents constructing ranges that are unchecked or mismatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteRange<'a> {
    bytes: &'a [u8],
    ranges: [Range<usize>; 2],
}
impl<'a> ByteRange<'a> {
    /// Original file ranges in digest order.
    pub fn ranges(&self) -> &[Range<usize>; 2] {
        &self.ranges
    }
    /// End offset of the second range, used to identify the signed revision.
    pub fn signed_end(&self) -> usize {
        self.ranges[1].end
    }
    /// Streaming borrowed chunks in digest order. There is no concatenated copy.
    pub fn signed_bytes(&self) -> impl ExactSizeIterator<Item = &'a [u8]> + Clone + '_ {
        self.ranges.iter().map(|range| &self.bytes[range.clone()])
    }
}

pub(super) fn check<'a>(
    ctx: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> Result<std::result::Result<ByteRange<'a>, ByteRangeError>> {
    if dictionary
        .iter()
        .any(|(_, v)| matches!(v, Value::Reference(_)))
    {
        return Ok(Err(ByteRangeError::IndirectValue));
    }
    let Some([a, b, c, d]) = dictionary.get(b"ByteRange").and_then(Value::as_array) else {
        return Ok(Err(ByteRangeError::ExpectedTwoRanges));
    };
    let integer = |value: &Value| value.as_integer().and_then(|n| usize::try_from(n).ok());
    let (Some(start), Some(length), Some(next), Some(tail)) =
        (integer(a), integer(b), integer(c), integer(d))
    else {
        return Ok(Err(ByteRangeError::InvalidInteger));
    };
    if start != 0 {
        return Ok(Err(ByteRangeError::DoesNotStartAtZero));
    }
    let bytes = ctx.document.original_bytes();
    let Some(end) = next.checked_add(tail).filter(|&end| end <= bytes.len()) else {
        return Ok(Err(ByteRangeError::OutsideFile));
    };
    if length > bytes.len() || next > bytes.len() {
        return Ok(Err(ByteRangeError::OutsideFile));
    }
    if length > next {
        return Ok(Err(ByteRangeError::OverlappingRanges));
    }
    let Some(object) = source
        .object
        .map(|id| ctx.document.object(id))
        .transpose()?
    else {
        return Ok(Err(ByteRangeError::SourceUnavailable));
    };
    let ObjectSource::File(extent) = object.source() else {
        return Ok(Err(ByteRangeError::SourceUnavailable));
    };
    if extent.end > end {
        return Ok(Err(ByteRangeError::SignatureOutsideSignedBytes));
    }
    ctx.scan(extent.len(), source)?;
    let span =
        super::source::contents_span(ctx, dictionary, object.value(), extent.clone(), source);
    let span = match span {
        Ok(span) => span,
        Err(error) if error.kind == pagura_core::ErrorKind::Limit => return Err(error),
        Err(_) => return Ok(Err(ByteRangeError::AmbiguousSource)),
    };
    let Some(span) = span else {
        return Ok(Err(ByteRangeError::SourceUnavailable));
    };
    if bytes.get(span.start) != Some(&b'<') || bytes.get(span.start + 1) == Some(&b'<') {
        return Ok(Err(ByteRangeError::ContentsNotHex));
    }
    if span != (length..next) {
        return Ok(Err(ByteRangeError::ContentsGapMismatch));
    }
    Ok(Ok(ByteRange {
        bytes,
        ranges: [0..length, next..end],
    }))
}
