//! Bind a signature dictionary to its original Contents token (12.8.3.3.1).
use crate::source::ReadResult;
use crate::view::{Context, Source};
use pagura_core::{
    Dictionary, Error, Name, Value,
    syntax::{Origin, Parser},
};
use std::ops::Range;

pub(super) fn contents_span(
    ctx: &mut Context<'_>,
    target: &Dictionary,
    root: &Value,
    extent: Range<usize>,
    source: Source,
) -> ReadResult<Option<Range<usize>>> {
    let lease = ctx.document.materialize()?;
    let bytes = &lease.bytes()[..extent.end];
    let mut parser = Parser::with_offset(bytes, extent.start, Origin::File, ctx.document.limits())?;
    parser.unsigned()?;
    parser.unsigned()?;
    if !parser.keyword(b"obj") {
        return Err(source
            .error(Error::syntax("signature source lacks object header"))
            .into());
    }
    parser.skip_whitespace();
    let start = parser.offset();
    // Reparse strictly: duplicate keys or other repaired syntax cannot bind a
    // trustworthy physical token. Decrypted text values are deliberately not
    // compared with the original encrypted bytes.
    parser.parse()?;
    if !parser.keyword(b"endobj") {
        return Err(source
            .error(Error::syntax("signature source lacks endobj"))
            .into());
    }
    parser.seek(start)?;
    let Some(path) = dictionary_path(ctx, root, target, source)? else {
        return Ok(None);
    };
    // Skip only siblings along the path. Parsing the selected subtree at each
    // level would repeatedly allocate its values on deeply nested direct data.
    for step in path {
        match step {
            Step::Key(key) => {
                let Some(start) = entry_start(ctx, &mut parser, key.as_bytes(), source)? else {
                    return Ok(None);
                };
                parser.seek(start)?;
            }
            Step::Index(index) => {
                if parser.peek() != Some(b'[') {
                    return Ok(None);
                }
                parser.seek(parser.offset() + 1)?;
                for _ in 0..index {
                    ctx.step(0, source)?;
                    parser.parse()?;
                }
                parser.skip_whitespace();
            }
        }
    }
    let Some(start) = entry_start(ctx, &mut parser, b"Contents", source)? else {
        return Ok(None);
    };
    parser.parse()?;
    Ok(Some(start..parser.offset()))
}

fn entry_start(
    ctx: &mut Context<'_>,
    parser: &mut Parser<'_>,
    key: &[u8],
    source: Source,
) -> ReadResult<Option<usize>> {
    parser.skip_whitespace();
    let at = parser.offset();
    if !parser
        .data()
        .get(at..)
        .is_some_and(|v| v.starts_with(b"<<"))
    {
        return Ok(None);
    }
    parser.seek(at + 2)?;
    loop {
        ctx.step(0, source)?;
        parser.skip_whitespace();
        if parser
            .data()
            .get(parser.offset()..)
            .is_some_and(|v| v.starts_with(b">>"))
        {
            return Ok(None);
        }
        let name = parser.parse()?;
        let name = name.as_name().ok_or_else(|| {
            source.error(Error::syntax("signature dictionary key must be a name"))
        })?;
        parser.skip_whitespace();
        let start = parser.offset();
        if name.as_bytes() == key {
            return Ok(Some(start));
        }
        parser.parse()?;
    }
}

#[derive(Clone, Copy)]
enum Step<'a> {
    Key(&'a Name),
    Index(usize),
}
struct Node<'a> {
    value: &'a Value,
    parent: Option<(usize, Step<'a>)>,
    depth: usize,
}

/// Find the dictionary by identity in the parsed containing object, then follow
/// that exact path in the original syntax. Parent links avoid cloning a path
/// for every node and keep the traversal iterative on hostile direct trees.
fn dictionary_path<'a>(
    ctx: &mut Context<'_>,
    root: &'a Value,
    target: &Dictionary,
    source: Source,
) -> ReadResult<Option<Vec<Step<'a>>>> {
    ctx.allocate::<Node<'_>>(2, source)?;
    let mut nodes = vec![Node {
        value: root,
        parent: None,
        depth: 0,
    }];
    let mut cursor = 0;
    while cursor < nodes.len() {
        let node = &nodes[cursor];
        ctx.step(node.depth, source)?;
        if node
            .value
            .as_dictionary()
            .is_some_and(|d| std::ptr::eq(d, target))
        {
            ctx.allocate::<Step<'_>>(node.depth, source)?;
            let mut path = Vec::with_capacity(node.depth);
            while let Some((parent, step)) = nodes[cursor].parent {
                path.push(step);
                cursor = parent;
            }
            path.reverse();
            return Ok(Some(path));
        }
        let depth = node.depth.saturating_add(1);
        match node.value {
            Value::Array(values) => {
                ctx.allocate::<Node<'_>>(values.len().saturating_mul(2), source)?;
                nodes.extend(values.iter().enumerate().map(|(index, value)| Node {
                    value,
                    parent: Some((cursor, Step::Index(index))),
                    depth,
                }));
            }
            Value::Dictionary(dictionary) => {
                ctx.allocate::<Node<'_>>(dictionary.len().saturating_mul(2), source)?;
                nodes.extend(dictionary.iter().map(|(key, value)| Node {
                    value,
                    parent: Some((cursor, Step::Key(key))),
                    depth,
                }));
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(None)
}
