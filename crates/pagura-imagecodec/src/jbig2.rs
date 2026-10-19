//! JBIG2 embedded streams (7.4.7; T.88 D.3). Decoding returns packed
//! black-is-one samples; the PDF image adapter is responsible for PDF sample
//! polarity.
//!
//! Implements the bitonal coding procedures of T.88 (02/2000), including
//! arithmetic and Huffman symbols/text, generic/refinement regions, patterns,
//! and halftones. Later extended adaptive templates and necessary extensions
//! are explicitly unsupported. The public decoder consumes PDF page 1;
//! standalone file headers and end-of-page and end-of-file segments are
//! forbidden by 7.4.7. Bare clause numbers here refer to ISO 32000-2.

mod arithmetic;
mod bitmap;
mod generic;
mod halftone;
mod header;
mod huffman;
mod integer;
mod mmr;
mod refinement;
mod symbol;
#[cfg(test)]
mod tests;
mod text;

use crate::budget::{Budget, Messages};
use bitmap::{CombinationOp, Fill};
use header::{Destination, Region, SegmentFlags, SegmentType};
use mmr::Eofb;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Position, Result, ResultExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

/// A decoded page bitmap: 1-bit rows, most significant bit first, each
/// padded to whole bytes, 1 = black. Padding bits in every row are zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    width: u32,
    height: u32,
    /// Bytes per row: `width` divided by 8, rounded up.
    stride: usize,
    /// `stride × height` bytes.
    data: Vec<u8>,
}

impl Bitmap {
    /// Width in pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Bytes per row.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// The packed rows.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The packed rows, without copying them.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }
}

/// Decodes PDF page 1 and its optional `/JBIG2Globals` stream.
///
/// One budget covers the whole decode, globals included (ADR 0016):
///
/// - `decode_work` bounds the work: one unit per segment and per referral,
///   per symbol height class, export run, imported symbol, text instance,
///   pattern, profile and Huffman table line, eighteen units per pixel
///   decoded, refined or composited, one per value an allocation
///   initializes and one per MMR bit read;
/// - `decode_bytes` bounds the working memory held at any one time. What
///   later segments may use stays charged until the decode ends: segment
///   records, symbol and pattern dictionaries, intermediate regions, code
///   tables, retained contexts and the profiles of the globals. What one
///   segment uses is charged before it is allocated and released when the
///   segment is done with it: referral lists, imported symbol tables,
///   contexts, Huffman tables built for the segment, MMR row lists, and
///   region, plane and refined-symbol bitmaps;
/// - `decoded_bytes` bounds the page bitmap, the output;
/// - `pixels` bounds every bitmap, the page and each intermediate one.
///
/// No partial page is returned on an error.
///
/// # Errors
///
/// Returns positioned syntax errors for malformed embedded streams, cycle
/// errors for self-referrals, explicit unsupported tags for unimplemented
/// coding modes, and limit errors before oversized allocations or work.
pub fn decode(data: &[u8], globals: Option<&[u8]>, limits: &Limits) -> Result<Bitmap> {
    let mut state = State {
        limits,
        budget: Budget::new(limits, MESSAGES),
        page: None,
        segments: BTreeMap::new(),
        unavailable: BTreeSet::new(),
        used_regions: BTreeSet::new(),
        profiles: None,
        page_profile: false,
        deferred: BTreeSet::new(),
        attached: None,
    };
    if let Some(globals) = globals {
        state.stream(globals, Stream::Globals)?;
        // 7.4.7 and Table 12 supply globals independently of each page stream.
        // Their decoded resources remain available for every page using them;
        // retention still governs referrals within either individual stream.
        state.unavailable.clear();
        state.attached = None;
    }
    state.stream(data, Stream::Page)?;
    let page = state.page.ok_or_else(|| {
        Error::syntax("missing JBIG2 page information").at(Position::Buffer(data.len()))
    })?;
    if page.unknown_height
        && (page.last_stripe != Some(page.bitmap.height.saturating_sub(1))
            || page.bitmap.height == 0)
    {
        return Err(Error::syntax("unfinished JBIG2 striped page").at(Position::Buffer(data.len())));
    }
    Ok(page.bitmap)
}

/// Which of the two PDF streams a segment comes from (7.4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    /// The `/JBIG2Globals` stream, whose segments belong to no page.
    Globals,
    /// The image stream, whose segments belong to page 1.
    Page,
}

impl Stream {
    /// The page association of the stream's segments.
    const fn page(self) -> u32 {
        match self {
            Self::Globals => 0,
            Self::Page => 1,
        }
    }
}

struct Page {
    bitmap: Bitmap,
    flags: PageFlags,
    unknown_height: bool,
    stripe_size: Option<u32>,
    last_stripe: Option<u32>,
    stripe_bottom: u32,
}

/// The page height that leaves the height to the end-of-stripe segments
/// (T.88 7.4.8.2).
const UNKNOWN_HEIGHT: u32 = u32::MAX;

/// Page segment flags (T.88 7.4.8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PageFlags(u8);

impl PageFlags {
    /// Bit 1: the page might contain refinement regions.
    const REFINEMENTS: u8 = 0x02;
    /// Bit 2: the default pixel value.
    const DEFAULT_PIXEL: u8 = 0x04;
    /// Bits 3–4: the default combination operator.
    const OPERATOR_SHIFT: u32 = 3;
    /// Bit 5: the page requires auxiliary buffers, for intermediate regions.
    const AUXILIARY_BUFFERS: u8 = 0x20;
    /// Bit 6: regions may override the default combination operator.
    const OPERATOR_OVERRIDDEN: u8 = 0x40;
    /// Bit 7: reserved in T.88 (02/2000).
    const RESERVED: u8 = 0x80;

    const fn fill(self) -> Fill {
        Fill::from_flag(self.0 & Self::DEFAULT_PIXEL != 0)
    }

    const fn operator(self) -> CombinationOp {
        CombinationOp::from_two_bits(u16::from_be_bytes([0, self.0 >> Self::OPERATOR_SHIFT]))
    }

    const fn operator_overridden(self) -> bool {
        self.0 & Self::OPERATOR_OVERRIDDEN != 0
    }

    const fn refinements(self) -> bool {
        self.0 & Self::REFINEMENTS != 0
    }

    const fn auxiliary_buffers(self) -> bool {
        self.0 & Self::AUXILIARY_BUFFERS != 0
    }
}

/// Bit 15 of the page striping information: the page is striped, in
/// stripes of at most the low 15 bits' rows (T.88 7.4.8.6).
const STRIPED: u16 = 0x8000;

enum Segment {
    Dictionary(symbol::Dictionary),
    Region(StoredRegion),
    Table(Rc<huffman::Table>),
    Patterns(halftone::Dictionary),
    Metadata,
}

/// Region segment information (T.88 7.4.1).
#[derive(Clone, Copy, PartialEq, Eq)]
struct RegionInfo {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    operator: CombinationOp,
}

/// Region segment information flags (T.88 7.4.1.5).
#[derive(Clone, Copy)]
struct RegionFlags(u8);

impl RegionFlags {
    /// Bits 0–2: the external combination operator. PDF leaves the other
    /// bits, among them the color extension flag, zero.
    const OPERATOR: u8 = 0x07;

    fn operator(self) -> Option<CombinationOp> {
        if self.0 & !Self::OPERATOR != 0 {
            return None;
        }
        CombinationOp::try_from(self.0).ok()
    }
}

struct StoredRegion {
    bitmap: Rc<Bitmap>,
    info: RegionInfo,
}

/// The `Limit` messages of a JBIG2 decode.
const MESSAGES: Messages = Messages {
    work: "JBIG2 decode work exceeds budget",
    memory: "JBIG2 working memory exceeds budget",
    output: "JBIG2 page exceeds the decoded byte budget",
};

/// The working memory one stored segment holds: its map entry and its
/// number in each of the three availability sets.
const SEGMENT_BYTES: usize = size_of::<(u32, Segment)>() + 3 * size_of::<u32>();

struct State<'a> {
    limits: &'a Limits,
    budget: Budget,
    page: Option<Page>,
    // Storage remains charged until completion; availability still follows
    // T.88 7.2.4.
    segments: BTreeMap<u32, Segment>,
    unavailable: BTreeSet<u32>,
    used_regions: BTreeSet<u32>,
    /// The sorted profiles of the globals.
    profiles: Option<Vec<u32>>,
    page_profile: bool,
    deferred: BTreeSet<u32>,
    attached: Option<u32>,
}

/// The identification string of a standalone JBIG2 file (T.88 D.4.1).
const FILE_ID: [u8; 8] = [0x97, 0x4a, 0x42, 0x32, 0x0d, 0x0a, 0x1a, 0x0a];

impl State<'_> {
    fn stream(&mut self, data: &[u8], stream: Stream) -> Result<()> {
        let mut cursor = Cursor::new(data).with_message("truncated JBIG2 segment data");
        if data.starts_with(&FILE_ID) {
            return Err(cursor.error("JBIG2 file headers are forbidden in PDF"));
        }
        while !cursor.at_end() {
            let position = cursor.position();
            self.segment(&mut cursor, stream).at(position)?;
        }
        Ok(())
    }

    fn segment(&mut self, cursor: &mut Cursor<'_>, stream: Stream) -> Result<()> {
        self.budget.work(1)?;
        self.budget.memory::<u8>(SEGMENT_BYTES)?;
        let number = cursor.u32()?;
        if self.segments.contains_key(&number) {
            return Err(Error::syntax("invalid or duplicate JBIG2 segment number"));
        }
        let flags = SegmentFlags(cursor.u8()?);
        let kind = flags.kind()?;
        let referrals = header::Referrals::read(cursor)?;
        let count = referrals.count();
        self.budget.work(count)?;
        let mut references = Vec::new();
        self.budget.reserve(&mut references, count)?;
        if kind.is_refinement() && count > 1 {
            return Err(Error::syntax(
                "JBIG2 refinement requires at most one reference",
            ));
        }
        for _ in 0..count {
            // Referrals are as wide as the segment number needs (T.88 7.2.5).
            let reference = if number <= 256 {
                u32::from(cursor.u8()?)
            } else if number <= 65536 {
                u32::from(cursor.u16()?)
            } else {
                cursor.u32()?
            };
            if reference == number {
                return Err(Error::cycle("JBIG2 segment refers to itself"));
            }
            if reference > number
                || !self.segments.contains_key(&reference)
                || self.unavailable.contains(&reference)
            {
                return Err(Error::syntax("JBIG2 referral is not an earlier segment"));
            }
            if self.deferred.contains(&reference)
                && (kind != SegmentType::Extension
                    || count != 1
                    || self.attached != Some(reference))
            {
                return Err(Error::syntax("invalid JBIG2 deferred extension referral"));
            }
            if kind != SegmentType::Extension
                && matches!(self.segments.get(&reference), Some(Segment::Region(_)))
                && !self.used_regions.insert(reference)
            {
                return Err(Error::syntax(
                    "JBIG2 intermediate region referred to more than once",
                ));
            }
            references.push(reference);
        }
        let page = if flags.long_page_association() {
            cursor.u32()?
        } else {
            u32::from(cursor.u8()?)
        };
        if page != stream.page() {
            return Err(Error::syntax("invalid JBIG2 embedded page association"));
        }
        let length = cursor.u32()?;
        let (mut body, actual_rows) = header::data(cursor, kind, length)?;
        let position = body.position();
        let segment = self
            .body(kind, stream, &references, &mut body, actual_rows)
            .at(position)?;
        header::finish(&body)?;
        for (index, n) in std::iter::once(&number)
            .chain(references.iter())
            .enumerate()
        {
            if !referrals.retains(index) {
                self.unavailable.insert(*n);
            }
        }
        if flags.deferred_non_retain() {
            self.deferred.insert(number);
            self.attached = Some(number);
        } else if kind != SegmentType::Extension
            || references.len() != 1
            || self.attached != references.first().copied()
        {
            self.attached = None;
        }
        self.segments.insert(number, segment);
        self.budget.release::<u32>(references.len());
        Ok(())
    }

    fn body(
        &mut self,
        kind: SegmentType,
        stream: Stream,
        references: &[u32],
        cursor: &mut Cursor<'_>,
        actual_rows: Option<u32>,
    ) -> Result<Segment> {
        match stream {
            Stream::Globals if !kind.is_global() => {
                return Err(Error::syntax("page segment in JBIG2 globals"));
            }
            // Segments that draw on the page find it themselves.
            Stream::Page if self.page.is_none() && kind.is_global() => return Err(no_page()),
            Stream::Globals | Stream::Page => {}
        }
        if !references.is_empty() && !kind.takes_referrals() {
            return Err(Error::syntax("unexpected JBIG2 segment referrals"));
        }
        let (region, destination) = match kind {
            SegmentType::SymbolDictionary => return self.dictionary(cursor, references),
            SegmentType::PatternDictionary => {
                let patterns = halftone::dictionary(cursor, self.limits, &mut self.budget)?;
                return Ok(Segment::Patterns(patterns));
            }
            SegmentType::Tables => {
                let table = huffman::custom(cursor, &mut self.budget)?;
                return Ok(Segment::Table(Rc::new(table)));
            }
            SegmentType::PageInformation => {
                self.page_info(cursor)?;
                return Ok(Segment::Metadata);
            }
            SegmentType::EndOfStripe => {
                self.end_stripe(cursor)?;
                return Ok(Segment::Metadata);
            }
            SegmentType::Profiles => {
                self.profiles(cursor, stream)?;
                return Ok(Segment::Metadata);
            }
            SegmentType::Extension => {
                extension(cursor)?;
                return Ok(Segment::Metadata);
            }
            SegmentType::ColorPalette => {
                return Err(Error::syntax("JBIG2 color palettes are forbidden in PDF"));
            }
            SegmentType::EndOfPage | SegmentType::EndOfFile => {
                return Err(Error::syntax(
                    "JBIG2 end-of-page and end-of-file segments are forbidden in PDF",
                ));
            }
            SegmentType::IntermediateText => (Region::Text, Destination::Stored),
            SegmentType::ImmediateText | SegmentType::ImmediateLosslessText => {
                (Region::Text, Destination::Page)
            }
            SegmentType::IntermediateHalftone => (Region::Halftone, Destination::Stored),
            SegmentType::ImmediateHalftone | SegmentType::ImmediateLosslessHalftone => {
                (Region::Halftone, Destination::Page)
            }
            SegmentType::IntermediateGeneric => (Region::Generic, Destination::Stored),
            SegmentType::ImmediateGeneric | SegmentType::ImmediateLosslessGeneric => {
                (Region::Generic, Destination::Page)
            }
            SegmentType::IntermediateRefinement => (Region::Refinement, Destination::Stored),
            SegmentType::ImmediateRefinement | SegmentType::ImmediateLosslessRefinement => {
                (Region::Refinement, Destination::Page)
            }
        };
        let stored = self.region(cursor, region, destination, references, actual_rows)?;
        Ok(stored.map_or(Segment::Metadata, Segment::Region))
    }

    fn dictionary(&mut self, cursor: &mut Cursor<'_>, references: &[u32]) -> Result<Segment> {
        let imported = imported(&self.segments, references, &mut self.budget)?;
        let flags = symbol::SymbolFlags::read(cursor)?;
        let tables = if flags.huffman() {
            tables(&self.segments, references, 4)?
        } else {
            if references
                .iter()
                .any(|n| matches!(self.segments.get(n), Some(Segment::Table(_))))
            {
                return Err(Error::syntax(
                    "Huffman table referred to by arithmetic dictionary",
                ));
            }
            Vec::new()
        };
        let retained = references
            .iter()
            .rev()
            .filter_map(|n| self.segments.get(n))
            .find_map(|s| match s {
                Segment::Dictionary(dictionary) => Some(dictionary.contexts.as_ref()),
                _ => None,
            })
            .flatten();
        let dictionary = if flags.huffman() {
            symbol::huffman::decode(
                cursor,
                flags,
                &imported,
                &tables,
                retained,
                self.limits,
                &mut self.budget,
            )?
        } else {
            symbol::decode(
                cursor,
                flags,
                &imported,
                retained,
                self.limits,
                &mut self.budget,
            )?
        };
        self.budget.release::<Rc<Bitmap>>(imported.len());
        Ok(Segment::Dictionary(dictionary))
    }

    fn profiles(&mut self, cursor: &mut Cursor<'_>, stream: Stream) -> Result<()> {
        let in_order = match stream {
            // Only the first segment of the globals lists profiles, and the
            // page stream may then list a superset once.
            Stream::Globals => self.segments.is_empty(),
            Stream::Page => self.profiles.is_some() && !self.page_profile,
        };
        if !in_order {
            return Err(Error::syntax("invalid JBIG2 profiles segment order"));
        }
        let count = u32_to_usize(cursor.u32()?);
        self.budget.work(count)?;
        let mut profiles = Vec::new();
        self.budget.reserve(&mut profiles, count)?;
        for _ in 0..count {
            profiles.push(cursor.u32()?);
        }
        profiles.sort_unstable();
        match (stream, &self.profiles) {
            (Stream::Page, Some(first))
                if first
                    .iter()
                    .any(|profile| profiles.binary_search(profile).is_err()) =>
            {
                return Err(Error::syntax("JBIG2 page profile omits global profile"));
            }
            (Stream::Page, _) => {
                self.page_profile = true;
                self.budget.release::<u32>(count);
            }
            (Stream::Globals, _) => self.profiles = Some(profiles),
        }
        Ok(())
    }

    fn page_info(&mut self, cursor: &mut Cursor<'_>) -> Result<()> {
        if self.page.is_some() {
            return Err(Error::syntax("duplicate JBIG2 page information"));
        }
        let width = cursor.u32()?;
        let height = cursor.u32()?;
        cursor.skip(8)?; // Resolution fields do not change decoded samples.
        let flags = PageFlags(cursor.u8()?);
        let striping = cursor.u16()?;
        if width == 0 || height == 0 || flags.0 & PageFlags::RESERVED != 0 {
            return Err(Error::syntax("invalid JBIG2 page information"));
        }
        let unknown_height = height == UNKNOWN_HEIGHT;
        let stripe_size = (striping & STRIPED != 0).then_some(u32::from(striping & !STRIPED));
        if unknown_height && stripe_size.is_none() {
            return Err(Error::syntax("unknown JBIG2 page height requires stripes"));
        }
        let rows = if unknown_height { 0 } else { height };
        // Width must itself fit the pixel budget even for an empty growing page.
        bitmap::layout(width, rows.max(1), self.limits)?;
        bitmap::charge_pixels(&mut self.budget, u64::from(width) * u64::from(rows))?;
        let bitmap = Bitmap::page(width, rows, flags.fill(), self.limits, &mut self.budget)?;
        self.page = Some(Page {
            bitmap,
            flags,
            unknown_height,
            stripe_size,
            last_stripe: None,
            stripe_bottom: 0,
        });
        Ok(())
    }

    fn end_stripe(&mut self, cursor: &mut Cursor<'_>) -> Result<()> {
        let line = cursor.u32()?;
        let page = self.page.as_mut().ok_or_else(no_page)?;
        let maximum = page
            .stripe_size
            .ok_or_else(|| Error::syntax("stripe on unstriped JBIG2 page"))?;
        if page.last_stripe.is_some_and(|last| line <= last)
            || line.saturating_sub(page.last_stripe.unwrap_or(0)) > maximum
            || page.stripe_bottom > line.saturating_add(1)
        {
            return Err(Error::syntax("invalid JBIG2 stripe boundary"));
        }
        let height = line
            .checked_add(1)
            .ok_or_else(|| Error::limit("JBIG2 stripe height overflow"))?;
        if !page.unknown_height && height > page.bitmap.height {
            return Err(Error::syntax("JBIG2 stripe exceeds page height"));
        }
        page.grow(height, self.limits, &mut self.budget)?;
        page.last_stripe = Some(line);
        Ok(())
    }

    fn region(
        &mut self,
        cursor: &mut Cursor<'_>,
        region: Region,
        destination: Destination,
        references: &[u32],
        actual_rows: Option<u32>,
    ) -> Result<Option<StoredRegion>> {
        let info = RegionInfo::read(cursor, actual_rows)?;
        let page = self.page.as_mut().ok_or_else(no_page)?;
        page.check_region(&info, region, destination, references)?;
        if region == Region::Refinement {
            check_refinement(&self.segments, &info, destination, references)?;
        }
        if destination == Destination::Page && page.unknown_height {
            let bottom = info
                .y
                .checked_add(info.height)
                .ok_or_else(|| Error::limit("JBIG2 region coordinate overflow"))?;
            page.grow(bottom, self.limits, &mut self.budget)?;
        }
        bitmap::layout(info.width, info.height, self.limits)?;
        bitmap::charge_pixels(
            &mut self.budget,
            u64::from(info.width) * u64::from(info.height),
        )?;
        let (segments, limits, budget) = (&self.segments, self.limits, &mut self.budget);
        let size = (info.width, info.height);
        let decoded = match region {
            Region::Text => text_region(cursor, size, references, segments, limits, budget)?,
            Region::Refinement => {
                let reference = match references.first() {
                    None => Reference::Page(&page.bitmap, (info.x, info.y)),
                    Some(number) => {
                        let Some(Segment::Region(stored)) = segments.get(number) else {
                            return Err(Error::syntax(
                                "JBIG2 refinement referral has wrong segment type",
                            ));
                        };
                        Reference::Region(&stored.bitmap)
                    }
                };
                refinement_region(cursor, size, reference, limits, budget)?
            }
            Region::Halftone => {
                let [number] = references else {
                    return Err(Error::syntax(
                        "JBIG2 halftone requires one pattern dictionary",
                    ));
                };
                let Some(Segment::Patterns(dictionary)) = segments.get(number) else {
                    return Err(Error::syntax("wrong JBIG2 halftone reference type"));
                };
                halftone::region(cursor, size, dictionary, limits, budget)?
            }
            // MMR data of unknown length ends with an end-of-block code
            // (T.88 7.2.7).
            Region::Generic => {
                let eofb = if actual_rows.is_some() {
                    Eofb::Required
                } else {
                    Eofb::Optional
                };
                generic::decode(cursor, size, eofb, limits, budget)?
            }
        };
        match destination {
            Destination::Page => {
                if let Some(bottom) = page.bitmap.combine_page(
                    &decoded,
                    info.x,
                    info.y,
                    info.operator,
                    page.last_stripe,
                )? {
                    page.stripe_bottom = page.stripe_bottom.max(bottom);
                }
                decoded.release(budget);
                Ok(None)
            }
            Destination::Stored => Ok(Some(StoredRegion {
                bitmap: bitmap::share(decoded, budget)?,
                info,
            })),
        }
    }
}

impl RegionInfo {
    /// Reads region segment information; an immediate generic region of
    /// unknown length has `actual_rows` rows instead of the declared ones.
    fn read(cursor: &mut Cursor<'_>, actual_rows: Option<u32>) -> Result<Self> {
        let width = cursor.u32()?;
        let declared_height = cursor.u32()?;
        let height = actual_rows.unwrap_or(declared_height);
        if height > declared_height {
            return Err(Error::syntax(
                "JBIG2 actual row count exceeds declared height",
            ));
        }
        let x = cursor.u32()?;
        let y = cursor.u32()?;
        let operator = RegionFlags(cursor.u8()?).operator();
        let Some(operator) = operator.filter(|_| width != 0 && height != 0) else {
            return Err(Error::syntax("invalid JBIG2 region information"));
        };
        Ok(Self {
            width,
            height,
            x,
            y,
            operator,
        })
    }
}

/// Decodes a text region (T.88 7.4.3) with the symbols and tables of the
/// segments it refers to.
fn text_region(
    cursor: &mut Cursor<'_>,
    size: (u32, u32),
    references: &[u32],
    segments: &BTreeMap<u32, Segment>,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    let imported = imported(segments, references, budget)?;
    let flags = text::TextFlags::read(cursor)?;
    let bitmap = if flags.huffman() {
        let tables = tables(segments, references, 8)?;
        text::huffman::decode(cursor, flags, size, &imported, &tables, limits, budget)?
    } else {
        if references
            .iter()
            .any(|n| matches!(segments.get(n), Some(Segment::Table(_))))
        {
            return Err(Error::syntax(
                "Huffman table referred to by arithmetic text",
            ));
        }
        text::decode(cursor, flags, size, &imported, limits, budget)?
    };
    budget.release::<Rc<Bitmap>>(imported.len());
    Ok(bitmap)
}

/// The error for a page-stream segment before the page information.
fn no_page() -> Error {
    Error::syntax("JBIG2 page information must precede associated segments")
}

impl Page {
    /// Checks a region against what the page information allows. Unless
    /// the page lets regions override it, only an immediate refinement of
    /// the page itself may use another operator than the page's default.
    fn check_region(
        &self,
        info: &RegionInfo,
        region: Region,
        destination: Destination,
        references: &[u32],
    ) -> Result<()> {
        let refines_page = region == Region::Refinement
            && destination == Destination::Page
            && references.is_empty();
        if !self.flags.operator_overridden()
            && info.operator != self.flags.operator()
            && !refines_page
        {
            return Err(Error::syntax(
                "JBIG2 direct region operator differs from fixed page default",
            ));
        }
        if destination == Destination::Stored && !self.flags.auxiliary_buffers() {
            return Err(Error::syntax("JBIG2 page forbids auxiliary regions"));
        }
        if region == Region::Refinement && !self.flags.refinements() {
            return Err(Error::syntax("JBIG2 page forbids refinement regions"));
        }
        Ok(())
    }

    /// Grows a page of unknown height to at least `height` rows.
    fn grow(&mut self, height: u32, limits: &Limits, budget: &mut Budget) -> Result<()> {
        if !self.unknown_height || height <= self.bitmap.height {
            return Ok(());
        }
        self.bitmap.grow(height, self.flags.fill(), limits, budget)
    }
}

/// Checks a refinement region against the region it refines, if any: the
/// page with the REPLACE operator, or an intermediate region with the same
/// information (T.88 7.4.7).
fn check_refinement(
    segments: &BTreeMap<u32, Segment>,
    info: &RegionInfo,
    destination: Destination,
    references: &[u32],
) -> Result<()> {
    match references {
        [] if destination == Destination::Page && info.operator == CombinationOp::Replace => Ok(()),
        [number] => {
            let Some(Segment::Region(reference)) = segments.get(number) else {
                return Err(Error::syntax("invalid JBIG2 refinement reference"));
            };
            if *info == reference.info {
                Ok(())
            } else {
                Err(Error::syntax(
                    "JBIG2 refinement region information differs from its reference",
                ))
            }
        }
        _ => Err(Error::syntax(
            "invalid JBIG2 refinement reference or operator",
        )),
    }
}

/// The Huffman tables among the referred-to segments, at most `maximum`.
fn tables(
    segments: &BTreeMap<u32, Segment>,
    references: &[u32],
    maximum: usize,
) -> Result<Vec<Rc<huffman::Table>>> {
    let tables: Vec<_> = references
        .iter()
        .filter_map(|n| match segments.get(n) {
            Some(Segment::Table(table)) => Some(Rc::clone(table)),
            _ => None,
        })
        .take(maximum + 1)
        .collect();
    if tables.len() > maximum {
        return Err(Error::syntax("too many JBIG2 table references"));
    }
    Ok(tables)
}

/// The symbols of the referred-to symbol dictionaries, in order (T.88
/// 7.4.2.2, 7.4.3.2); referred-to Huffman tables are skipped.
fn imported(
    segments: &BTreeMap<u32, Segment>,
    references: &[u32],
    budget: &mut Budget,
) -> Result<Vec<Rc<Bitmap>>> {
    let mut count = 0_usize;
    for reference in references {
        match segments.get(reference) {
            Some(Segment::Dictionary(dictionary)) => {
                count = count
                    .checked_add(dictionary.symbols.len())
                    .ok_or_else(|| Error::limit("JBIG2 imported symbol count overflow"))?;
            }
            Some(Segment::Table(_)) => {}
            _ => {
                return Err(Error::syntax(
                    "JBIG2 symbol referral has wrong segment type",
                ));
            }
        }
    }
    budget.work(count)?;
    let mut imported = Vec::new();
    budget.reserve(&mut imported, count)?;
    for reference in references {
        if let Some(Segment::Dictionary(dictionary)) = segments.get(reference) {
            imported.extend(dictionary.symbols.iter().cloned());
        }
    }
    Ok(imported)
}

/// The bitmap a refinement region refines.
#[derive(Clone, Copy)]
enum Reference<'a> {
    /// The page, clipped to the region at this origin (T.88 7.4.7.4).
    Page(&'a Bitmap, (u32, u32)),
    /// An intermediate region.
    Region(&'a Bitmap),
}

/// Decodes a refinement region (T.88 7.4.7).
fn refinement_region(
    cursor: &mut Cursor<'_>,
    size: (u32, u32),
    reference: Reference<'_>,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    let flags = refinement::RefinementFlags(cursor.u8()?);
    if !flags.is_valid() {
        return Err(Error::syntax("reserved JBIG2 refinement flags"));
    }
    let parameters = refinement::Parameters::read(cursor, flags.template())?;
    let mut model = refinement::Model::new(parameters, None, budget)?;
    let mut decoder = arithmetic::Decoder::new(cursor)?;
    let request = refinement::Request {
        width: size.0,
        height: size.1,
        dx: 0,
        dy: 0,
        prediction: flags.prediction(),
    };
    let bitmap = match reference {
        Reference::Page(page, origin) => {
            model.decode_page(&mut decoder, page, origin, request, limits, budget)?
        }
        Reference::Region(region) => model.decode(&mut decoder, region, request, limits, budget)?,
    };
    model.release(budget);
    Ok(bitmap)
}

/// The type of an extension segment (T.88 7.4.14.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExtensionType(u32);

impl ExtensionType {
    /// Bit 31: a decoder that does not know the extension cannot decode
    /// the page.
    const NECESSARY: u32 = 0x8000_0000;
    /// Bit 29, which a necessary extension must set.
    const RESERVED: u32 = 0x2000_0000;

    const fn necessary(self) -> bool {
        self.0 & Self::NECESSARY != 0
    }

    const fn reserved(self) -> bool {
        self.0 & Self::RESERVED != 0
    }
}

/// Unrecognized extensions that are not necessary do not alter the bitmap
/// (T.88 7.4.14).
fn extension(cursor: &mut Cursor<'_>) -> Result<()> {
    let kind = ExtensionType(cursor.u32()?);
    if kind.necessary() {
        if !kind.reserved() {
            return Err(Error::syntax("invalid necessary JBIG2 extension flags"));
        }
        return Err(Error::unsupported(
            crate::features::JBIG2_EXTENSION,
            "unknown necessary JBIG2 extension",
        ));
    }
    cursor.take_rest();
    Ok(())
}
