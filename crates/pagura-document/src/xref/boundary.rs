//! File-header and final-tail grammar shared by complete and ranged opening.

use crate::repairs;
use pagura_core::syntax::{Origin, ParseMode, Parser, is_whitespace};
use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Position, Result};

/// The header must start within the first 1024 bytes (7.5.2).
const HEADER_WINDOW: usize = 1024;
/// `%PDF-1.x` or `%PDF-2.x`.
const HEADER_LENGTH: usize = 8;
/// `startxref` must lie within the last 1024 bytes (7.5.5).
pub(super) const TAIL_WINDOW: usize = 1024;

/// Header grammar over a window starting at absolute file offset zero.
/// A repair is provisional until the whole header line has completed.
pub(super) fn parse_header<M: ParseMode>(
    parser: &mut Parser<'_, M>,
    diagnostics: &mut Diagnostics,
) -> std::result::Result<usize, M::Fault> {
    let bytes = parser.data();
    let offset = bytes
        .get(..bytes.len().min(HEADER_WINDOW + HEADER_LENGTH))
        .and_then(|head| {
            // The header grammar permits any single minor digit for major
            // versions 1 and 2.
            head.windows(HEADER_LENGTH).position(|part| match part {
                [b'%', b'P', b'D', b'F', b'-', b'1' | b'2', b'.', minor] => minor.is_ascii_digit(),
                _ => false,
            })
        })
        .filter(|&at| at < HEADER_WINDOW);
    let Some(offset) = offset else {
        // The final admissible start is HEADER_WINDOW - 1. An unavailable
        // suffix can still contain that header; actual EOF remains invalid.
        parser.has_bytes(HEADER_WINDOW + HEADER_LENGTH - 1)?;
        return Err(Error::syntax("missing or unknown PDF header")
            .at(Position::File(0))
            .into());
    };
    if offset != 0 {
        diagnostics.repair(
            Diagnostic::new(
                repairs::HEADER_OFFSET,
                "PDF header follows leading junk; offsets adjusted when necessary",
            )
            .at(Position::File(offset)),
        )?;
    }
    let line = offset + HEADER_LENGTH;
    if matches!(bytes.get(line), Some(b'\r' | b'\n')) {
        parser.seek(line + 1)?;
    } else {
        let suffix = bytes
            .get(line..)
            .ok_or(Error::syntax("truncated PDF header"))?;
        let end = suffix
            .iter()
            .take(HEADER_WINDOW)
            .position(|&byte| matches!(byte, b'\r' | b'\n'));
        let Some(end) = end else {
            parser.has_bytes(line + HEADER_WINDOW)?;
            return Err(Error::syntax("missing or unknown PDF header")
                .at(Position::File(offset))
                .into());
        };
        diagnostics.repair(
            Diagnostic::new(
                repairs::HEADER_FORMAT,
                "extra bytes in the PDF header line; version retained",
            )
            .at(Position::File(line)),
        )?;
        parser.seek(line + end + 1)?;
    }
    Ok(offset)
}

/// The complete final tail, with absolute positions and declared file length.
/// A sparse caller must obtain this exact extent before deciding the last
/// marker; an available earlier marker is not authoritative.
pub(super) fn startxref(
    bytes: &[u8],
    origin: usize,
    file_length: usize,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Option<usize>> {
    const STARTXREF: &[u8] = b"startxref";
    debug_assert_eq!(origin.checked_add(bytes.len()), Some(file_length));
    let Some(marker) = bytes
        .windows(STARTXREF.len())
        .rposition(|part| part == STARTXREF)
    else {
        return Ok(None);
    };
    let mut tail = Parser::with_offset(bytes, marker + STARTXREF.len(), Origin::File, *limits)?
        .with_origin_offset(origin)?;
    let offset = usize::try_from(tail.unsigned()?)
        .ok()
        .filter(|&offset| offset < file_length)
        .ok_or(
            Error::syntax("startxref offset outside the file").at(Position::File(origin + marker)),
        )?;
    let rest = bytes.get(tail.offset()..).unwrap_or_default();
    let rest = rest
        .get(rest.iter().take_while(|&&byte| is_whitespace(byte)).count()..)
        .unwrap_or_default();
    let Some(after_eof) = rest.strip_prefix(b"%%EOF") else {
        return Err(Error::syntax("missing %%EOF after startxref").at(tail.position()));
    };
    if !after_eof.iter().all(|&b| is_whitespace(b)) {
        diagnostics.repair(
            Diagnostic::new(
                repairs::TRAILING_BYTES,
                "data follows the final %%EOF; ignored",
            )
            .at(Position::File(file_length - after_eof.len())),
        )?;
    }
    Ok(Some(offset))
}

#[cfg(test)]
mod tests;
