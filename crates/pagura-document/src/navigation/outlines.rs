//! Flat, bounded outline traversal (12.3.3, Tables 150–152).
use super::destinations::{self, Target};
use crate::source::{ReadFault, ReadResult};
use crate::view::{Context, Source, ValueIdentity, identity, number};
use crate::{Document, repairs};
use pagura_core::{Dictionary, Error, ErrorKind, Value};
use std::collections::BTreeSet;

/// Outline item flags (Table 152).
const ITALIC: i64 = 1;
const BOLD: i64 = 2;
const FLAGS: i64 = ITALIC | BOLD;

/// One outline item in preorder. Parent indexes avoid recursive result storage.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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

/// The children of one outline node, while the forward chain is walked.
#[derive(Clone, Copy)]
struct Level {
    /// The node whose First/Last chain this is.
    parent: ValueIdentity,
    /// Index of that node's item, or `None` for the outline root.
    owner: Option<usize>,
    /// The node's Last entry, checked against the chain's end.
    last: Option<ValueIdentity>,
    source: Source,
    depth: usize,
}

/// An outline item still to visit.
struct Pending<'a> {
    value: &'a Value,
    level: Level,
    previous: Option<ValueIdentity>,
}

/// An item's declared Count and its children counted so far (12.3.3).
struct Count {
    declared: Option<i64>,
    source: Source,
    children: usize,
}

fn edge<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> ReadResult<Option<&'a Value>> {
    let Some(value) = dictionary.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    match context.resolve(value, source) {
        Ok((value, _)) if value.is_null() => return Ok(None),
        Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::MissingObject => {}
        Err(fault) => return Err(fault),
        _ => {}
    }
    if !matches!(value, Value::Reference(_)) {
        context.repair(
            repairs::OUTLINE_LINKS,
            "direct outline link accepted (12.3.3, Tables 150–151)",
            source,
        )?;
    }
    Ok(Some(value))
}

fn child_level<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
    owner: Option<usize>,
    depth: usize,
) -> ReadResult<Option<(&'a Value, Level)>> {
    let (dictionary, source) = context.dictionary(value, source)?;
    let first = edge(context, dictionary, b"First", source)?;
    let last = edge(context, dictionary, b"Last", source)?
        .map(|v| reciprocal(context, v, source))
        .transpose()?
        .flatten();
    if first.is_some() != last.is_some() {
        context.repair(
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
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    expected: Option<ValueIdentity>,
    source: Source,
) -> ReadResult<()> {
    let actual = edge(context, dictionary, key, source)?
        .map(|v| reciprocal(context, v, source))
        .transpose()?
        .flatten();
    if actual != expected {
        context.repair(
            repairs::OUTLINE_LINKS,
            "outline Parent/Prev disagrees with forward chain; forward chain used (12.3.3)",
            source,
        )?;
    }
    Ok(())
}

fn appearance<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<([f64; 3], i64)> {
    let mut color = [0.0; 3];
    if let Some(value) = context.entry(dictionary, b"C", source)? {
        let values = value.as_array().filter(|v| v.len() == 3).ok_or_else(|| {
            source.error(Error::syntax("outline color requires three components"))
        })?;
        for (out, value) in color.iter_mut().zip(values) {
            *out = number(context.resolve(value, source)?.0, source)?;
            if !(0.0..=1.0).contains(out) {
                return Err(
                    (source.error(Error::syntax("outline color outside unit range"))).into(),
                );
            }
        }
    }
    let flags = context
        .entry(dictionary, b"F", source)?
        .map(|v| {
            v.as_integer()
                .ok_or_else(|| source.error(Error::syntax("outline flags must be integer")))
        })
        .transpose()?
        .unwrap_or(0);
    if flags & !FLAGS != 0 {
        return Err((source.error(Error::syntax("reserved outline style flags"))).into());
    }
    Ok((color, flags))
}

pub(super) fn read(document: &Document) -> ReadResult<Vec<OutlineItem<'_>>> {
    let mut context = Context::new(document);
    let (catalog, catalog_source) = context.catalog()?;
    let Some(root) = context.present(catalog, b"Outlines", catalog_source)? else {
        return Ok(Vec::new());
    };
    let (root, source) = context.resolve(root, catalog_source)?;
    let (root_dictionary, source) = context.dictionary(root, source)?;
    if context
        .entry(root_dictionary, b"Type", source)?
        .is_some_and(|v| v.as_name().is_none_or(|n| n != b"Outlines"))
    {
        context.repair(
            repairs::OUTLINE_TYPE,
            "incorrect optional outline Type ignored in catalog Outlines context \
             (12.3.3, Table 150)",
            source,
        )?;
    }
    let mut items = Vec::new();
    let mut counts = Vec::new();
    let mut visited = BTreeSet::new();
    let mut stack = Vec::new();
    if let Some((first, level)) = child_level(&mut context, root, source, None, 0)? {
        stack.push(Pending {
            value: first,
            level,
            previous: None,
        });
    }
    while let Some(Pending {
        value,
        level,
        previous,
    }) = stack.pop()
    {
        context.step(level.depth, level.source)?;
        let (value, source) = context.resolve(value, level.source)?;
        context.index_entry(source)?;
        if !visited.insert(identity(value)) {
            context.repair(
                repairs::OUTLINE_CYCLE,
                "repeated outline item omitted (12.3.3)",
                source,
            )?;
            continue;
        }
        let (dictionary, source) = context.dictionary(value, source)?;
        check_link(
            &mut context,
            dictionary,
            b"Parent",
            Some(level.parent),
            source,
        )?;
        check_link(&mut context, dictionary, b"Prev", previous, source)?;
        let (item, declared) = item(&mut context, dictionary, &level, source)?;
        let index = items.len();
        items.push(item);
        counts.push(Count {
            declared,
            source,
            children: 0,
        });
        if let Some(next) = edge(&mut context, dictionary, b"Next", source)? {
            context.allocate::<Pending<'_>>(1, source)?;
            stack.push(Pending {
                value: next,
                level,
                previous: Some(identity(value)),
            });
        } else if level.last != Some(identity(value)) {
            context.repair(
                repairs::OUTLINE_LINKS,
                "outline Last differs from forward-chain endpoint; endpoint used (12.3.3)",
                level.source,
            )?;
        }
        if let Some((first, child)) =
            child_level(&mut context, value, source, Some(index), level.depth + 1)?
        {
            context.allocate::<Pending<'_>>(1, source)?;
            stack.push(Pending {
                value: first,
                level: child,
                previous: None,
            });
        }
    }
    validate_counts(&mut context, &items, counts, root_dictionary, source)?;
    Ok(items)
}

/// Reads one item's own entries, returning its declared Count for validation.
fn item<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    level: &Level,
    source: Source,
) -> ReadResult<(OutlineItem<'a>, Option<i64>)> {
    let title = context
        .required(dictionary, b"Title", source)?
        .as_string()
        .ok_or_else(|| source.error(Error::syntax("outline Title must be a text string")))?;
    let title = context.text(title, source)?;
    let count = context
        .entry(dictionary, b"Count", source)?
        .map(|v| {
            v.as_integer()
                .ok_or_else(|| source.error(Error::syntax("outline Count must be integer")))
        })
        .transpose()?;
    let (color, flags) = appearance(context, dictionary, source)?;
    context.allocate::<OutlineItem<'_>>(1, source)?;
    context.allocate::<Count>(1, source)?;
    let item = OutlineItem {
        parent: level.owner,
        depth: level.depth,
        title,
        open: count.is_some_and(|c| c > 0),
        color,
        italic: flags & ITALIC != 0,
        bold: flags & BOLD != 0,
        target: destinations::target(context, dictionary, source)?,
        dictionary,
    };
    Ok((item, count))
}

fn validate_counts<'a>(
    context: &mut Context<'a>,
    items: &[OutlineItem<'a>],
    mut counts: Vec<Count>,
    root_dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<()> {
    let mut visible = 0usize;
    // Children follow their parent in preorder, so a reverse pass sees every
    // child's visible total before its parent's.
    for (index, item) in items.iter().enumerate().rev() {
        let Count {
            declared,
            source,
            children,
        } = counts[index];
        if (children > 0 && declared.is_none())
            || declared
                .is_some_and(|n| n.unsigned_abs() != pagura_core::convert::usize_to_u64(children))
        {
            context.repair(
                repairs::OUTLINE_COUNT,
                "outline Count magnitude corrected; sign retained (12.3.3)",
                source,
            )?;
        }
        let visible_here = 1 + if item.open { children } else { 0 };
        if let Some(parent) = item.parent {
            counts[parent].children += visible_here;
        } else {
            visible += visible_here;
        }
    }
    let declared = context
        .entry(root_dictionary, b"Count", source)?
        .and_then(Value::as_integer);
    if (visible > 0 || declared.is_some())
        && declared.and_then(|n| usize::try_from(n).ok()) != Some(visible)
    {
        context.repair(
            repairs::OUTLINE_COUNT,
            "outline root Count corrected to visible items (12.3.3)",
            source,
        )?;
    }
    Ok(())
}

fn reciprocal<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> ReadResult<Option<ValueIdentity>> {
    match context.resolve(value, source) {
        Ok((value, _)) => Ok(Some(identity(value))),
        Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::MissingObject => {
            context.repair(
                repairs::OUTLINE_LINKS,
                "dangling reciprocal outline link ignored; forward chain used (12.3.3)",
                source,
            )?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
