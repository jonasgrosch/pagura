//! Bounded reading adapter (ISO 32000-2 9.10, 12.3–12.6; ADR 0014).
//!
//! Every reading response is encoded here into one capped payload.
mod navigation;
mod text;

use crate::protocol::{Reader, SessionStats};
pub(crate) use crate::wire::Writer;
use pagura::{DisplayList, Error, Limits, Position, Result, SearchOptions};

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
/// `u8 present | [f64 x0, y0, x1, y1]`.
pub(crate) fn bounds(list: &DisplayList, cap: usize) -> Result<Response> {
    let bounds = list.content_bounds()?;
    let mut out = Writer::new(cap);
    out.u8(u8::from(bounds.is_some()))?;
    if let Some(bounds) = bounds {
        for value in [bounds.x0(), bounds.y0(), bounds.x1(), bounds.y1()] {
            out.f64(value)?;
        }
    }
    Ok(Response::new(144, out))
}
/// The kind-137 fields, then `u64 text_bytes | u32 text_pages`.
pub(crate) fn stats(stats: SessionStats, text: (usize, usize), cap: usize) -> Result<Response> {
    let (text_bytes, text_pages) = text;
    let mut out = Writer::new(cap);
    out.u64(stats.source_bytes)?;
    out.u64(stats.cache_bytes)?;
    out.u32(stats.documents)?;
    out.u32(stats.lists)?;
    out.u64(u64::try_from(text_bytes).unwrap_or(u64::MAX))?;
    out.count(text_pages)?;
    Ok(Response::new(143, out))
}
pub(crate) use navigation::{labels, links, outlines};
pub(crate) use text::{page_text, search};
