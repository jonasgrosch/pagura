//! Strict Prev-chain revision terminators (7.5.6).
use crate::navigation::context::{Context, Source};
use pagura_core::{
    Error, Result, Value,
    syntax::{Origin, Parser, is_whitespace},
};
#[derive(Debug, Clone, Copy)]
pub(crate) struct Boundary {
    pub(crate) eof: usize,
    pub(crate) eol: usize,
}
pub(crate) fn boundary(
    ctx: &mut Context<'_>,
    revision: &crate::Revision,
    end: usize,
    source: Source,
) -> Result<(Boundary, usize)> {
    let bytes = ctx.doc.original_bytes();
    let offset = revision.end_section;
    let mut parser = Parser::with_offset(&bytes[..end], offset, Origin::File, ctx.doc.limits())?;
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
    if !parser.keyword(b"startxref")
        || usize::try_from(parser.unsigned()?).ok() != Some(revision.offset())
    {
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
