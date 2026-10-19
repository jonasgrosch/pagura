//! Revision terminators: the `startxref` and `%%EOF` that end each revision
//! of the Prev chain (7.5.5, 7.5.6). Opening already parsed every
//! cross-reference section and recorded where it ended.

use crate::Revision;
use crate::view::{Context, Source};
use pagura_core::syntax::{Origin, Parser, is_whitespace};
use pagura_core::{Error, Result};

const EOF_MARKER: &[u8] = b"%%EOF";

/// Where a revision ends: just past `%%EOF`, and past its end-of-line marker.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Boundary {
    pub(crate) eof: usize,
    pub(crate) eol: usize,
}

/// Reads `startxref`, the revision's own offset and `%%EOF` after its
/// cross-reference data, before `end`.
pub(crate) fn boundary(
    ctx: &Context<'_>,
    revision: &Revision,
    end: usize,
    source: Source,
) -> Result<Boundary> {
    let bytes =
        ctx.document.original_bytes().get(..end).ok_or_else(|| {
            source.error(Error::syntax("signature revision lies outside the file"))
        })?;
    let mut parser = Parser::with_offset(bytes, revision.end, Origin::File, ctx.document.limits())?;
    if !parser.keyword(b"startxref")
        || usize::try_from(parser.unsigned()?).ok() != Some(revision.offset())
    {
        return Err(source.error(Error::syntax(
            "signature revision startxref does not match Prev chain",
        )));
    }
    // %%EOF is a comment to the ordinary tokenizer. Only PDF whitespace may
    // intervene here; skip_whitespace would swallow the boundary itself.
    let marker = parser.offset()
        + bytes
            .get(parser.offset()..)
            .unwrap_or_default()
            .iter()
            .take_while(|&&byte| is_whitespace(byte))
            .count();
    if !bytes
        .get(marker..)
        .is_some_and(|tail| tail.starts_with(EOF_MARKER))
    {
        return Err(source.error(Error::syntax("signature revision lacks EOF marker")));
    }
    let eof = marker + EOF_MARKER.len();
    let eol = match bytes.get(eof..) {
        Some([b'\r', b'\n', ..]) => eof + 2,
        Some([b'\r' | b'\n', ..]) => eof + 1,
        _ => eof,
    };
    // A marker followed by ordinary text is not an EOF comment boundary.
    if eol == eof && eof < end && !bytes.get(eof).is_some_and(|&byte| is_whitespace(byte)) {
        return Err(source.error(Error::syntax("signature EOF marker has trailing text")));
    }
    Ok(Boundary { eof, eol })
}
