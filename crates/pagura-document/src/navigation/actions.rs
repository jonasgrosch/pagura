//! Actions are inert data, including their ordered Next graph (12.6).
use super::destinations::{self, Destination, RemoteDestination};
use crate::source::ReadResult;
use crate::view::{Context, Source, ValueIdentity, identity};
use crate::{Document, IndirectObject, repairs};
use pagura_core::{Dictionary, Error, Name, Value};
use std::collections::BTreeSet;

/// Standard Named actions (12.6.4.12), with extension names preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

/// Script source, never evaluated by Pagura (12.6.4.17), which allows a
/// text string or a text stream and nothing else.
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
#[non_exhaustive]
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
#[non_exhaustive]
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
#[non_exhaustive]
pub struct ActionSequence<'a> {
    /// Flat actions; parent indexes refer to earlier entries. Reused acyclic
    /// subgraphs occur each time; only back edges are omitted with a repair.
    pub actions: Vec<Action<'a>>,
}

fn boolean<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    key: &[u8],
    source: Source,
) -> ReadResult<Option<bool>> {
    match context.entry(dictionary, key, source)? {
        None => Ok(None),
        Some(Value::Boolean(flag)) => Ok(Some(*flag)),
        Some(_) => Err(source
            .error(Error::syntax("action flag must be boolean"))
            .into()),
    }
}

fn file<'a>(context: &mut Context<'a>, value: &'a Value, source: Source) -> ReadResult<&'a Value> {
    let (v, source) = context.resolve(value, source)?;
    if matches!(v, Value::String(_) | Value::Dictionary(_)) {
        Ok(v)
    } else {
        Err(source
            .error(Error::syntax("invalid action file specification"))
            .into())
    }
}

fn kind<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<ActionKind<'a>> {
    context.type_name(dictionary, b"Action", source)?;
    let name = context
        .required(dictionary, b"S", source)?
        .as_name()
        .ok_or_else(|| source.error(Error::syntax("action S must be a name")))?;
    Ok(match name.as_bytes() {
        b"GoTo" => ActionKind::GoTo(destinations::resolve(
            context,
            dictionary
                .get(b"D")
                .ok_or_else(|| source.error(Error::syntax("GoTo requires D")))?,
            source,
        )?),
        b"GoToR" => ActionKind::GoToR {
            file: file(
                context,
                dictionary
                    .get(b"F")
                    .ok_or_else(|| source.error(Error::syntax("GoToR requires F")))?,
                source,
            )?,
            destination: destinations::remote(
                context,
                dictionary
                    .get(b"D")
                    .ok_or_else(|| source.error(Error::syntax("GoToR requires D")))?,
                source,
            )?,
            new_window: boolean(context, dictionary, b"NewWindow", source)?,
        },
        b"URI" => {
            let uri = context
                .required(dictionary, b"URI", source)?
                .as_string()
                .ok_or_else(|| source.error(Error::syntax("URI action requires a string")))?;
            let (catalog, catalog_source) = context.catalog()?;
            let base = if let Some(uri) = context.present(catalog, b"URI", catalog_source)? {
                let (uri, source) = context.dictionary(uri, catalog_source)?;
                context
                    .entry(uri, b"Base", source)?
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
                is_map: boolean(context, dictionary, b"IsMap", source)?.unwrap_or(false),
            }
        }
        b"Named" => {
            let n = context
                .required(dictionary, b"N", source)?
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
        b"Launch" => parse_launch(context, dictionary, source)?,
        b"JavaScript" => {
            let value = dictionary
                .get(b"JS")
                .ok_or_else(|| source.error(Error::syntax("JavaScript requires JS")))?;
            let (script, source) = context.resolve(value, source)?;
            let script = if let Some(bytes) = script.as_string() {
                Script::String(bytes)
            } else {
                let id = source.object.ok_or_else(|| {
                    source.error(Error::syntax("script must be a string or stream"))
                })?;
                let object = context.document.object(id)?;
                if object.stream_range().is_none() || identity(object.value()) != identity(script) {
                    return Err(source
                        .error(Error::syntax("script must be a string or stream"))
                        .into());
                }
                Script::Stream(object)
            };
            ActionKind::JavaScript(script)
        }
        _ => ActionKind::Other(name),
    })
}

pub(crate) fn read<'a>(document: &'a Document, value: &'a Value) -> ReadResult<ActionSequence<'a>> {
    read_with(&mut Context::new(document), value, Source::default())
}

/// One step of the iterative depth-first walk over an action's Next tree.
enum Frame<'a> {
    /// Visit an action below `parent`.
    Enter {
        value: &'a Value,
        source: Source,
        parent: Option<usize>,
        depth: usize,
    },
    /// Leave an action: later siblings may reuse it without forming a cycle.
    Exit(ValueIdentity),
}

pub(crate) fn read_with<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> ReadResult<ActionSequence<'a>> {
    let mut actions = Vec::new();
    let mut active = BTreeSet::new();
    let mut stack = vec![Frame::Enter {
        value,
        source,
        parent: None,
        depth: 0,
    }];
    while let Some(frame) = stack.pop() {
        let (value, source, parent, depth) = match frame {
            Frame::Exit(identity) => {
                active.remove(&identity);
                continue;
            }
            Frame::Enter {
                value,
                source,
                parent,
                depth,
            } => (value, source, parent, depth),
        };
        context.step(depth, source)?;
        let (value, source) = context.resolve(value, source)?;
        context.index_entry(source)?;
        if !active.insert(identity(value)) {
            context.repair(
                repairs::ACTION_CYCLE,
                "cyclic action Next edge omitted (12.6.2)",
                source,
            )?;
            continue;
        }
        let (dictionary, source) = context.dictionary(value, source)?;
        context.allocate::<Action<'_>>(1, source)?;
        let index = actions.len();
        actions.push(Action {
            parent,
            kind: kind(context, dictionary, source)?,
            dictionary,
        });
        context.allocate::<Frame<'_>>(1, source)?;
        stack.push(Frame::Exit(identity(value)));
        if let Some(next) = dictionary.get(b"Next") {
            let (next, next_source) = context.resolve(next, source)?;
            if next.is_null() {
                continue;
            }
            let children = if let Value::Array(children) = next {
                children.as_slice()
            } else {
                std::slice::from_ref(next)
            };
            context.allocate::<Frame<'_>>(children.len(), source)?;
            for child in children.iter().rev() {
                stack.push(Frame::Enter {
                    value: child,
                    source: next_source,
                    parent: Some(index),
                    depth: depth + 1,
                });
            }
        }
    }
    Ok(ActionSequence { actions })
}

fn parse_launch<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<ActionKind<'a>> {
    let file = context
        .present(dictionary, b"F", source)?
        .map(|v| file(context, v, source))
        .transpose()?;
    if file.is_none() {
        let mut platform = false;
        for key in [b"Win".as_slice(), b"Mac", b"Unix"] {
            platform |= context.present(dictionary, key, source)?.is_some();
        }
        if !platform {
            return Err(source
                .error(Error::syntax(
                    "Launch requires a file or platform parameters",
                ))
                .into());
        }
    }
    Ok(ActionKind::Launch {
        file,
        new_window: boolean(context, dictionary, b"NewWindow", source)?,
    })
}
