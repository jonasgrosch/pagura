//! Confirm revision ends by parsing at Prev-chain offsets, not by searching for
//! marker text inside arbitrary document data (7.5.6; 12.8.1).
use super::{RevisionCoverage, byte_range::charge_scan};
use crate::navigation::context::{Context, Source};
use pagura_core::{
    Error, Position, Result, Value,
    syntax::{Origin, Parser, is_whitespace},
};

#[derive(Debug, Clone, Copy)]
pub(super) struct Boundary {
    eof: usize,
    eol: usize,
}

pub(super) fn read(ctx: &mut Context<'_>, scanned: &mut usize) -> Result<Vec<Option<Boundary>>> {
    let revisions = ctx.doc.revisions();
    ctx.allocate::<Option<Boundary>>(revisions.len(), Source::default())?;
    revisions
        .iter()
        .enumerate()
        .map(|(index, revision)| {
            let source = Source {
                object: None,
                position: Position::File(revision.offset()),
            };
            let end = index
                .checked_sub(1)
                .map_or(ctx.doc.original_bytes().len(), |i| revisions[i].offset());
            if end <= revision.offset() {
                return Ok(None);
            }
            let result = boundary(ctx, revision.offset(), end, source);
            match result {
                Ok((boundary, consumed)) => {
                    charge_scan(ctx.doc, scanned, consumed, source)?;
                    Ok(Some(boundary))
                }
                Err(error) if error.kind == pagura_core::ErrorKind::Limit => {
                    Err(source.error(error))
                }
                Err(_) => Ok(None),
            }
        })
        .collect()
}

pub(super) fn coverage<'a>(
    ctx: &Context<'a>,
    boundaries: &[Option<Boundary>],
    signed_end: usize,
) -> RevisionCoverage<'a> {
    if let Some(index) = boundaries
        .iter()
        .position(|b| b.is_some_and(|b| signed_end == b.eof || signed_end == b.eol))
    {
        let revisions = ctx.doc.revisions();
        RevisionCoverage::Covered {
            index,
            revision: &revisions[index],
            later_revisions: &revisions[..index],
        }
    } else if boundaries.iter().any(Option::is_none) {
        RevisionCoverage::Indeterminate
    } else {
        RevisionCoverage::NotRevisionBoundary
    }
}

fn boundary(
    ctx: &mut Context<'_>,
    offset: usize,
    end: usize,
    source: Source,
) -> Result<(Boundary, usize)> {
    let bytes = ctx.doc.original_bytes();
    let mut parser = Parser::at(&bytes[..end], offset, Origin::File, ctx.doc.limits())?;
    if parser.keyword(b"xref") {
        while !parser.keyword(b"trailer") {
            ctx.step(0, source)?;
            parser.unsigned()?;
            let count = usize::try_from(parser.unsigned()?).map_err(|_| {
                source.error(Error::limit("signature xref count exceeds address space"))
            })?;
            if count > ctx.doc.limits().objects {
                return Err(source.error(Error::limit("signature xref entry budget exceeded")));
            }
            for _ in 0..count {
                ctx.step(0, source)?;
                parser.unsigned()?;
                parser.unsigned()?;
                if !matches!(parser.word()?, b"n" | b"f") {
                    return Err(
                        source.error(Error::syntax("invalid signature revision xref entry"))
                    );
                }
            }
        }
        if parser.parse()?.as_dictionary().is_none() {
            return Err(source.error(Error::syntax(
                "signature revision trailer must be a dictionary",
            )));
        }
    } else {
        parser.unsigned()?;
        parser.unsigned()?;
        if !parser.keyword(b"obj") {
            return Err(source.error(Error::syntax("signature revision lacks xref object")));
        }
        let value = parser.parse()?;
        let dictionary = value
            .as_dictionary()
            .ok_or_else(|| source.error(Error::syntax("signature xref stream lacks dictionary")))?;
        let length = dictionary
            .get(b"Length")
            .and_then(Value::as_integer)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n <= ctx.doc.limits().stream_bytes)
            .ok_or_else(|| {
                source.error(Error::syntax(
                    "signature xref stream needs direct bounded Length",
                ))
            })?;
        if !dictionary.is_name(b"Type", b"XRef") || !parser.keyword(b"stream") {
            return Err(source.error(Error::syntax("signature revision is not an xref stream")));
        }
        let rest = &parser.data()[parser.offset()..];
        let eol = if rest.starts_with(b"\r\n") {
            2
        } else if rest.starts_with(b"\n") || rest.starts_with(b"\r") {
            1
        } else {
            return Err(source.error(Error::syntax("signature xref stream lacks EOL")));
        };
        let stream_end = parser
            .offset()
            .checked_add(eol)
            .and_then(|n| n.checked_add(length))
            .filter(|&n| n <= end)
            .ok_or_else(|| source.error(Error::syntax("signature xref stream exceeds revision")))?;
        parser.seek(stream_end)?;
        if !parser.keyword(b"endstream") || !parser.keyword(b"endobj") {
            return Err(source.error(Error::syntax("signature xref stream lacks terminator")));
        }
    }
    if !parser.keyword(b"startxref") || usize::try_from(parser.unsigned()?).ok() != Some(offset) {
        return Err(source.error(Error::syntax(
            "signature revision startxref does not match Prev chain",
        )));
    }
    // %%EOF is a comment to the ordinary tokenizer. Only PDF whitespace may
    // intervene here; skip_whitespace would swallow the boundary itself.
    let mut at = parser.offset();
    while bytes.get(at).is_some_and(|&b| is_whitespace(b)) && at < end {
        at += 1;
    }
    if !bytes.get(at..end).is_some_and(|b| b.starts_with(b"%%EOF")) {
        return Err(source.error(Error::syntax("signature revision lacks EOF marker")));
    }
    let eof = at + 5;
    let eol = match bytes.get(eof..end) {
        Some(tail) if tail.starts_with(b"\r\n") => eof + 2,
        Some([b'\r' | b'\n', ..]) => eof + 1,
        _ => eof,
    };
    // A marker followed by ordinary text is not an EOF comment boundary.
    if eol == eof && eof < end && !bytes.get(eof).is_some_and(|&b| is_whitespace(b)) {
        return Err(source.error(Error::syntax("signature EOF marker has trailing text")));
    }
    Ok((Boundary { eof, eol }, eol - offset))
}
