//! Flat, bounded outline traversal (12.3.3, Tables 150–152).
use super::destinations::{self, Target};
use crate::view::{Context, Source, identity, number, text};
use crate::{Document, repairs};
use pagura_core::{Dictionary, Error, Result, Value};
use std::collections::BTreeSet;

/// One outline item in preorder. Parent indexes avoid recursive result storage.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem<'a> {
    /// Parent item index, or None for a top-level item.
    pub parent: Option<usize>,
    /// Nesting level, starting at zero.
    pub depth: usize,
    /// Decoded Unicode title (7.9.2.2).
    pub title: String,
    /// Whether immediate children are initially displayed, from Count's sign.
    pub open: bool,
    /// DeviceRGB text colour, each component in 0..=1.
    pub color: [f64; 3],
    /// Italic text style.
    pub italic: bool,
    /// Bold text style.
    pub bold: bool,
    /// Destination or inert action data, if present.
    pub target: Option<Target<'a>>,
    /// Unmodified item dictionary.
    pub dictionary: &'a Dictionary,
}
#[derive(Clone, Copy)]
struct Level {
    parent: usize,
    owner: Option<usize>,
    last: Option<usize>,
    source: Source,
    depth: usize,
}
fn edge<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<&'a Value>> {
    let Some(value) = d.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    match ctx.resolve(value, source) {
        Ok((value, _)) if value.is_null() => return Ok(None),
        Err(error) if error.kind != pagura_core::ErrorKind::MissingObject => return Err(error),
        _ => {}
    }
    if !matches!(value, Value::Reference(_)) {
        ctx.repair(
            repairs::OUTLINE_LINKS,
            "direct outline link accepted (12.3.3, Tables 150–151)",
            source,
        )?;
    }
    Ok(Some(value))
}
fn child_level<'a>(
    ctx: &mut Context<'a>,
    value: &'a Value,
    source: Source,
    owner: Option<usize>,
    depth: usize,
) -> Result<Option<(&'a Value, Level)>> {
    let (d, source) = ctx.dictionary(value, source)?;
    let first = edge(ctx, d, b"First", source)?;
    let last = edge(ctx, d, b"Last", source)?
        .map(|v| reciprocal(ctx, v, source))
        .transpose()?
        .flatten();
    if first.is_some() != last.is_some() {
        ctx.repair(
            repairs::OUTLINE_LINKS,
            "inconsistent outline First/Last; forward chain used (12.3.3)",
            source,
        )?;
    }
    Ok(first.map(|first| {
        (
            first,
            Level {
                parent: identity(value),
                owner,
                last,
                source,
                depth,
            },
        )
    }))
}
fn check_link<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    key: &[u8],
    expected: Option<usize>,
    source: Source,
) -> Result<()> {
    let actual = edge(ctx, d, key, source)?
        .map(|v| reciprocal(ctx, v, source))
        .transpose()?
        .flatten();
    if actual != expected {
        ctx.repair(
            repairs::OUTLINE_LINKS,
            "outline Parent/Prev disagrees with forward chain; forward chain used (12.3.3)",
            source,
        )?;
    }
    Ok(())
}
fn appearance<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<([f64; 3], i64)> {
    let mut color = [0.0; 3];
    if let Some(value) = ctx.entry(d, b"C", source)? {
        let values = value.as_array().filter(|v| v.len() == 3).ok_or_else(|| {
            source.error(Error::syntax("outline color requires three components"))
        })?;
        for (out, value) in color.iter_mut().zip(values) {
            *out = number(ctx.doc.resolve(value)?, source)?;
            if !(0.0..=1.0).contains(out) {
                return Err(source.error(Error::syntax("outline color outside unit range")));
            }
        }
    }
    let flags = ctx
        .entry(d, b"F", source)?
        .map(|v| {
            v.as_integer()
                .ok_or_else(|| source.error(Error::syntax("outline flags must be integer")))
        })
        .transpose()?
        .unwrap_or(0);
    if !(0..=3).contains(&flags) {
        return Err(source.error(Error::syntax("reserved outline style flags")));
    }
    Ok((color, flags))
}
pub(super) fn read(doc: &Document) -> Result<Vec<OutlineItem<'_>>> {
    let mut ctx = Context::new(doc);
    let (catalog, catalog_source) = ctx.catalog()?;
    let Some(root) = ctx.present(catalog, b"Outlines", catalog_source)? else {
        return Ok(Vec::new());
    };
    let (root, source) = ctx.resolve(root, catalog_source)?;
    let (root_dictionary, source) = ctx.dictionary(root, source)?;
    if ctx
        .entry(root_dictionary, b"Type", source)?
        .is_some_and(|v| v.as_name().is_none_or(|n| n != b"Outlines"))
    {
        ctx.repair(repairs::OUTLINE_TYPE,"incorrect optional outline Type ignored in catalog Outlines context (12.3.3, Table 150)",source)?;
    }
    let mut items = Vec::new();
    let mut counts = Vec::new();
    let mut visited = BTreeSet::new();
    let mut stack = Vec::new();
    if let Some((first, level)) = child_level(&mut ctx, root, source, None, 0)? {
        stack.push((first, level, None));
    }
    while let Some((value, level, previous)) = stack.pop() {
        ctx.step(level.depth, level.source)?;
        let (value, source) = ctx.resolve(value, level.source)?;
        ctx.allocate::<usize>(8, source)?;
        if !visited.insert(identity(value)) {
            ctx.repair(
                repairs::OUTLINE_CYCLE,
                "repeated outline item omitted (12.3.3)",
                source,
            )?;
            continue;
        }
        let (d, source) = ctx.dictionary(value, source)?;
        check_link(&mut ctx, d, b"Parent", Some(level.parent), source)?;
        check_link(&mut ctx, d, b"Prev", previous, source)?;
        let title = ctx
            .required(d, b"Title", source)?
            .as_string()
            .ok_or_else(|| source.error(Error::syntax("outline Title must be a text string")))?;
        let title = text::decode(&mut ctx, title, source)?;
        let count = ctx
            .entry(d, b"Count", source)?
            .map(|v| {
                v.as_integer()
                    .ok_or_else(|| source.error(Error::syntax("outline Count must be integer")))
            })
            .transpose()?;
        let (color, flags) = appearance(&mut ctx, d, source)?;
        ctx.allocate::<OutlineItem<'_>>(1, source)?;
        ctx.allocate::<(Option<i64>, Source, usize)>(1, source)?;
        let index = items.len();
        items.push(OutlineItem {
            parent: level.owner,
            depth: level.depth,
            title,
            open: count.is_some_and(|c| c > 0),
            color,
            italic: flags & 1 != 0,
            bold: flags & 2 != 0,
            target: destinations::target(&mut ctx, d, source)?,
            dictionary: d,
        });
        counts.push((count, source, 0usize));
        if let Some(next) = edge(&mut ctx, d, b"Next", source)? {
            ctx.allocate::<(&Value, Level, Option<usize>)>(1, source)?;
            stack.push((next, level, Some(identity(value))));
        } else if level.last != Some(identity(value)) {
            ctx.repair(
                repairs::OUTLINE_LINKS,
                "outline Last differs from forward-chain endpoint; endpoint used (12.3.3)",
                level.source,
            )?;
        }
        if let Some((first, child)) =
            child_level(&mut ctx, value, source, Some(index), level.depth + 1)?
        {
            ctx.allocate::<(&Value, Level, Option<usize>)>(1, source)?;
            stack.push((first, child, None));
        }
    }
    validate_counts(&mut ctx, &items, counts, root_dictionary, source)?;
    Ok(items)
}

fn validate_counts<'a>(
    ctx: &mut Context<'a>,
    items: &[OutlineItem<'a>],
    mut counts: Vec<(Option<i64>, Source, usize)>,
    root_dictionary: &'a Dictionary,
    source: Source,
) -> Result<()> {
    let mut visible = 0usize;
    for index in (0..items.len()).rev() {
        let (declared, source, children) = counts[index];
        if (children > 0 && declared.is_none())
            || declared
                .is_some_and(|n| n.unsigned_abs() != u64::try_from(children).unwrap_or(u64::MAX))
        {
            ctx.repair(
                repairs::OUTLINE_COUNT,
                "outline Count magnitude corrected; sign retained (12.3.3)",
                source,
            )?;
        }
        let visible_here = 1 + if items[index].open { children } else { 0 };
        if let Some(parent) = items[index].parent {
            counts[parent].2 += visible_here;
        } else {
            visible += visible_here;
        }
    }
    let declared = ctx
        .entry(root_dictionary, b"Count", source)?
        .and_then(Value::as_integer);
    if (visible > 0 || declared.is_some())
        && declared.and_then(|n| usize::try_from(n).ok()) != Some(visible)
    {
        ctx.repair(
            repairs::OUTLINE_COUNT,
            "outline root Count corrected to visible items (12.3.3)",
            source,
        )?;
    }
    Ok(())
}

fn reciprocal<'a>(
    ctx: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> Result<Option<usize>> {
    match ctx.resolve(value, source) {
        Ok((value, _)) => Ok(Some(identity(value))),
        Err(error) if error.kind == pagura_core::ErrorKind::MissingObject => {
            ctx.repair(
                repairs::OUTLINE_LINKS,
                "dangling reciprocal outline link ignored; forward chain used (12.3.3)",
                source,
            )?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
