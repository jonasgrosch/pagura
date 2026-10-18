//! Bounded reading adapter (ISO 32000-2 9.10, 12.3–12.6; ADR 0014).
mod navigation;

use crate::protocol::Reader;
pub(crate) use crate::wire::Writer;
use pagura::{Error, Limits, Position, Result, SearchOptions};

/// The bounded search window and continuation cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Search<'a> {
    /// Session-local document handle.
    pub document: u32,
    /// First page, inclusive; also the continuation page.
    pub start: u32,
    /// Last page, exclusive; at most sixteen pages per request.
    pub end: u32,
    /// Match ordinal on the first page, for continuation within a page.
    pub offset: u32,
    /// Maximum matches to return, from 1 through 1024.
    pub limit: u32,
    /// Bit 0 case-sensitive, bit 1 whole words, bit 2 join line hyphens.
    pub flags: u8,
    /// Nonempty UTF-8 query, at most 4096 bytes.
    pub query: &'a str,
}
impl Search<'_> {
    pub(crate) fn validate(self, pages: usize) -> Result<SearchOptions> {
        if self.start > self.end {
            return Err(Error::invalid_argument("search start exceeds end").at(Position::Buffer(9)));
        }
        if u64::from(self.end) > u64::try_from(pages).unwrap_or(u64::MAX) {
            return Err(
                Error::invalid_argument("search end outside document").at(Position::Buffer(13))
            );
        }
        if self.end - self.start > 16 {
            return Err(
                Error::invalid_argument("search window exceeds sixteen pages")
                    .at(Position::Buffer(13)),
            );
        }
        if !(1..=1024).contains(&self.limit) {
            return Err(
                Error::invalid_argument("search limit must be 1 through 1024")
                    .at(Position::Buffer(21)),
            );
        }
        if self.flags > 7 {
            return Err(
                Error::invalid_argument("unknown search option bits").at(Position::Buffer(25))
            );
        }
        if self.query.is_empty() || self.query.len() > 4096 {
            return Err(Error::invalid_argument(
                "search query must contain 1 through 4096 UTF-8 bytes",
            )
            .at(Position::Buffer(30)));
        }
        if self.start == self.end && self.offset != 0 {
            return Err(
                Error::invalid_argument("empty search window requires zero cursor")
                    .at(Position::Buffer(17)),
            );
        }
        Ok(SearchOptions {
            case_insensitive: self.flags & 1 == 0,
            whole_words: self.flags & 2 != 0,
            join_hyphenated: self.flags & 4 != 0,
        })
    }
}
/// Requests added by protocol v1.2.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Request<'a> {
    /// Reading-order page text and selection geometry (document, page).
    Text(u32, u32),
    /// A resumable search window.
    Search(Search<'a>),
    /// Link annotations (document, page).
    Links(u32, u32),
    /// Preorder outline tree for a document.
    Outlines(u32),
    /// Labels in page order for a document.
    Labels(u32),
    /// Extended session accounting; kind 9 remains unchanged.
    Stats,
    /// Conservative painted bounds in default user space (document, page; v1.3).
    Bounds(u32, u32),
}
/// A fully bounded reading payload. Its wire kind and data are constructed only
/// by the adapter; v1/v1.1 serialization never changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub(crate) kind: u8,
    pub(crate) payload: Vec<u8>,
}
impl Response {
    pub(crate) fn new(kind: u8, writer: Writer) -> Self {
        Self {
            kind,
            payload: writer.finish(),
        }
    }
}
pub(crate) fn decode<'a>(kind: u8, reader: &mut Reader<'a>, limits: Limits) -> Result<Request<'a>> {
    Ok(match kind {
        10 => Request::Text(reader.u32()?, reader.u32()?),
        11 => {
            let (document, start, end, offset, limit) = (
                reader.u32()?,
                reader.u32()?,
                reader.u32()?,
                reader.u32()?,
                reader.u32()?,
            );
            let flags = reader.byte()?;
            let query = std::str::from_utf8(reader.blob(limits.string_bytes.min(4096))?)
                .map_err(|_| Error::syntax("search query is not UTF-8").at(Position::Buffer(30)))?;
            Request::Search(Search {
                document,
                start,
                end,
                offset,
                limit,
                flags,
                query,
            })
        }
        12 => Request::Links(reader.u32()?, reader.u32()?),
        13 => Request::Outlines(reader.u32()?),
        14 => Request::Labels(reader.u32()?),
        16 => Request::Bounds(reader.u32()?, reader.u32()?),
        _ => Request::Stats,
    })
}
pub(crate) fn page_text(page: &pagura::TextPage, cap: usize) -> Result<Response> {
    if page.reading_characters().len() > 1_000_000 {
        return Err(Error::limit(
            "page text character count exceeds one million",
        ));
    }
    let mut out = Writer::new(cap);
    out.text(page.text())?;
    // A shared table prevents ActualText replacements from repeating all source
    // quads for every semantic character. Construction stays linear in records.
    out.count(page.glyphs().len())?;
    for glyph in page.glyphs() {
        for point in glyph.quad.0 {
            out.f64(point.x)?;
            out.f64(point.y)?;
        }
    }
    out.count(page.reading_characters().len())?;
    for character in page.reading_characters() {
        if page.glyphs().get(character.glyphs.clone()).is_none() {
            return Err(Error::invalid_argument("invalid extracted glyph range"));
        }
        out.count(character.range.end)?;
        out.count(character.glyphs.start)?;
        out.count(character.glyphs.len())?;
    }
    Ok(Response::new(138, out))
}
pub(crate) fn text_charge(page: &pagura::TextPage) -> usize {
    // TextPage excludes its inline header and allocator bookkeeping; retain the
    // fixed cache-entry allowance in addition to the owned-capacity estimate.
    page.retained_bytes().saturating_add(512)
}
pub(crate) use navigation::{labels, links, outlines};
