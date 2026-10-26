//! Page text and search responses (9.10, ADR 0014).

use super::{MAX_PAGE_CHARACTERS, Response, Search};
use crate::protocol::{Field, PageRef, ResponseKind};
use crate::wire::Writer;
use pagura::ReadResult;
use pagura::{Error, Result, SearchOptions, TextPage};
use std::sync::Arc;

/// `text copy_text | u32 quads | Quad[quads] | u32 characters | Character[characters]`.
pub(crate) fn page_text(page: &TextPage, cap: usize) -> Result<Response> {
    if page.reading_characters().len() > MAX_PAGE_CHARACTERS {
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
        out.f64s(glyph.quad.0.map(|point| [point.x, point.y]).as_flattened())?;
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
    Ok(Response::new(ResponseKind::PageText, out))
}

/// Where the next search request continues, written before the matches.
#[derive(Debug, Clone, Copy)]
struct Cursor {
    page: u32,
    offset: u32,
    count: u32,
}

impl Cursor {
    const BYTES: usize = 13;

    /// `u32 next_page | u32 next_offset | u8 done | u32 count`; the window
    /// is done when it continues at its end.
    fn to_bytes(self, end: u32) -> [u8; Self::BYTES] {
        let mut bytes = [0; Self::BYTES];
        bytes[..4].copy_from_slice(&self.page.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.offset.to_le_bytes());
        bytes[8] = u8::from(self.page == end);
        bytes[9..].copy_from_slice(&self.count.to_le_bytes());
        bytes
    }
}

/// `Cursor | Match[count]`, where a Match is `u32 page, start_byte,
/// end_byte, quad_count | Quad[quad_count]`. `text` extracts (or recalls)
/// one page of the searched document.
pub(crate) fn search(
    search: &Search<'_>,
    options: SearchOptions,
    cap: usize,
    mut text: impl FnMut(PageRef) -> ReadResult<Arc<TextPage>>,
) -> ReadResult<Response> {
    let (start, end, limit) = (search.start.get(), search.end.get(), search.limit.get());
    let mut out = Writer::new(cap);
    let header = out.slot::<{ Cursor::BYTES }>()?;
    let mut count = 0;
    for page in start..end {
        // Page errors point at the window's start field.
        let extracted = text(PageRef::new(
            search.document,
            Field::new(page, search.start.offset()),
        ))?;
        let matches = extracted.find(search.query.get(), options)?;
        let skip = if page == start {
            search.offset.index()?
        } else {
            0
        };
        if skip > matches.len() {
            return Err(
                Error::invalid_argument("search match cursor outside page results")
                    .at(search.offset.position())
                    .into(),
            );
        }
        for (ordinal, found) in matches.iter().enumerate().skip(skip) {
            out.u32(page)?;
            out.count(found.range.start)?;
            out.count(found.range.end)?;
            out.count(found.quads.len())?;
            for quad in &found.quads {
                out.f64s(quad.0.map(|point| [point.x, point.y]).as_flattened())?;
            }
            count += 1;
            if count == limit {
                let next = if ordinal + 1 == matches.len() {
                    Cursor {
                        page: page + 1,
                        offset: 0,
                        count,
                    }
                } else {
                    let offset = u32::try_from(ordinal + 1)
                        .map_err(|_| Error::limit("search match cursor exceeds u32"))?;
                    Cursor {
                        page,
                        offset,
                        count,
                    }
                };
                out.fill(header, next.to_bytes(end));
                return Ok(Response::new(ResponseKind::Search, out));
            }
        }
    }
    out.fill(
        header,
        Cursor {
            page: end,
            offset: 0,
            count,
        }
        .to_bytes(end),
    );
    Ok(Response::new(ResponseKind::Search, out))
}
