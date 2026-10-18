//! Bounded recovery of malformed security fields (7.3.4.2, 7.6; ADR 0003).
//!
//! The source object and core parser determine token boundaries. Only fixed-size
//! binary security fields can bypass EOL normalization, after a length mismatch.

use crate::{Document, ObjectSource, repairs};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Diagnostic, Diagnostics, Error, ObjectId, Position, Result};
use pagura_crypto::standard::{self, FileKey, SecurityParams};

pub(crate) fn position(document: &Document, id: Option<ObjectId>) -> Result<Position> {
    id.map_or(Ok(Position::Unknown), |id| {
        Ok(document.object(id)?.source().position())
    })
}

pub(crate) fn hashes(
    document: &Document,
    id: Option<ObjectId>,
    params: &mut SecurityParams,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let expected = if params.revision >= 5 { 48 } else { 32 };
    if params.owner_hash.len() != expected || params.user_hash.len() != expected {
        literal_hashes(document, id, params, diagnostics)?;
    }
    if params.revision >= 5
        && [&params.owner_hash, &params.user_hash]
            .iter()
            .any(|hash| zero_padded(hash, expected))
    {
        let mut diagnostic = Diagnostic::new(
            repairs::HASH_PADDING,
            "zero padding after fixed-length security password hashes discarded",
        )
        .at(position(document, id)?);
        if let Some(id) = id {
            diagnostic = diagnostic.in_object(id);
        }
        diagnostics.repair(diagnostic)?;
        for hash in [&mut params.owner_hash, &mut params.user_hash] {
            if zero_padded(hash, expected) {
                hash.truncate(expected);
            }
        }
    }
    Ok(())
}

/// A missing V=2 Length defaults to 40 bits (Table 20). Only one successful
/// authentication at another legal size can establish a replacement; the
/// supplied password and all other parameters stay fixed (Algorithms 2, 6, 7).
/// No cipher or password algorithm is duplicated in the document layer.
pub(crate) fn authenticate(
    document: &Document,
    id: Option<ObjectId>,
    params: &SecurityParams,
    missing_length: bool,
    diagnostics: &mut Diagnostics,
) -> Result<FileKey> {
    let original = match standard::authenticate(params, &document.options.password) {
        Ok(key) => return Ok(key),
        Err(error) => error,
    };
    if !missing_length
        || params.version != 2
        || params.revision != 3
        || original.kind != pagura_core::ErrorKind::Password
    {
        return Err(original);
    }
    let mut candidate = params.clone();
    let mut found = None;
    // The 5-byte default has already failed. Each other size is tried once.
    for length in 6..=16 {
        candidate.key_length = length;
        match standard::authenticate(&candidate, &document.options.password) {
            Ok(key) if found.is_none() => found = Some(key),
            Ok(_) => return Err(original),
            Err(error) if error.kind == pagura_core::ErrorKind::Password => {}
            Err(error) => return Err(error),
        }
    }
    let Some(key) = found else {
        return Err(original);
    };
    let mut diagnostic = Diagnostic::new(
        repairs::KEY_LENGTH_MISSING,
        "missing legacy Length recovered from a unique authenticated key size",
    )
    .at(position(document, id)?);
    if let Some(id) = id {
        diagnostic = diagnostic.in_object(id);
    }
    diagnostics.repair(diagnostic)?;
    Ok(key)
}

fn zero_padded(hash: &[u8], expected: usize) -> bool {
    hash.len() > expected
        && hash
            .get(expected..)
            .is_some_and(|tail| tail.iter().all(|&byte| byte == 0))
}

fn literal_hashes(
    document: &Document,
    id: Option<ObjectId>,
    params: &mut SecurityParams,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let Some(id) = id else {
        return Ok(());
    };
    let object = document.object(id)?;
    let ObjectSource::File(range) = object.source() else {
        return Ok(());
    };
    let Some(bytes) = document.original_bytes().get(..range.end) else {
        return Ok(());
    };
    let mut parser = Parser::with_offset(bytes, range.start, Origin::File, document.limits())?
        .with_recovery(document.recovery());
    parser.unsigned()?;
    parser.unsigned()?;
    if !parser.keyword(b"obj") {
        return Ok(());
    }
    parser.skip_whitespace();
    if !parser
        .data()
        .get(parser.offset()..)
        .is_some_and(|rest| rest.starts_with(b"<<"))
    {
        return Ok(());
    }
    parser.seek(parser.offset() + 2)?;
    let expected = if params.revision >= 5 { 48 } else { 32 };
    if expected > document.limits().string_bytes {
        return Err(
            Error::limit("recovered security hash exceeds the string budget").in_object(id),
        );
    }
    let mut replacements: [Option<(Vec<u8>, Position)>; 2] = [None, None];
    loop {
        parser.skip_whitespace();
        if parser
            .data()
            .get(parser.offset()..)
            .is_some_and(|rest| rest.starts_with(b">>"))
        {
            break;
        }
        let key = parser.parse()?;
        parser.skip_whitespace();
        let start = parser.offset();
        let value = parser.parse()?;
        if let Some(index) = key
            .as_name()
            .and_then(|key| [b"O".as_slice(), b"U"].iter().position(|&name| key == name))
        {
            // A later nonliteral value must also shadow an earlier literal.
            replacements[index] = None;
            if bytes.get(start) == Some(&b'(')
                && let Some(token) = bytes.get(start..parser.offset())
                && let Some(raw) = raw_literal(token, expected)
                && value.as_string() != Some(raw.as_slice())
            {
                replacements[index] = Some((raw, Position::File(start)));
            }
        }
    }
    if let Some((_, position)) = replacements.iter().flatten().next() {
        diagnostics.repair(
            Diagnostic::new(
                repairs::LITERAL_HASH,
                "binary literal password hashes recovered without EOL normalization",
            )
            .at(*position)
            .in_object(id),
        )?;
        for (target, replacement) in [&mut params.owner_hash, &mut params.user_hash]
            .into_iter()
            .zip(replacements)
        {
            if let Some((raw, _)) = replacement {
                *target = raw;
            }
        }
    }
    Ok(())
}

/// Core parsing has already validated nesting and delimited the literal. Decode
/// escapes as 7.3.4.2 requires, preserving only unescaped physical CR/LF bytes.
/// The fixed expected hash length bounds allocation independently of token size.
fn raw_literal(token: &[u8], expected: usize) -> Option<Vec<u8>> {
    let bytes = token.strip_prefix(b"(")?.strip_suffix(b")")?;
    let mut result = Vec::with_capacity(expected);
    let mut iter = bytes.iter().copied().peekable();
    while let Some(byte) = iter.next() {
        let byte = if byte == b'\\' {
            match iter.next()? {
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'b' => 8,
                b'f' => 12,
                b'\r' => {
                    if iter.peek() == Some(&b'\n') {
                        iter.next();
                    }
                    continue;
                }
                b'\n' => continue,
                first @ b'0'..=b'7' => {
                    let mut code = u16::from(first - b'0');
                    for _ in 0..2 {
                        if let Some(next @ b'0'..=b'7') = iter.peek().copied() {
                            iter.next();
                            code = code * 8 + u16::from(next - b'0');
                        } else {
                            break;
                        }
                    }
                    code.to_le_bytes()[0]
                }
                other => other,
            }
        } else {
            byte
        };
        if result.len() == expected {
            return None;
        }
        result.push(byte);
    }
    (result.len() == expected).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::raw_literal;

    #[test]
    fn raw_security_literals_preserve_physical_eols_and_pdf_escape_semantics() {
        for (token, expected) in [
            (b"(\r\n)".as_slice(), b"\r\n".as_slice()),
            (b"(\\r\\n)", b"\r\n"),
            (b"(\\015\\012)", b"\r\n"),
            (b"(\\777)", b"\xff"),
            (b"(a\\\r\nb)", b"ab"),
            (b"(a\\\nb)", b"ab"),
            (b"(\\(x\\)\\\\)", b"(x)\\"),
            (b"(a(b)c)", b"a(b)c"),
        ] {
            assert_eq!(
                raw_literal(token, expected.len()).as_deref(),
                Some(expected)
            );
        }
        assert!(raw_literal(b"(long)", 3).is_none());
        assert!(raw_literal(b"(short)", 6).is_none());
        assert!(raw_literal(b"<00>", 1).is_none());
    }
}
