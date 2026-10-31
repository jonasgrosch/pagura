//! JBIG2 embedded sequential streams (ISO 32000-2 clause 7.4.7; T.88
//! Annex D.3). Decoding returns packed black-is-one samples; the PDF image
//! adapter is responsible for PDF sample polarity.
//!
//! Implements the bitonal coding procedures of T.88 (02/2000), including
//! arithmetic and Huffman symbols/text, generic/refinement regions, patterns,
//! and halftones. Later extended adaptive templates and necessary extensions
//! are explicitly unsupported. The public decoder consumes PDF page 1;
//! standalone file headers and EOP/EOF segments are forbidden by PDF 7.4.7.
//! Strict decoding rejects them; explicitly reported recovery is limited to
//! complete declared single-page sequential wrappers with validated boundaries.

mod arithmetic;
mod bitmap;
mod budget;
mod generic;
mod halftone;
mod huffman;
mod huffman_symbol;
mod huffman_text;
mod integer;
mod mmr;
mod reader;
mod refinement;
mod symbol;
#[cfg(test)]
mod tests;
mod text;
mod wrapper;

use pagura_core::{Diagnostics, Error, Limits, Position, Recovery, Result};
use reader::Reader;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use wrapper::{Boundary, Framing, Stream};

/// Stable JBIG2 feature tags. Tags for arithmetic capabilities now supported
/// remain available for compatibility.
pub mod tags {
    use pagura_core::Feature;
    /// Compatibility tag for symbol dictionary decoding (T.88 clause 6.5).
    pub const SYMBOL_DICTIONARY: Feature = Feature::new("jbig2.symbol-dictionary");
    /// Compatibility tag for refinement and aggregate symbols.
    pub const SYMBOL_REFINEMENT: Feature = Feature::new("jbig2.symbol-refinement");
    /// Compatibility tag for retained arithmetic symbol contexts.
    pub const SYMBOL_CONTEXT: Feature = Feature::new("jbig2.symbol-context");
    /// Compatibility tag for text refinement decoding.
    pub const TEXT_REFINEMENT: Feature = Feature::new("jbig2.text-refinement");
    /// Compatibility tag for text reference corners and transposition.
    pub const TEXT_LAYOUT: Feature = Feature::new("jbig2.text-layout");
    /// Compatibility tag for text region decoding (T.88 clause 6.4).
    pub const TEXT_REGION: Feature = Feature::new("jbig2.text-region");
    /// Compatibility tag for pattern dictionaries (T.88 clause 6.7).
    pub const PATTERN_DICTIONARY: Feature = Feature::new("jbig2.pattern-dictionary");
    /// Compatibility tag for halftone regions (T.88 clause 6.6).
    pub const HALFTONE_REGION: Feature = Feature::new("jbig2.halftone-region");
    /// Compatibility tag for generic refinement.
    pub const REFINEMENT_REGION: Feature = Feature::new("jbig2.refinement-region");
    /// Compatibility tag for user Huffman tables (T.88 clause 7.4.13).
    pub const HUFFMAN_TABLE: Feature = Feature::new("jbig2.huffman-table");
    /// Unknown necessary extension segments (T.88 clause 7.4.14).
    pub const EXTENSION: Feature = Feature::new("jbig2.extension");
    /// Compatibility tag; conflicting fixed page operators are syntax errors.
    pub const PAGE_COMBINATION: Feature = Feature::new("jbig2.page-combination");
    /// Compatibility tag for profile metadata.
    pub const PROFILE: Feature = Feature::new("jbig2.profile");
    /// Compatibility tag; reserved segment types are syntax errors.
    pub const SEGMENT_TYPE: Feature = Feature::new("jbig2.segment-type");
    /// The extended adaptive template from the T.88 amendments.
    pub const EXTENDED_TEMPLATE: Feature = Feature::new("jbig2.extended-template");
    /// Compatibility tag; T.88 clause 6.2.6 forbids T.6 extensions.
    pub const MMR_EXTENSION: Feature = Feature::new("jbig2.mmr-extension");
    /// Compatibility tag for unspecified generic lengths (T.88 clause 7.2.7).
    pub const UNKNOWN_LENGTH: Feature = Feature::new("jbig2.unknown-length");
}

/// Stable repairs of malformed JBIG2 file packaging (ADR 0003).
pub mod repairs {
    use pagura_core::Repair;

    /// A complete declared single-page sequential file is decoded with its
    /// validated header/EOP/EOF boundaries (PDF 7.4.7; T.88 D.1/D.4).
    pub const FILE_WRAPPER: Repair = Repair::new("jbig2.file-wrapper");
    /// An empty, unreferenced terminal EOF associated with the sole already
    /// closed page is treated as file metadata (T.88 7.3.2/7.4.11).
    pub const EOF_PAGE_ASSOCIATION: Repair = Repair::new("jbig2.eof-page-association");

    /// Every repair owned by this decoder.
    pub const ALL: &[Repair] = &[FILE_WRAPPER, EOF_PAGE_ASSOCIATION];
}

/// A decoded page bitmap: 1-bit rows, MSB first, padded to bytes, 1 = black.
/// Padding bits in every returned row are zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed rows.
    pub data: Vec<u8>,
}

/// Decodes PDF page 1 and its optional `/JBIG2Globals` stream.
///
/// Every input and bitmap is bounded by `limits`; segment count is bounded
/// by `objects` and referrals by `value_nodes`. Pixel work, segment
/// dispatch, referral expansion, symbol height/export runs and text
/// placements share the cumulative `decode_work` budget. `decoded_bytes`
/// also bounds simultaneously live bitmaps and arithmetic contexts. No
/// partial page is returned on an error.
///
/// # Errors
///
/// Returns positioned syntax errors for malformed embedded streams, cycle
/// errors for self-referrals, explicit unsupported tags for unimplemented
/// coding modes, and limit errors before oversized allocations or work.
pub fn decode(data: &[u8], globals: Option<&[u8]>, limits: &Limits) -> Result<Bitmap> {
    decode_with_diagnostics(
        data,
        globals,
        limits,
        &mut Diagnostics::new(Recovery::Strict),
    )
}

/// Decodes embedded data or explicitly reported single-page file packaging.
///
/// Only sequential files declaring exactly one page are eligible. Page
/// information, all coded segments/referrals, EOP and terminal empty EOF are
/// validated. The separate EOF association repair accepts only page 1 after
/// that sole page is closed. Random-access, unknown-count, multipage and
/// incomplete files remain errors. Internal page-0 resources must precede
/// the page information; late global segments remain outside this policy.
/// External globals cannot carry a file wrapper.
/// Source bytes, arithmetic termination, sample decisions and budgets are
/// unchanged; positions remain relative to the original stream.
///
/// # Errors
/// As [`decode`]. Strict diagnostics refuse reported departures. No partial
/// bitmap is returned if later framing, coding or resource validation fails.
pub fn decode_with_diagnostics(
    data: &[u8],
    globals: Option<&[u8]>,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Bitmap> {
    let total = data
        .len()
        .checked_add(globals.map_or(0, <[u8]>::len))
        .ok_or_else(|| Error::limit("JBIG2 input size overflow"))?;
    if total > limits.stream_bytes || total > limits.file_bytes {
        return Err(Error::limit("JBIG2 input exceeds byte budget").at(Position::Buffer(0)));
    }
    let mut state = State {
        limits,
        diagnostics,
        page: None,
        segments: BTreeMap::new(),
        bytes: 0,
        work: budget::Work::default(),
        referrals: 0,
        unavailable: BTreeSet::new(),
        used_regions: BTreeSet::new(),
        profiles: None,
        page_profile: false,
        deferred: BTreeSet::new(),
        attached: None,
    };
    if let Some(globals) = globals {
        state.stream(globals, Stream::Globals)?;
        // PDF 7.4.7/Table 12 supplies globals independently of each page stream.
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

struct Page {
    bitmap: Bitmap,
    default_black: bool,
    operator: u8,
    override_operator: bool,
    unknown_height: bool,
    stripe_size: Option<u32>,
    last_stripe: Option<u32>,
    stripe_bottom: u32,
    capabilities: u8,
}

enum Segment {
    Dictionary(symbol::Dictionary),
    Region(StoredRegion),
    Table(Rc<huffman::Table>),
    Patterns(halftone::Dictionary),
    Metadata,
}

#[derive(Clone, Copy)]
struct RegionInfo {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    flags: u8,
}

struct StoredRegion {
    bitmap: Rc<Bitmap>,
    x: u32,
    y: u32,
    operator: u8,
}

struct State<'a> {
    limits: &'a Limits,
    diagnostics: &'a mut Diagnostics,
    page: Option<Page>,
    // Storage remains charged until completion; availability still follows 7.2.4.
    segments: BTreeMap<u32, Segment>,
    bytes: usize,
    work: budget::Work,
    referrals: usize,
    unavailable: BTreeSet<u32>,
    used_regions: BTreeSet<u32>,
    profiles: Option<BTreeSet<u32>>,
    page_profile: bool,
    deferred: BTreeSet<u32>,
    attached: Option<u32>,
}

impl State<'_> {
    fn stream(&mut self, data: &[u8], stream: Stream) -> Result<()> {
        let mut framing = Framing::new(data, stream, self.diagnostics)?;
        let mut reader = Reader::new(data, 0);
        reader.offset = framing.offset();
        while reader.offset < data.len() {
            let position = reader.position();
            self.segment(&mut reader, &mut framing)
                .map_err(|error| error.at(position))?;
        }
        framing.finish(reader.position())
    }

    fn segment(&mut self, reader: &mut Reader<'_>, framing: &mut Framing) -> Result<()> {
        let segment_position = reader.position();
        if self.segments.len() >= self.limits.objects {
            return Err(Error::limit("JBIG2 segment count exceeds budget"));
        }
        self.work.operations(1, self.limits)?;
        let number = reader.u32()?;
        if self.segments.contains_key(&number) {
            return Err(Error::syntax("invalid or duplicate JBIG2 segment number"));
        }
        let flags = reader.byte()?;
        let (count, retention) = self.retention(reader)?;
        self.check_referrals(count)?;
        self.referrals += count;
        if count > self.limits.decoded_bytes.saturating_sub(self.bytes) / size_of::<u32>() {
            return Err(Error::limit("JBIG2 referral storage exceeds byte budget"));
        }
        self.work.operations(count, self.limits)?;
        if matches!(flags & 63, 40 | 42 | 43) && count > 1 {
            return Err(Error::syntax(
                "JBIG2 refinement requires at most one reference",
            ));
        }
        let mut references = Vec::with_capacity(count);
        for _ in 0..count {
            let reference = if number <= 256 {
                u32::from(reader.byte()?)
            } else if number <= 65536 {
                u32::from(reader.u16()?)
            } else {
                reader.u32()?
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
                && (flags & 63 != 62 || count != 1 || self.attached != Some(reference))
            {
                return Err(Error::syntax("invalid JBIG2 deferred extension referral"));
            }
            if flags & 63 != 62
                && matches!(self.segments.get(&reference), Some(Segment::Region(_)))
                && !self.used_regions.insert(reference)
            {
                return Err(Error::syntax(
                    "JBIG2 intermediate region referred to more than once",
                ));
            }
            references.push(reference);
        }
        let association = reader.position();
        let page = if flags & 64 != 0 {
            reader.u32()?
        } else {
            u32::from(reader.byte()?)
        };
        framing.validate_page(page)?;
        let length_position = reader.position();
        let length = reader.u32()?;
        let kind = flags & 63;
        let (mut body, actual_rows) = reader.segment(kind, length)?;
        let position = body.position();
        let reference_bytes = references.len() * size_of::<u32>();
        self.bytes += reference_bytes;
        let scope = framing.scope(
            &Boundary {
                kind,
                page,
                referrals: count,
                length,
                association,
                length_position,
                position: segment_position,
                last: reader.offset == reader.data.len(),
            },
            self.page.as_ref(),
            self.diagnostics,
        )?;
        let segment = if let Some(stream) = scope {
            self.body(kind, stream, &references, &mut body, actual_rows)
                .map_err(|error| error.at(position))?
        } else {
            Segment::Metadata
        };
        self.bytes -= reference_bytes;
        body.finish()?;
        self.retain_segment(number, flags, &references, retention, segment);
        Ok(())
    }

    /// Applies the original segment/referral retention and deferred attachment.
    fn retain_segment(
        &mut self,
        number: u32,
        flags: u8,
        references: &[u32],
        retention: &[u8],
        segment: Segment,
    ) {
        for (index, n) in std::iter::once(&number)
            .chain(references.iter())
            .enumerate()
        {
            if retention[index / 8] & (1 << (index % 8)) == 0 {
                self.unavailable.insert(*n);
            }
        }
        if flags & 128 != 0 {
            self.deferred.insert(number);
            self.attached = Some(number);
        } else if flags & 63 != 62
            || references.len() != 1
            || self.attached != references.first().copied()
        {
            self.attached = None;
        }
        self.segments.insert(number, segment);
    }

    fn retention<'a>(&self, reader: &mut Reader<'a>) -> Result<(usize, &'a [u8])> {
        let short = reader.take(1)?;
        let flags = short[0];
        let (count, retention) = match flags >> 5 {
            0..=4 => {
                let count = usize::from(flags >> 5);
                if flags & 31 >= 1 << (count + 1) {
                    return Err(Error::syntax("nonzero unused JBIG2 retention flags"));
                }
                (count, short)
            }
            7 => {
                let tail = reader.take(3)?;
                let count =
                    usize::try_from(u32::from_be_bytes([flags & 31, tail[0], tail[1], tail[2]]))
                        .map_err(|_| Error::limit("JBIG2 referral count overflow"))?;
                if count < 5 {
                    return Err(Error::syntax("noncanonical JBIG2 referral count"));
                }
                self.check_referrals(count)?;
                let retention = reader.take((count + 8) / 8)?;
                if (count + 1) % 8 != 0 && retention[retention.len() - 1] >> ((count + 1) % 8) != 0
                {
                    return Err(Error::syntax("nonzero unused JBIG2 retention flags"));
                }
                (count, retention)
            }
            _ => return Err(Error::syntax("reserved JBIG2 referral count")),
        };
        Ok((count, retention))
    }

    fn check_referrals(&self, count: usize) -> Result<()> {
        if count > self.limits.value_nodes.saturating_sub(self.referrals) {
            return Err(Error::limit("JBIG2 referrals exceed budget"));
        }
        Ok(())
    }

    fn body(
        &mut self,
        kind: u8,
        stream: Stream,
        references: &[u32],
        reader: &mut Reader<'_>,
        actual_rows: Option<u32>,
    ) -> Result<Segment> {
        match kind {
            0 | 4 | 6 | 7 | 16 | 20 | 22 | 23 | 36 | 38 | 39 | 40 | 42 | 43 | 48..=53 | 62 => {}
            54 => return Err(Error::syntax("JBIG2 colour palettes are forbidden in PDF")),
            _ => return Err(Error::syntax("reserved JBIG2 segment type")),
        }
        if matches!(kind, 49 | 51) {
            return Err(Error::syntax(
                "JBIG2 end-of-page and end-of-file segments are forbidden in PDF",
            ));
        }
        if stream == Stream::Globals && !matches!(kind, 0 | 16 | 53 | 52 | 62) {
            return Err(Error::syntax("page segment in JBIG2 globals"));
        }
        if stream == Stream::Page && self.page.is_none() && kind != 48 {
            return Err(Error::syntax(
                "JBIG2 page information must precede associated segments",
            ));
        }
        if matches!(kind, 16 | 53) {
            if !references.is_empty() {
                return Err(Error::syntax("unexpected JBIG2 dictionary referrals"));
            }
            let mut limits = *self.limits;
            limits.decoded_bytes = limits.decoded_bytes.saturating_sub(self.bytes);
            return if kind == 16 {
                let dictionary = halftone::dictionary(reader, &limits, &mut self.work)?;
                self.bytes += dictionary.bytes;
                Ok(Segment::Patterns(dictionary))
            } else {
                let table = huffman::custom(reader, &limits, &mut self.work)?;
                self.bytes += table.bytes();
                Ok(Segment::Table(Rc::new(table)))
            };
        }
        if kind == 0 {
            return self.dictionary(reader, references);
        }
        if !matches!(kind, 4 | 6 | 7 | 20 | 22 | 23 | 40 | 42 | 43 | 62) && !references.is_empty() {
            return Err(Error::syntax("unexpected JBIG2 segment referrals"));
        }
        match kind {
            48 => self.page_info(reader)?,
            50 => self.end_stripe(reader)?,
            52 => self.profiles(reader, stream)?,
            62 => extension(reader)?,
            _ => {
                return self
                    .region(reader, kind, references, actual_rows)
                    .map(|region| region.map_or(Segment::Metadata, Segment::Region));
            }
        }
        Ok(Segment::Metadata)
    }

    fn dictionary(&mut self, reader: &mut Reader<'_>, references: &[u32]) -> Result<Segment> {
        let imported = self.imported(references)?;
        let mut limits = *self.limits;
        limits.decoded_bytes = limits
            .decoded_bytes
            .saturating_sub(self.bytes + imported.len() * size_of::<Rc<Bitmap>>());
        let retained = references
            .iter()
            .rev()
            .filter_map(|n| self.segments.get(n))
            .find_map(|s| match s {
                Segment::Dictionary(dictionary) => Some(dictionary.contexts.as_ref()),
                _ => None,
            })
            .flatten();
        let dictionary = if reader
            .data
            .get(reader.offset + 1)
            .is_some_and(|b| b & 1 != 0)
        {
            huffman_symbol::decode(
                reader,
                &imported,
                &self.tables(references, 4)?,
                retained,
                &limits,
                &mut self.work,
            )?
        } else {
            if references
                .iter()
                .any(|n| matches!(self.segments.get(n), Some(Segment::Table(_))))
            {
                return Err(Error::syntax(
                    "Huffman table referred to by arithmetic dictionary",
                ));
            }
            symbol::decode(reader, &imported, retained, &limits, &mut self.work)?
        };
        self.bytes += dictionary.bytes;
        Ok(Segment::Dictionary(dictionary))
    }

    fn profiles(&mut self, reader: &mut Reader<'_>, stream: Stream) -> Result<()> {
        if stream == Stream::Globals && !self.segments.is_empty()
            || stream == Stream::Page && (self.profiles.is_none() || self.page_profile)
        {
            return Err(Error::syntax("invalid JBIG2 profiles segment order"));
        }
        let count = reader.u32()? as usize;
        if count > self.limits.value_nodes
            || count > self.limits.decoded_bytes.saturating_sub(self.bytes) / 32
        {
            return Err(Error::limit("JBIG2 profile list exceeds budget"));
        }
        self.work.operations(count, self.limits)?;
        let mut profiles = BTreeSet::new();
        for _ in 0..count {
            profiles.insert(reader.u32()?);
        }
        if stream == Stream::Globals {
            self.profiles = Some(profiles);
            self.bytes += count * 32;
        } else {
            if self
                .profiles
                .as_ref()
                .is_some_and(|first| !first.is_subset(&profiles))
            {
                return Err(Error::syntax("JBIG2 page profile omits global profile"));
            }
            self.page_profile = true;
        }
        Ok(())
    }

    fn tables(&self, references: &[u32], maximum: usize) -> Result<Vec<Rc<huffman::Table>>> {
        let tables: Vec<_> = references
            .iter()
            .filter_map(|n| match self.segments.get(n) {
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

    fn imported(&mut self, references: &[u32]) -> Result<Vec<Rc<Bitmap>>> {
        let mut count = 0usize;
        for reference in references {
            let segment = self
                .segments
                .get(reference)
                .ok_or_else(|| Error::syntax("missing JBIG2 symbol dictionary"))?;
            if matches!(segment, Segment::Table(_)) {
                continue;
            }
            let Segment::Dictionary(dictionary) = segment else {
                return Err(Error::syntax(
                    "JBIG2 symbol referral has wrong segment type",
                ));
            };
            count = count
                .checked_add(dictionary.symbols.len())
                .filter(|&n| {
                    n <= self.limits.value_nodes
                        && n <= self.limits.decoded_bytes.saturating_sub(self.bytes)
                            / size_of::<Rc<Bitmap>>()
                })
                .ok_or_else(|| Error::limit("JBIG2 imported symbol table exceeds budget"))?;
        }
        self.work.operations(count, self.limits)?;
        let mut imported = Vec::with_capacity(count);
        for reference in references {
            if let Some(Segment::Dictionary(dictionary)) = self.segments.get(reference) {
                imported.extend(dictionary.symbols.iter().cloned());
            }
        }
        Ok(imported)
    }

    fn page_info(&mut self, reader: &mut Reader<'_>) -> Result<()> {
        if self.page.is_some() {
            return Err(Error::syntax("duplicate JBIG2 page information"));
        }
        let width = reader.u32()?;
        let height = reader.u32()?;
        reader.take(8)?; // Resolution fields do not change decoded samples.
        let flags = reader.byte()?;
        let stripe = reader.u16()?;
        if width == 0 || height == 0 || flags & 128 != 0 {
            return Err(Error::syntax("invalid JBIG2 page information"));
        }
        let unknown_height = height == u32::MAX;
        if unknown_height && stripe & 0x8000 == 0 {
            return Err(Error::syntax("unknown JBIG2 page height requires stripes"));
        }
        let stripe_size = (stripe & 0x8000 != 0).then_some(u32::from(stripe & 0x7fff));
        let mut limits = *self.limits;
        limits.decoded_bytes = limits.decoded_bytes.saturating_sub(self.bytes);
        bitmap::layout(width, if unknown_height { 0 } else { height }, &limits)?;
        self.work.pixels(
            u64::from(width) * u64::from(if unknown_height { 0 } else { height }),
            self.limits,
        )?;
        let bitmap = Bitmap::new(
            width,
            if unknown_height { 0 } else { height },
            flags & 4 != 0,
            &limits,
        )?;
        // Width must itself fit the pixel budget even for an empty growing page.
        bitmap::layout(width, 1, self.limits)?;
        if bitmap.data.len() > self.limits.decoded_bytes.saturating_sub(self.bytes) {
            return Err(Error::limit("JBIG2 live bitmaps exceed byte budget"));
        }
        self.bytes += bitmap.data.len();
        self.page = Some(Page {
            bitmap,
            default_black: flags & 4 != 0,
            operator: (flags >> 3) & 3,
            override_operator: flags & 64 != 0,
            unknown_height,
            stripe_size,
            last_stripe: None,
            stripe_bottom: 0,
            capabilities: flags & 0x22,
        });
        Ok(())
    }

    fn end_stripe(&mut self, reader: &mut Reader<'_>) -> Result<()> {
        let line = reader.u32()?;
        let page = self
            .page
            .as_ref()
            .ok_or_else(|| Error::syntax("stripe before JBIG2 page information"))?;
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
        self.grow(height)?;
        if let Some(page) = &mut self.page {
            page.last_stripe = Some(line);
        }
        Ok(())
    }

    fn grow(&mut self, height: u32) -> Result<()> {
        let page = self
            .page
            .as_mut()
            .ok_or_else(|| Error::syntax("region before JBIG2 page information"))?;
        if !page.unknown_height || height <= page.bitmap.height {
            return Ok(());
        }
        let (_, bytes) = bitmap::layout(page.bitmap.width, height, self.limits)?;
        let extra = bytes - page.bitmap.data.len();
        self.work.pixels(
            u64::from(page.bitmap.width) * u64::from(height - page.bitmap.height),
            self.limits,
        )?;
        if extra > self.limits.decoded_bytes.saturating_sub(self.bytes) {
            return Err(Error::limit("JBIG2 live bitmaps exceed byte budget"));
        }
        page.bitmap
            .data
            .resize(bytes, if page.default_black { 255 } else { 0 });
        page.bitmap.height = height;
        page.bitmap.clear_padding();
        self.bytes += extra;
        Ok(())
    }

    fn validate_region(&self, info: &RegionInfo, kind: u8, references: &[u32]) -> Result<bool> {
        let RegionInfo {
            width,
            height,
            x,
            y,
            flags,
        } = *info;
        let page = self
            .page
            .as_ref()
            .ok_or_else(|| Error::syntax("region before JBIG2 page information"))?;
        let immediate = !matches!(kind, 4 | 20 | 36 | 40);
        if !page.override_operator
            && flags != page.operator
            && (!matches!(kind, 42 | 43) || !references.is_empty())
        {
            return Err(Error::syntax(
                "JBIG2 direct region operator differs from fixed page default",
            ));
        }
        if !immediate && page.capabilities & 32 == 0 {
            return Err(Error::syntax("JBIG2 page forbids auxiliary regions"));
        }
        if matches!(kind, 40 | 42 | 43) && page.capabilities & 2 == 0 {
            return Err(Error::syntax("JBIG2 page forbids refinement regions"));
        }
        if matches!(kind, 40 | 42 | 43) {
            match references {
                [] if kind != 40 && flags == 4 => {}
                [number] => {
                    let Some(Segment::Region(reference)) = self.segments.get(number) else {
                        return Err(Error::syntax("invalid JBIG2 refinement reference"));
                    };
                    if (width, height, x, y, flags)
                        != (
                            reference.bitmap.width,
                            reference.bitmap.height,
                            reference.x,
                            reference.y,
                            reference.operator,
                        )
                    {
                        return Err(Error::syntax(
                            "JBIG2 refinement region information differs from its reference",
                        ));
                    }
                }
                _ => {
                    return Err(Error::syntax(
                        "invalid JBIG2 refinement reference or operator",
                    ));
                }
            }
        }
        Ok(immediate)
    }

    fn region(
        &mut self,
        reader: &mut Reader<'_>,
        kind: u8,
        references: &[u32],
        actual_rows: Option<u32>,
    ) -> Result<Option<StoredRegion>> {
        let width = reader.u32()?;
        let declared_height = reader.u32()?;
        let height = actual_rows.unwrap_or(declared_height);
        if height > declared_height {
            return Err(Error::syntax(
                "JBIG2 actual row count exceeds declared height",
            ));
        }
        let x = reader.u32()?;
        let y = reader.u32()?;
        let flags = reader.byte()?;
        if width == 0 || height == 0 || flags > 4 {
            return Err(Error::syntax("invalid JBIG2 region information"));
        }
        let info = RegionInfo {
            width,
            height,
            x,
            y,
            flags,
        };
        let immediate = self.validate_region(&info, kind, references)?;
        let page = self
            .page
            .as_ref()
            .ok_or_else(|| Error::syntax("missing JBIG2 page"))?;
        if immediate && page.unknown_height {
            let bottom = y
                .checked_add(height)
                .ok_or_else(|| Error::limit("JBIG2 region coordinate overflow"))?;
            self.grow(bottom)?;
        }
        bitmap::layout(width, height, self.limits)?;
        self.work
            .pixels(u64::from(width) * u64::from(height), self.limits)?;
        let mut limits = *self.limits;
        limits.decoded_bytes = limits.decoded_bytes.saturating_sub(self.bytes);
        let region = if matches!(kind, 4 | 6 | 7) {
            self.text_region(reader, (width, height), references, &mut limits)?
        } else if matches!(kind, 40 | 42 | 43) {
            self.refinement_region(reader, (width, height), (x, y), references, &limits)?
        } else if matches!(kind, 20 | 22 | 23) {
            let [number] = references else {
                return Err(Error::syntax(
                    "JBIG2 halftone requires one pattern dictionary",
                ));
            };
            let Some(Segment::Patterns(dictionary)) = self.segments.get(number) else {
                return Err(Error::syntax("wrong JBIG2 halftone reference type"));
            };
            halftone::region(reader, (width, height), dictionary, &limits, &mut self.work)?
        } else {
            generic::decode(reader, width, height, &limits, actual_rows.is_some())?
        };
        if immediate && let Some(page) = &mut self.page {
            let operator =
                if page.override_operator || (matches!(kind, 42 | 43) && references.is_empty()) {
                    flags
                } else {
                    page.operator
                };
            if let Some(bottom) =
                page.bitmap
                    .combine_page(&region, x, y, operator, page.last_stripe)?
            {
                page.stripe_bottom = page.stripe_bottom.max(bottom);
            }
        }
        if immediate {
            Ok(None)
        } else {
            self.bytes += region.data.len();
            Ok(Some(StoredRegion {
                bitmap: Rc::new(region),
                x,
                y,
                operator: flags,
            }))
        }
    }

    fn text_region(
        &mut self,
        reader: &mut Reader<'_>,
        size: (u32, u32),
        references: &[u32],
        limits: &mut Limits,
    ) -> Result<Bitmap> {
        let imported = self.imported(references)?;
        limits.decoded_bytes = limits
            .decoded_bytes
            .saturating_sub(imported.len() * size_of::<Rc<Bitmap>>());
        if reader
            .data
            .get(reader.offset + 1)
            .is_some_and(|b| b & 1 != 0)
        {
            huffman_text::decode(
                reader,
                size,
                &imported,
                &self.tables(references, 8)?,
                limits,
                &mut self.work,
            )
        } else {
            if references
                .iter()
                .any(|n| matches!(self.segments.get(n), Some(Segment::Table(_))))
            {
                return Err(Error::syntax(
                    "Huffman table referred to by arithmetic text",
                ));
            }
            text::decode(reader, size, &imported, limits, &mut self.work)
        }
    }

    fn refinement_region(
        &mut self,
        reader: &mut Reader<'_>,
        size: (u32, u32),
        origin: (u32, u32),
        references: &[u32],
        limits: &Limits,
    ) -> Result<Bitmap> {
        let (reference, dx, dy) = match references {
            [] => {
                let page = self
                    .page
                    .as_ref()
                    .ok_or_else(|| Error::syntax("JBIG2 refinement has no reference page"))?;
                (&page.bitmap, 0, 0)
            }
            [number] => {
                let Some(Segment::Region(bitmap)) = self.segments.get(number) else {
                    return Err(Error::syntax(
                        "JBIG2 refinement referral has wrong segment type",
                    ));
                };
                (bitmap.bitmap.as_ref(), 0, 0)
            }
            _ => {
                return Err(Error::syntax(
                    "JBIG2 refinement requires at most one reference",
                ));
            }
        };
        let flags = reader.byte()?;
        if flags & !3 != 0 {
            return Err(Error::syntax("reserved JBIG2 refinement flags"));
        }
        let parameters = refinement::Parameters::read(reader, flags & 1)?;
        let mut model = refinement::Model::new(parameters, None, limits)?;
        let base = match reader.position() {
            Position::Buffer(p) => p,
            _ => 0,
        };
        let mut decoder = arithmetic::Decoder::new(reader.rest(), base)?;
        let request = refinement::Request {
            width: size.0,
            height: size.1,
            dx,
            dy,
            prediction: flags & 2 != 0,
        };
        if references.is_empty() {
            model.decode_page(
                &mut decoder,
                reference,
                origin,
                request,
                limits,
                &mut self.work,
            )
        } else {
            model.decode(&mut decoder, reference, request, limits, &mut self.work)
        }
    }
}

/// Unrecognised non-necessary extensions do not alter the bitmap (7.4.14).
fn extension(reader: &mut Reader<'_>) -> Result<()> {
    let kind = reader.u32()?;
    if kind & 0x8000_0000 != 0 {
        if kind & 0x2000_0000 == 0 {
            return Err(Error::syntax("invalid necessary JBIG2 extension flags"));
        }
        return Err(Error::unsupported(
            tags::EXTENSION,
            "unknown necessary JBIG2 extension",
        ));
    }
    reader.rest();
    Ok(())
}
