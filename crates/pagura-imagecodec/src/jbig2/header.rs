//! Segment headers and the framing of segment data (T.88 7.2).
use super::arithmetic::TERMINATOR;
use super::generic::GenericFlags;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Result};

/// The segment data length that leaves the length to the data itself
/// (T.88 7.2.7).
const UNKNOWN_LENGTH: u32 = u32::MAX;

/// The bytes that end MMR data of unknown length, before its row count
/// (T.88 7.2.7).
const MMR_END: [u8; 2] = [0, 0];

/// The length of region segment information (T.88 7.4.1).
const REGION_INFO_BYTES: usize = 17;

/// A segment type (T.88 7.3).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SegmentType {
    SymbolDictionary = 0,
    IntermediateText = 4,
    ImmediateText = 6,
    ImmediateLosslessText = 7,
    PatternDictionary = 16,
    IntermediateHalftone = 20,
    ImmediateHalftone = 22,
    ImmediateLosslessHalftone = 23,
    IntermediateGeneric = 36,
    ImmediateGeneric = 38,
    ImmediateLosslessGeneric = 39,
    IntermediateRefinement = 40,
    ImmediateRefinement = 42,
    ImmediateLosslessRefinement = 43,
    PageInformation = 48,
    EndOfPage = 49,
    EndOfStripe = 50,
    EndOfFile = 51,
    Profiles = 52,
    Tables = 53,
    ColorPalette = 54,
    Extension = 62,
}

impl TryFrom<u8> for SegmentType {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            0 => Self::SymbolDictionary,
            4 => Self::IntermediateText,
            6 => Self::ImmediateText,
            7 => Self::ImmediateLosslessText,
            16 => Self::PatternDictionary,
            20 => Self::IntermediateHalftone,
            22 => Self::ImmediateHalftone,
            23 => Self::ImmediateLosslessHalftone,
            36 => Self::IntermediateGeneric,
            38 => Self::ImmediateGeneric,
            39 => Self::ImmediateLosslessGeneric,
            40 => Self::IntermediateRefinement,
            42 => Self::ImmediateRefinement,
            43 => Self::ImmediateLosslessRefinement,
            48 => Self::PageInformation,
            49 => Self::EndOfPage,
            50 => Self::EndOfStripe,
            51 => Self::EndOfFile,
            52 => Self::Profiles,
            53 => Self::Tables,
            54 => Self::ColorPalette,
            62 => Self::Extension,
            _ => return Err(Error::syntax("reserved JBIG2 segment type")),
        })
    }
}

/// What a region segment decodes (T.88 7.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Region {
    Text,
    Halftone,
    Generic,
    Refinement,
}

/// Where the bitmap of a region segment goes (T.88 7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Destination {
    /// An intermediate region, kept for a later refinement region.
    Stored,
    /// An immediate region, lossless or not, drawn on the page.
    Page,
}

impl SegmentType {
    /// Whether the segment is a refinement region.
    pub(super) const fn is_refinement(self) -> bool {
        matches!(
            self,
            Self::IntermediateRefinement
                | Self::ImmediateRefinement
                | Self::ImmediateLosslessRefinement
        )
    }

    /// Whether the segment may stand in the globals, which belong to no
    /// page (7.4.7).
    pub(super) const fn is_global(self) -> bool {
        matches!(
            self,
            Self::SymbolDictionary
                | Self::PatternDictionary
                | Self::Tables
                | Self::Profiles
                | Self::Extension
        )
    }

    /// Whether the segment may refer to other segments: dictionaries,
    /// tables and patterns that a symbol dictionary or a text, halftone or
    /// refinement region uses, or the segment an extension extends.
    pub(super) const fn takes_referrals(self) -> bool {
        self.is_refinement()
            || matches!(
                self,
                Self::SymbolDictionary
                    | Self::IntermediateText
                    | Self::ImmediateText
                    | Self::ImmediateLosslessText
                    | Self::IntermediateHalftone
                    | Self::ImmediateHalftone
                    | Self::ImmediateLosslessHalftone
                    | Self::Extension
            )
    }
}

/// The segment header flags (T.88 7.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SegmentFlags(pub(super) u8);

impl SegmentFlags {
    /// Bits 0–5: the segment type.
    const TYPE: u8 = 0x3f;
    /// Bit 6: the page association field is four bytes long, not one.
    const LONG_PAGE_ASSOCIATION: u8 = 0x40;
    /// Bit 7: the segment is retained until the segment after it.
    const DEFERRED_NON_RETAIN: u8 = 0x80;

    pub(super) fn kind(self) -> Result<SegmentType> {
        SegmentType::try_from(self.0 & Self::TYPE)
    }

    pub(super) const fn long_page_association(self) -> bool {
        self.0 & Self::LONG_PAGE_ASSOCIATION != 0
    }

    pub(super) const fn deferred_non_retain(self) -> bool {
        self.0 & Self::DEFERRED_NON_RETAIN != 0
    }
}

/// The referred-to segment count and retention flags of a segment header
/// (T.88 7.2.4).
pub(super) struct Referrals<'a> {
    count: usize,
    /// Bit 0 retains this segment, bit `i` its `i`-th referred-to segment.
    retention: &'a [u8],
}

impl<'a> Referrals<'a> {
    /// Bits 5–7 of the first byte: the count, or the long form.
    const COUNT_SHIFT: u32 = 5;
    /// The count field of the long form, whose count follows.
    const LONG_FORM: u8 = 7;
    /// Bits 0–4 of the first byte: the retention flags of the short form,
    /// or the top bits of the long form's count.
    const LOW_BITS: u8 = 0x1f;

    pub(super) fn read(cursor: &mut Cursor<'a>) -> Result<Self> {
        let start = cursor.offset();
        let first = cursor.u8()?;
        match first >> Self::COUNT_SHIFT {
            short @ 0..=4 => {
                let count = usize::from(short);
                if first & Self::LOW_BITS >= 1 << (count + 1) {
                    return Err(Error::syntax("nonzero unused JBIG2 retention flags"));
                }
                let retention = cursor.data().get(start..=start).unwrap_or_default();
                Ok(Self { count, retention })
            }
            Self::LONG_FORM => {
                let [a, b, c] = cursor.take_array()?;
                let count = u32_to_usize(u32::from_be_bytes([first & Self::LOW_BITS, a, b, c]));
                if count < 5 {
                    return Err(Error::syntax("noncanonical JBIG2 referral count"));
                }
                // Retention bits for the segment and each referral, padded to bytes.
                let retention = cursor.take(count / 8 + 1)?;
                let used = (count + 1) % 8;
                if used != 0 && retention.last().is_some_and(|last| last >> used != 0) {
                    return Err(Error::syntax("nonzero unused JBIG2 retention flags"));
                }
                Ok(Self { count, retention })
            }
            _ => Err(Error::syntax("reserved JBIG2 referral count")),
        }
    }

    /// The number of referred-to segments.
    pub(super) const fn count(&self) -> usize {
        self.count
    }

    /// Whether the segment (index 0) or its referred-to segment `index`
    /// (counting from 1) stays available to later segments.
    pub(super) fn retains(&self, index: usize) -> bool {
        self.retention
            .get(index / 8)
            .is_some_and(|byte| byte & (1 << (index % 8)) != 0)
    }
}

/// Takes the data of a segment of type `kind` whose header gave `length`.
///
/// An immediate generic region may leave its length unknown (T.88 7.2.7);
/// its data then ends with a marker and a row count (T.88 7.4.6.4), which is
/// returned with the data.
pub(super) fn data<'a>(
    cursor: &mut Cursor<'a>,
    kind: SegmentType,
    length: u32,
) -> Result<(Cursor<'a>, Option<u32>)> {
    if length != UNKNOWN_LENGTH {
        return Ok((cursor.take_cursor(u32_to_usize(length))?, None));
    }
    if kind != SegmentType::ImmediateGeneric {
        return Err(cursor.error("unspecified length of JBIG2 non-generic segment"));
    }
    let rest = cursor.rest();
    // The region information precedes the generic region flags.
    let flags = GenericFlags(
        *rest
            .get(REGION_INFO_BYTES)
            .ok_or_else(|| cursor.error("truncated JBIG2 unknown-length region"))?,
    );
    let arithmetic = !flags.mmr();
    let header = REGION_INFO_BYTES + 1 + flags.adaptive_bytes();
    let marker = if arithmetic { TERMINATOR } else { MMR_END };
    let tail = rest
        .get(header..)
        .ok_or_else(|| cursor.error("truncated JBIG2 unknown-length header"))?;
    let end = tail
        .windows(2)
        .position(|pair| pair == marker)
        .ok_or_else(|| cursor.error("unterminated JBIG2 unknown-length region"))?
        + header;
    // The arithmetic terminator belongs to the coded data; the MMR marker
    // only ends it.
    let body = cursor.take_cursor(end + if arithmetic { marker.len() } else { 0 })?;
    if !arithmetic {
        cursor.skip(marker.len())?;
    }
    let rows = cursor.u32()?;
    Ok((body, Some(rows)))
}

/// Requires that a segment's decoder consumed all of its data.
pub(super) fn finish(body: &Cursor<'_>) -> Result<()> {
    if body.at_end() {
        Ok(())
    } else {
        Err(body.error("trailing JBIG2 segment data"))
    }
}
