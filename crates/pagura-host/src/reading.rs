//! Bounded reading adapter (9.10, 12.3–12.6; ADR 0014).
//!
//! Every reading response is encoded here into one capped payload, which the
//! frame encoder sends without copying.

mod navigation;
mod text;

pub(crate) use navigation::{labels, links, outlines};
pub(crate) use text::{page_text, search};

use crate::protocol::{Field, PageRef, RequestKind, ResponseKind, SearchFlags, SessionStats};
use crate::wire::Writer;
use pagura::{DisplayList, Error, Result, SearchOptions};

/// Most matches one search request returns.
pub const MAX_SEARCH_MATCHES: u32 = 1024;
/// Most pages one search window spans.
pub const MAX_SEARCH_PAGES: u32 = 16;
/// Most UTF-8 bytes in a search query; `Limits::string_bytes` can lower it.
pub const MAX_QUERY_BYTES: usize = 4096;
/// Most semantic characters in one page text response.
pub const MAX_PAGE_CHARACTERS: usize = 1_000_000;

/// The bounded search window and continuation cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Search<'a> {
    /// Session-local document handle.
    pub document: Field<u32>,
    /// First page, inclusive; also the continuation page.
    pub start: Field<u32>,
    /// Last page, exclusive; at most [`MAX_SEARCH_PAGES`] after `start`.
    pub end: Field<u32>,
    /// Match ordinal on the first page, for continuation within a page.
    pub offset: Field<u32>,
    /// Maximum matches to return, from 1 through [`MAX_SEARCH_MATCHES`].
    pub limit: Field<u32>,
    /// [`SearchFlags`] bits; undefined bits are an operational error.
    pub flags: Field<u8>,
    /// Nonempty UTF-8 query, at most [`MAX_QUERY_BYTES`].
    pub query: Field<&'a str>,
}

impl Search<'_> {
    /// Checks the window against a document of `pages` pages.
    pub(crate) fn validate(&self, pages: usize) -> Result<SearchOptions> {
        let (start, end) = (self.start.get(), self.end.get());
        if start > end {
            return Err(
                Error::invalid_argument("search start exceeds end").at(self.start.position())
            );
        }
        if usize::try_from(end).ok().is_none_or(|end| end > pages) {
            return Err(
                Error::invalid_argument("search end outside document").at(self.end.position())
            );
        }
        if end - start > MAX_SEARCH_PAGES {
            return Err(
                Error::invalid_argument("search window exceeds sixteen pages")
                    .at(self.end.position()),
            );
        }
        if !(1..=MAX_SEARCH_MATCHES).contains(&self.limit.get()) {
            return Err(
                Error::invalid_argument("search limit must be 1 through 1024")
                    .at(self.limit.position()),
            );
        }
        let flags = SearchFlags::from_bits(self.flags.get()).ok_or_else(|| {
            Error::invalid_argument("unknown search option bits").at(self.flags.position())
        })?;
        let query = self.query.get();
        if query.is_empty() || query.len() > MAX_QUERY_BYTES {
            return Err(Error::invalid_argument(
                "search query must contain 1 through 4096 UTF-8 bytes",
            )
            .at(self.query.position()));
        }
        if start == end && self.offset.get() != 0 {
            return Err(
                Error::invalid_argument("empty search window requires zero cursor")
                    .at(self.offset.position()),
            );
        }
        Ok(SearchOptions {
            case_insensitive: !flags.contains(SearchFlags::CASE_SENSITIVE),
            whole_words: flags.contains(SearchFlags::WHOLE_WORDS),
            join_hyphenated: flags.contains(SearchFlags::JOIN_HYPHENATED),
        })
    }
}

/// Requests added by protocol 1.2 and 1.3.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Request<'a> {
    /// Reading-order page text and selection geometry.
    Text(PageRef),
    /// A resumable search window.
    Search(Search<'a>),
    /// Link annotations of a page.
    Links(PageRef),
    /// Preorder outline tree of a document.
    Outlines(Field<u32>),
    /// Labels of a document, in page order.
    Labels(Field<u32>),
    /// Extended session accounting; kind 9 remains unchanged.
    Stats,
    /// Conservative painted bounds in default user space (1.3).
    Bounds(PageRef),
}

impl Request<'_> {
    /// The request's kind byte.
    pub const fn kind(&self) -> RequestKind {
        match self {
            Self::Text(_) => RequestKind::PageText,
            Self::Search(_) => RequestKind::Search,
            Self::Links(_) => RequestKind::Links,
            Self::Outlines(_) => RequestKind::Outlines,
            Self::Labels(_) => RequestKind::Labels,
            Self::Stats => RequestKind::ReadingStats,
            Self::Bounds(_) => RequestKind::Bounds,
        }
    }
}

/// A fully bounded reading payload. Its kind and bytes are constructed only
/// by this adapter.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    kind: ResponseKind,
    payload: Vec<u8>,
}

impl Response {
    pub(crate) fn new(kind: ResponseKind, writer: Writer) -> Self {
        Self {
            kind,
            payload: writer.finish(),
        }
    }

    /// The response's kind byte.
    pub const fn kind(&self) -> ResponseKind {
        self.kind
    }

    /// The encoded payload, after kind and id.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// `u8 present | [f64 x0, y0, x1, y1]`.
pub(crate) fn bounds(list: &DisplayList, cap: usize) -> Result<Response> {
    let bounds = list.content_bounds()?;
    let mut out = Writer::new(cap);
    out.flag(bounds.is_some())?;
    if let Some(bounds) = bounds {
        out.f64s(&[bounds.x0(), bounds.y0(), bounds.x1(), bounds.y1()])?;
    }
    Ok(Response::new(ResponseKind::Bounds, out))
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
    Ok(Response::new(ResponseKind::ReadingStats, out))
}
