//! Page text and search responses (9.10, ADR 0014).

use super::{Response, Search, Writer};
use pagura::{Error, Position, Result, SearchOptions, TextPage};
use std::sync::Arc;

/// `text copy_text | u32 quads | Quad[quads] | u32 characters | Character[characters]`.
pub(crate) fn page_text(page: &TextPage, cap: usize) -> Result<Response> {
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

/// `u32 next_page, next_offset | u8 done | u32 count | Match[count]`, where
/// a Match is `u32 page, start_byte, end_byte, quad_count | Quad[quad_count]`.
/// `text` extracts (or recalls) one page of the searched document.
pub(crate) fn search(
    search: Search<'_>,
    options: SearchOptions,
    cap: usize,
    mut text: impl FnMut(u32) -> Result<Arc<TextPage>>,
) -> Result<Response> {
    let mut out = Writer::new(cap);
    out.push(&[0; 13])?;
    let mut count = 0;
    for page in search.start..search.end {
        let extracted = text(page)?;
        let matches = extracted.find(search.query, options)?;
        let offset = if page == search.start {
            usize::try_from(search.offset)
                .map_err(|_| Error::invalid_argument("page index exceeds address space"))?
        } else {
            0
        };
        if offset > matches.len() {
            return Err(
                Error::invalid_argument("search match cursor outside page results")
                    .at(Position::Buffer(17)),
            );
        }
        for (ordinal, found) in matches.iter().enumerate().skip(offset) {
            out.u32(page)?;
            out.count(found.range.start)?;
            out.count(found.range.end)?;
            out.count(found.quads.len())?;
            for quad in &found.quads {
                for point in quad.0 {
                    out.f64(point.x)?;
                    out.f64(point.y)?;
                }
            }
            count += 1;
            if count == search.limit {
                let (next, offset) = if ordinal + 1 == matches.len() {
                    (page + 1, 0)
                } else {
                    (page, ordinal + 1)
                };
                out.cursor(
                    next,
                    u32::try_from(offset)
                        .map_err(|_| Error::limit("search match cursor exceeds u32"))?,
                    next == search.end,
                    count,
                )?;
                return Ok(Response::new(139, out));
            }
        }
    }
    out.cursor(search.end, 0, true, count)?;
    Ok(Response::new(139, out))
}
