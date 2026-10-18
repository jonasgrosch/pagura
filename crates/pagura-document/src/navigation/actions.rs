//! Actions are inert data, including their ordered Next graph (12.6).
use super::destinations::{self, Destination, RemoteDestination};
use crate::view::{Context, Source, identity};
use crate::{Document, IndirectObject, repairs};
use pagura_core::{Dictionary, Error, Name, Result, Value};
use std::collections::BTreeSet;

/// Standard Named actions (12.6.4.12), with extension names preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedAction<'a> {
    /// Advance one page.
    NextPage,
    /// Go back one page.
    PrevPage,
    /// Go to the first page.
    FirstPage,
    /// Go to the last page.
    LastPage,
    /// An extension name, without implied execution support.
    Other(&'a Name),
}
/// Script source, never evaluated by Pagura (12.6.4.17).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Script<'a> {
    /// String bytes, preserving their encoding.
    String(&'a [u8]),
    /// A script stream; the caller can use Document::stream_data to inspect it.
    Stream(&'a IndirectObject),
}
/// An action's data. Every variant is inert; the host chooses whether and how
/// to activate it. File specifications and platform parameters remain source data.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionKind<'a> {
    /// Jump in this document.
    GoTo(Destination),
    /// A destination in an external file (12.6.4.3).
    GoToR {
        /// File specification, never opened.
        file: &'a Value,
        /// Remote page number or name.
        destination: RemoteDestination<'a>,
        /// Optional host window preference.
        new_window: Option<bool>,
    },
    /// URI and catalog Base kept separately, for host-side RFC 3986 resolution.
    Uri {
        /// URI bytes (UTF-8 per Table 210).
        uri: &'a [u8],
        /// Optional catalog URI Base, never guessed from the host filesystem.
        base: Option<&'a [u8]>,
        /// Append clicked user-space coordinates when activating.
        is_map: bool,
    },
    /// A standard or extension navigation command.
    Named(NamedAction<'a>),
    /// Launch data, never used to start a process or open a file.
    Launch {
        /// Optional file specification; legacy platform entries remain in Action::dictionary.
        file: Option<&'a Value>,
        /// Optional host window preference.
        new_window: Option<bool>,
    },
    /// Script data, never executed.
    JavaScript(Script<'a>),
    /// Another action type, listed without execution semantics.
    Other(&'a Name),
}
/// One action in a flat preorder sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct Action<'a> {
    /// Parent action index, or None for the initial action.
    pub parent: Option<usize>,
    /// Parsed action data.
    pub kind: ActionKind<'a>,
    /// Unmodified source dictionary, retaining extension/platform entries.
    pub dictionary: &'a Dictionary,
}
/// An inert action/Next tree in depth-first execution order (12.6.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ActionSequence<'a> {
    /// Flat actions; parent indexes refer to earlier entries. Reused acyclic
    /// subgraphs occur each time; only back edges are omitted with a repair.
    pub actions: Vec<Action<'a>>,
}
fn boolean(
    ctx: &mut Context<'_>,
    d: &Dictionary,
    key: &[u8],
    source: Source,
) -> Result<Option<bool>> {
    d.get(key)
        .map(|v| {
            ctx.doc.resolve(v).and_then(|v| {
                if v.is_null() {
                    Ok(None)
                } else if let Value::Boolean(b) = v {
                    Ok(Some(*b))
                } else {
                    Err(source.error(Error::syntax("action flag must be boolean")))
                }
            })
        })
        .transpose()
        .map(Option::flatten)
}
fn file<'a>(ctx: &mut Context<'a>, value: &'a Value, source: Source) -> Result<&'a Value> {
    let (v, source) = ctx.resolve(value, source)?;
    if matches!(v, Value::String(_) | Value::Dictionary(_)) {
        Ok(v)
    } else {
        Err(source.error(Error::syntax("invalid action file specification")))
    }
}
fn kind<'a>(ctx: &mut Context<'a>, d: &'a Dictionary, source: Source) -> Result<ActionKind<'a>> {
    ctx.type_name(d, b"Action", source)?;
    let name = ctx
        .required(d, b"S", source)?
        .as_name()
        .ok_or_else(|| source.error(Error::syntax("action S must be a name")))?;
    Ok(match name.as_bytes() {
        b"GoTo" => ActionKind::GoTo(destinations::resolve(
            ctx,
            d.get(b"D")
                .ok_or_else(|| source.error(Error::syntax("GoTo requires D")))?,
            source,
        )?),
        b"GoToR" => ActionKind::GoToR {
            file: file(
                ctx,
                d.get(b"F")
                    .ok_or_else(|| source.error(Error::syntax("GoToR requires F")))?,
                source,
            )?,
            destination: destinations::remote(
                ctx,
                d.get(b"D")
                    .ok_or_else(|| source.error(Error::syntax("GoToR requires D")))?,
                source,
            )?,
            new_window: boolean(ctx, d, b"NewWindow", source)?,
        },
        b"URI" => {
            let uri = ctx
                .required(d, b"URI", source)?
                .as_string()
                .ok_or_else(|| source.error(Error::syntax("URI action requires a string")))?;
            let (catalog, catalog_source) = ctx.catalog()?;
            let base = if let Some(uri) = ctx.present(catalog, b"URI", catalog_source)? {
                let (uri, source) = ctx.dictionary(uri, catalog_source)?;
                ctx.entry(uri, b"Base", source)?
                    .map(|v| {
                        v.as_string()
                            .ok_or_else(|| source.error(Error::syntax("URI Base must be a string")))
                    })
                    .transpose()?
            } else {
                None
            };
            ActionKind::Uri {
                uri,
                base,
                is_map: boolean(ctx, d, b"IsMap", source)?.unwrap_or(false),
            }
        }
        b"Named" => {
            let n = ctx
                .required(d, b"N", source)?
                .as_name()
                .ok_or_else(|| source.error(Error::syntax("Named action requires a name")))?;
            ActionKind::Named(match n.as_bytes() {
                b"NextPage" => NamedAction::NextPage,
                b"PrevPage" => NamedAction::PrevPage,
                b"FirstPage" => NamedAction::FirstPage,
                b"LastPage" => NamedAction::LastPage,
                _ => NamedAction::Other(n),
            })
        }
        b"Launch" => parse_launch(ctx, d, source)?,
        b"JavaScript" => {
            let value = d
                .get(b"JS")
                .ok_or_else(|| source.error(Error::syntax("JavaScript requires JS")))?;
            let (script, source) = ctx.resolve(value, source)?;
            let script = if let Some(bytes) = script.as_string() {
                Script::String(bytes)
            } else {
                let id = source.object.ok_or_else(|| {
                    source.error(Error::syntax("script must be a string or stream"))
                })?;
                let object = ctx.doc.object(id)?;
                if object.stream_range().is_none() || identity(object.value()) != identity(script) {
                    return Err(source.error(Error::syntax("script must be a string or stream")));
                }
                Script::Stream(object)
            };
            ActionKind::JavaScript(script)
        }
        _ => ActionKind::Other(name),
    })
}
pub(crate) fn read<'a>(doc: &'a Document, value: &'a Value) -> Result<ActionSequence<'a>> {
    read_with(&mut Context::new(doc), value, Source::default())
}
pub(crate) fn read_with<'a>(
    ctx: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> Result<ActionSequence<'a>> {
    let mut actions = Vec::new();
    let mut active = BTreeSet::new();
    let mut stack = vec![(value, source, None, 0, false)];
    while let Some((value, source, parent, depth, exit)) = stack.pop() {
        if exit {
            active.remove(&identity(value));
            continue;
        }
        ctx.step(depth, source)?;
        let (value, source) = ctx.resolve(value, source)?;
        ctx.allocate::<usize>(8, source)?;
        if !active.insert(identity(value)) {
            ctx.repair(
                repairs::ACTION_CYCLE,
                "cyclic action Next edge omitted (12.6.2)",
                source,
            )?;
            continue;
        }
        let (dictionary, source) = ctx.dictionary(value, source)?;
        ctx.allocate::<Action<'_>>(1, source)?;
        let index = actions.len();
        actions.push(Action {
            parent,
            kind: kind(ctx, dictionary, source)?,
            dictionary,
        });
        ctx.allocate::<(&Value, Source, Option<usize>, usize, bool)>(1, source)?;
        stack.push((value, source, parent, depth, true));
        if let Some(next) = dictionary.get(b"Next") {
            let (next, next_source) = ctx.resolve(next, source)?;
            if next.is_null() {
                continue;
            }
            let children = if let Value::Array(children) = next {
                children.as_slice()
            } else {
                std::slice::from_ref(next)
            };
            ctx.allocate::<(&Value, Source, Option<usize>, usize, bool)>(children.len(), source)?;
            for child in children.iter().rev() {
                stack.push((child, next_source, Some(index), depth + 1, false));
            }
        }
    }
    Ok(ActionSequence { actions })
}

fn parse_launch<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
) -> Result<ActionKind<'a>> {
    let file = ctx
        .present(d, b"F", source)?
        .map(|v| file(ctx, v, source))
        .transpose()?;
    if file.is_none() {
        let mut platform = false;
        for key in [b"Win".as_slice(), b"Mac", b"Unix"] {
            platform |= ctx.present(d, key, source)?.is_some();
        }
        if !platform {
            return Err(source.error(Error::syntax(
                "Launch requires a file or platform parameters",
            )));
        }
    }
    Ok(ActionKind::Launch {
        file,
        new_window: boolean(ctx, d, b"NewWindow", source)?,
    })
}
