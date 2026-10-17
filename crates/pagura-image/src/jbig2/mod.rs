//! JBIG2 embedded sequential streams (ISO 32000-2 clause 7.4.7; T.88
//! Annex D.3). Decoding returns packed black-is-one samples; the PDF image
//! adapter is responsible for PDF sample polarity.
//!
//! Generic regions support arithmetic templates 0–3, adaptive pixels,
//! typical prediction and MMR, generic refinement, direct/refined/aggregate
//! arithmetic symbols, retained bitmap contexts, and top-left arithmetic text
//! regions with refinement. Other
//! coding procedures fail with stable feature tags. File headers, page
//! numbers other than 1, and end-of-page /
//! end-of-file segments are rejected as required by PDF clause 7.4.7.
//! Coding algorithms remain pending primary-text review of T.88; exact
//! external bitmap comparisons are recorded separately in the requirements.

mod arithmetic;
mod bitmap;
mod budget;
mod generic;
mod integer;
mod mmr;
mod reader;
mod refinement;
mod symbol;
#[cfg(test)]
mod tests;
mod text;

use pagura_core::{Error, Limits, Position, Result};
use reader::Reader;
use std::{collections::BTreeMap, rc::Rc};

/// Stable JBIG2 feature tags. Tags for arithmetic capabilities now supported
/// remain available for compatibility.
pub mod tags {
    use pagura_core::Feature;
    /// Huffman symbol dictionary decoding (T.88 clause 6.5).
    pub const SYMBOL_DICTIONARY: Feature = Feature::new("jbig2.symbol-dictionary");
    /// Compatibility tag for refinement and aggregate symbols.
    pub const SYMBOL_REFINEMENT: Feature = Feature::new("jbig2.symbol-refinement");
    /// Compatibility tag for retained arithmetic symbol contexts.
    pub const SYMBOL_CONTEXT: Feature = Feature::new("jbig2.symbol-context");
    /// Compatibility tag for text refinement decoding.
    pub const TEXT_REFINEMENT: Feature = Feature::new("jbig2.text-refinement");
    /// Transposed or non-top-left text reference corners.
    pub const TEXT_LAYOUT: Feature = Feature::new("jbig2.text-layout");
    /// Huffman text region decoding (T.88 clause 6.4).
    pub const TEXT_REGION: Feature = Feature::new("jbig2.text-region");
    /// Pattern dictionary decoding (T.88 clause 6.7).
    pub const PATTERN_DICTIONARY: Feature = Feature::new("jbig2.pattern-dictionary");
    /// Halftone region decoding (T.88 clause 6.6).
    pub const HALFTONE_REGION: Feature = Feature::new("jbig2.halftone-region");
    /// Compatibility tag for generic refinement.
    pub const REFINEMENT_REGION: Feature = Feature::new("jbig2.refinement-region");
    /// User-defined Huffman tables (T.88 clause 7.4.13).
    pub const HUFFMAN_TABLE: Feature = Feature::new("jbig2.huffman-table");
    /// Optional extension segments (T.88 clause 7.4.15).
    pub const EXTENSION: Feature = Feature::new("jbig2.extension");
    /// A region operator differing from a non-overridable page default.
    pub const PAGE_COMBINATION: Feature = Feature::new("jbig2.page-combination");
    /// Profile metadata segments.
    pub const PROFILE: Feature = Feature::new("jbig2.profile");
    /// Reserved segment types.
    pub const SEGMENT_TYPE: Feature = Feature::new("jbig2.segment-type");
    /// The extended adaptive template from the T.88 amendments.
    pub const EXTENDED_TEMPLATE: Feature = Feature::new("jbig2.extended-template");
    /// T.6 uncompressed extension coding.
    pub const MMR_EXTENSION: Feature = Feature::new("jbig2.mmr-extension");
    /// Generic regions with an unspecified segment length (T.88 clause 7.2.7).
    pub const UNKNOWN_LENGTH: Feature = Feature::new("jbig2.unknown-length");
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
/// Every input and bitmap is bounded by `limits`; cumulative pixel work is
/// bounded by `raster_work`, segment count by `objects`, and referrals by
/// `value_nodes`. Segment dispatch, referral expansion, symbol height/export
/// runs and text placements share the cumulative `operations` budget. `decoded_bytes` also bounds simultaneously live bitmaps
/// and arithmetic contexts. No partial page is returned on an error.
///
/// # Errors
///
/// Returns positioned syntax errors for malformed embedded streams, cycle
/// errors for self-referrals, explicit unsupported tags for unimplemented
/// coding modes, and limit errors before oversized allocations or work.
pub fn decode(data: &[u8], globals: Option<&[u8]>, limits: &Limits) -> Result<Bitmap> {
    let total = data
        .len()
        .checked_add(globals.map_or(0, <[u8]>::len))
        .ok_or_else(|| Error::limit("JBIG2 input size overflow"))?;
    if total > limits.stream_bytes || total > limits.file_bytes {
        return Err(Error::limit("JBIG2 input exceeds byte budget").at(Position::Buffer(0)));
    }
    let mut state = State {
        limits,
        page: None,
        segments: BTreeMap::new(),
        bytes: 0,
        work: budget::Work::default(),
        referrals: 0,
    };
    if let Some(globals) = globals {
        state.stream(globals, true)?;
    }
    state.stream(data, false)?;
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
    stripe_size: u32,
    last_stripe: Option<u32>,
}

enum Segment {
    Dictionary(symbol::Dictionary),
    Region(Rc<Bitmap>),
    Metadata,
}

struct State<'a> {
    limits: &'a Limits,
    page: Option<Page>,
    // Retention bits are lifetime hints. Dictionary storage is retained until
    // completion and charged conservatively, including coding-state overhead.
    // Intermediate region bitmaps are retained and charged to the byte budget.
    segments: BTreeMap<u32, Segment>,
    bytes: usize,
    work: budget::Work,
    referrals: usize,
}

impl State<'_> {
    fn stream(&mut self, data: &[u8], global: bool) -> Result<()> {
        if data.starts_with(&[0x97, 0x4a, 0x42, 0x32, 0x0d, 0x0a, 0x1a, 0x0a]) {
            return Err(
                Error::syntax("JBIG2 file headers are forbidden in PDF").at(Position::Buffer(0))
            );
        }
        let mut reader = Reader::new(data, 0);
        while reader.offset < data.len() {
            let position = reader.position();
            self.segment(&mut reader, global)
                .map_err(|error| error.at(position))?;
        }
        Ok(())
    }

    fn segment(&mut self, reader: &mut Reader<'_>, global: bool) -> Result<()> {
        if self.segments.len() >= self.limits.objects {
            return Err(Error::limit("JBIG2 segment count exceeds budget"));
        }
        self.work.operations(1, self.limits)?;
        let number = reader.u32()?;
        if self.segments.contains_key(&number) || number == u32::MAX {
            return Err(Error::syntax("invalid or duplicate JBIG2 segment number"));
        }
        let flags = reader.byte()?;
        let count_flags = reader.byte()?;
        let count = match count_flags >> 5 {
            0..=4 => usize::from(count_flags >> 5),
            7 => {
                let tail = reader.take(3)?;
                let count = u32::from_be_bytes([count_flags & 31, tail[0], tail[1], tail[2]]);
                let count = usize::try_from(count)
                    .map_err(|_| Error::limit("JBIG2 referral count overflow"))?;
                if count < 5 {
                    return Err(Error::syntax("noncanonical JBIG2 referral count"));
                }
                self.check_referrals(count)?;
                reader.take((count + 8) / 8)?;
                count
            }
            _ => return Err(Error::syntax("reserved JBIG2 referral count")),
        };
        self.check_referrals(count)?;
        self.referrals += count;
        if count > self.limits.decoded_bytes.saturating_sub(self.bytes) / size_of::<u32>() {
            return Err(Error::limit("JBIG2 referral storage exceeds byte budget"));
        }
        self.work.operations(count, self.limits)?;
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
            if reference > number || !self.segments.contains_key(&reference) {
                return Err(Error::syntax("JBIG2 referral is not an earlier segment"));
            }
            references.push(reference);
        }
        let page = if flags & 64 != 0 {
            reader.u32()?
        } else {
            u32::from(reader.byte()?)
        };
        if page != u32::from(!global) {
            return Err(Error::syntax("invalid JBIG2 embedded page association"));
        }
        let length = reader.u32()?;
        let kind = flags & 63;
        if length == u32::MAX {
            if matches!(kind, 38 | 39) {
                return Err(Error::unsupported(
                    tags::UNKNOWN_LENGTH,
                    "JBIG2 unspecified generic-region length is not implemented",
                ));
            }
            return Err(Error::syntax(
                "unspecified length of JBIG2 non-generic segment",
            ));
        }
        let length =
            usize::try_from(length).map_err(|_| Error::limit("JBIG2 segment size overflow"))?;
        let base = match reader.position() {
            Position::Buffer(p) => p,
            _ => 0,
        };
        let body = reader.take(length)?;
        let mut body = Reader::new(body, base);
        let reference_bytes = references.len() * size_of::<u32>();
        self.bytes += reference_bytes;
        let segment = self
            .body(kind, global, &references, &mut body)
            .map_err(|error| error.at(Position::Buffer(base)))?;
        self.bytes -= reference_bytes;
        body.finish()?;
        self.segments.insert(number, segment);
        Ok(())
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
        global: bool,
        references: &[u32],
        reader: &mut Reader<'_>,
    ) -> Result<Segment> {
        let unsupported = match kind {
            16 => Some(tags::PATTERN_DICTIONARY),
            20 | 22 | 23 => Some(tags::HALFTONE_REGION),
            53 => Some(tags::HUFFMAN_TABLE),
            62 => Some(tags::EXTENSION),
            0 | 4 | 6 | 7 | 36 | 38 | 39 | 40 | 42 | 43 | 48..=52 => None,
            54 => return Err(Error::syntax("JBIG2 colour palettes are forbidden in PDF")),
            _ => Some(tags::SEGMENT_TYPE),
        };
        if let Some(feature) = unsupported {
            return Err(Error::unsupported(
                feature,
                "JBIG2 segment coding is not implemented",
            ));
        }
        if matches!(kind, 49 | 51) {
            return Err(Error::syntax(
                "JBIG2 end-of-page and end-of-file segments are forbidden in PDF",
            ));
        }
        if global && kind != 0 {
            return Err(Error::syntax("page segment in JBIG2 globals"));
        }
        if kind == 0 {
            let imported = self.imported(references)?;
            let mut limits = *self.limits;
            limits.decoded_bytes = limits
                .decoded_bytes
                .saturating_sub(self.bytes + imported.len() * size_of::<Rc<Bitmap>>());
            let retained = references
                .last()
                .and_then(|n| self.segments.get(n))
                .and_then(|s| match s {
                    Segment::Dictionary(dictionary) => dictionary.contexts.as_ref(),
                    _ => None,
                });
            let dictionary = symbol::decode(reader, &imported, retained, &limits, &mut self.work)?;
            self.bytes += dictionary.bytes;
            return Ok(Segment::Dictionary(dictionary));
        }
        if !matches!(kind, 4 | 6 | 7 | 40 | 42 | 43) && !references.is_empty() {
            return Err(Error::syntax("unexpected JBIG2 segment referrals"));
        }
        match kind {
            48 => self.page_info(reader)?,
            50 => self.end_stripe(reader)?,
            52 => {
                return Err(Error::unsupported(
                    tags::PROFILE,
                    "JBIG2 profile segments are not implemented",
                ));
            }
            _ => {
                return self
                    .region(reader, kind, references)
                    .map(|region| region.map_or(Segment::Metadata, Segment::Region));
            }
        }
        Ok(Segment::Metadata)
    }

    fn imported(&mut self, references: &[u32]) -> Result<Vec<Rc<Bitmap>>> {
        let mut count = 0usize;
        for reference in references {
            let segment = self
                .segments
                .get(reference)
                .ok_or_else(|| Error::syntax("missing JBIG2 symbol dictionary"))?;
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
        let stripe_size = if stripe & 0x8000 != 0 {
            u32::from(stripe & 0x7fff)
        } else {
            0
        };
        if stripe & 0x8000 != 0 && stripe_size == 0 {
            return Err(Error::syntax("zero JBIG2 maximum stripe size"));
        }
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
        });
        Ok(())
    }

    fn end_stripe(&mut self, reader: &mut Reader<'_>) -> Result<()> {
        let line = reader.u32()?;
        let page = self
            .page
            .as_ref()
            .ok_or_else(|| Error::syntax("stripe before JBIG2 page information"))?;
        if page.stripe_size == 0
            || page.last_stripe.is_some_and(|last| line <= last)
            || line.saturating_sub(page.last_stripe.map_or(0, |n| n + 1)) >= page.stripe_size
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

    fn region(
        &mut self,
        reader: &mut Reader<'_>,
        kind: u8,
        references: &[u32],
    ) -> Result<Option<Rc<Bitmap>>> {
        let width = reader.u32()?;
        let height = reader.u32()?;
        let x = reader.u32()?;
        let y = reader.u32()?;
        let flags = reader.byte()?;
        if width == 0 || height == 0 || flags > 4 {
            return Err(Error::syntax("invalid JBIG2 region information"));
        }
        let page = self
            .page
            .as_ref()
            .ok_or_else(|| Error::syntax("region before JBIG2 page information"))?;
        let immediate = !matches!(kind, 4 | 36 | 40);
        if immediate && !page.override_operator && flags != page.operator {
            return Err(Error::unsupported(
                tags::PAGE_COMBINATION,
                "JBIG2 region operator differs from fixed page default",
            ));
        }
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
            let imported = self.imported(references)?;
            limits.decoded_bytes = limits
                .decoded_bytes
                .saturating_sub(imported.len() * size_of::<Rc<Bitmap>>());
            text::decode(reader, (width, height), &imported, &limits, &mut self.work)?
        } else if matches!(kind, 40 | 42 | 43) {
            self.refinement_region(reader, (width, height), (x, y), references, &limits)?
        } else {
            generic::decode(reader, width, height, &limits)?
        };
        if immediate && let Some(page) = &mut self.page {
            let operator = if page.override_operator {
                flags
            } else {
                page.operator
            };
            page.bitmap.combine(&region, x, y, operator);
        }
        if immediate {
            Ok(None)
        } else {
            self.bytes += region.data.len();
            Ok(Some(Rc::new(region)))
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
                (&page.bitmap, -i64::from(origin.0), -i64::from(origin.1))
            }
            [number] => {
                let Some(Segment::Region(bitmap)) = self.segments.get(number) else {
                    return Err(Error::syntax(
                        "JBIG2 refinement referral has wrong segment type",
                    ));
                };
                (bitmap.as_ref(), 0, 0)
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
        model.decode(
            &mut decoder,
            reference,
            refinement::Request {
                width: size.0,
                height: size.1,
                dx,
                dy,
                prediction: flags & 2 != 0,
            },
            limits,
            &mut self.work,
        )
    }
}
