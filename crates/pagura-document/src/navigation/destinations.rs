//! Explicit and named destinations (12.3.2, Table 149; Annex J.3.3–J.3.4).
use super::actions;
use crate::Document;
use crate::source::ReadResult;
use crate::view::{Context, Source, identity, number};
use pagura_core::{Dictionary, Error, Name, Rect, Value};
use std::collections::BTreeSet;

/// How the host should fit or position a destination page. Null coordinates
/// remain None; XYZ zoom zero is also None (Table 149, which defines exactly
/// these eight views).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DestinationView {
    /// Retain any unspecified coordinate or magnification.
    Xyz {
        /// Left coordinate.
        left: Option<f64>,
        /// Top coordinate.
        top: Option<f64>,
        /// Magnification; None retains the current zoom.
        zoom: Option<f64>,
    },
    /// Fit the whole page.
    Fit,
    /// Fit the page width, optionally positioning its top.
    FitH(Option<f64>),
    /// Fit the page height, optionally positioning its left edge.
    FitV(Option<f64>),
    /// Fit this rectangle in default user space.
    FitR(Rect),
    /// Fit the page content bounding box.
    FitB,
    /// Fit content width, optionally positioning the top.
    FitBh(Option<f64>),
    /// Fit content height, optionally positioning the left edge.
    FitBv(Option<f64>),
}

/// An explicit destination in this document.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Destination {
    /// Zero-based page index in this document.
    pub page: usize,
    /// View instruction, interpreted by the host.
    pub view: DestinationView,
}

impl Destination {
    /// A destination showing zero-based `page` with `view`.
    pub const fn new(page: usize, view: DestinationView) -> Self {
        Self { page, view }
    }
}

/// A named destination key, preserving its original PDF kind: a name or a
/// byte string, the only two 12.3.2.3 allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationName<'a> {
    /// Legacy catalog Dests key, or a name reference.
    Name(&'a Name),
    /// Name-tree key, or a byte-string reference.
    String(&'a [u8]),
}

impl<'a> DestinationName<'a> {
    /// Canonical decoded bytes used for key comparison.
    pub fn as_bytes(self) -> &'a [u8] {
        match self {
            Self::Name(n) => n.as_bytes(),
            Self::String(s) => s,
        }
    }
}

/// A named destination and its resolved local target.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct NamedDestination<'a> {
    /// Original key kind and bytes.
    pub name: DestinationName<'a>,
    /// Fully resolved page and view.
    pub destination: Destination,
}

/// A remote destination is data; its page is never resolved locally. It is
/// a page number or a name, as 12.6.4.3 requires.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RemoteDestination<'a> {
    /// Zero-based remote page number and view.
    Explicit {
        /// Remote page index.
        page: usize,
        /// View instruction.
        view: DestinationView,
    },
    /// Name to be resolved in the remote document by the host.
    Named(DestinationName<'a>),
}

/// An inert outline or annotation target. Activating it is the host's decision.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Target<'a> {
    /// A local page/view jump.
    Destination(Destination),
    /// An inert action sequence.
    Actions(actions::ActionSequence<'a>),
}

fn key(value: &Value) -> Option<DestinationName<'_>> {
    match value {
        Value::Name(n) => Some(DestinationName::Name(n)),
        Value::String(s) => Some(DestinationName::String(s)),
        _ => None,
    }
}

fn optional(value: &Value, source: Source) -> ReadResult<Option<f64>> {
    if value.is_null() {
        Ok(None)
    } else {
        Ok(number(value, source).map(Some)?)
    }
}

fn view<'a>(
    context: &mut Context<'a>,
    values: &'a [Value],
    source: Source,
) -> ReadResult<DestinationView> {
    if values.is_empty() || values.len() > 5 {
        return Err(source
            .error(Error::syntax("incorrect destination operand count"))
            .into());
    }
    context.allocate::<&Value>(values.len(), source)?;
    let values = values
        .iter()
        .map(|v| context.resolve(v, source).map(|(v, _)| v))
        .collect::<ReadResult<Vec<_>>>()?;
    let Some((name, operands)) = values
        .split_first()
        .and_then(|(name, operands)| Some((name.as_name()?, operands)))
    else {
        return Err(source
            .error(Error::syntax("destination requires a view name"))
            .into());
    };
    Ok(match (name.as_bytes(), operands) {
        (b"XYZ", [left, top, zoom]) => {
            let zoom = optional(zoom, source)?.filter(|v| *v != 0.0);
            if zoom.is_some_and(|v| v < 0.0) {
                return Err(source
                    .error(Error::syntax("destination zoom must be nonnegative"))
                    .into());
            }
            DestinationView::Xyz {
                left: optional(left, source)?,
                top: optional(top, source)?,
                zoom,
            }
        }
        (b"Fit", []) => DestinationView::Fit,
        (b"FitH", [top]) => DestinationView::FitH(optional(top, source)?),
        (b"FitV", [left]) => DestinationView::FitV(optional(left, source)?),
        (b"FitR", [left, bottom, right, top]) => DestinationView::FitR(
            Rect::from_corners(
                number(left, source)?,
                number(bottom, source)?,
                number(right, source)?,
                number(top, source)?,
            )
            .ok_or_else(|| source.error(Error::syntax("invalid destination rectangle")))?,
        ),
        (b"FitB", []) => DestinationView::FitB,
        (b"FitBH", [top]) => DestinationView::FitBh(optional(top, source)?),
        (b"FitBV", [left]) => DestinationView::FitBv(optional(left, source)?),
        _ => {
            return Err(source
                .error(Error::syntax(
                    "unknown destination view or incorrect operand count",
                ))
                .into());
        }
    })
}

pub(super) fn local(document: &Document, value: &Value) -> ReadResult<Destination> {
    resolve(&mut Context::new(document), value, Source::default())
}

pub(super) fn resolve<'a>(
    context: &mut Context<'a>,
    mut value: &'a Value,
    mut source: Source,
) -> ReadResult<Destination> {
    let mut visited = BTreeSet::new();
    for depth in 0..=context.document.limits().depth {
        context.step(depth, source)?;
        (value, source) = context.resolve(value, source)?;
        context.index_entry(source)?;
        if !visited.insert(identity(value)) {
            return Err(source
                .error(Error::cycle("cyclic named destination"))
                .into());
        }
        match value {
            Value::Dictionary(dictionary) => {
                value = dictionary.get(b"D").ok_or_else(|| {
                    source.error(Error::syntax("destination dictionary requires D"))
                })?;
            }
            Value::Array(values) => {
                let page_count = context.document.page_count()?;
                let (first, operands) = values
                    .split_first()
                    .ok_or_else(|| source.error(Error::syntax("destination requires a page")))?;
                let page = if let Some(page) = first.as_reference() {
                    context.page_index(page, source)?
                } else if let Some(page) = first
                    .as_integer()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| *n < page_count)
                {
                    context.repair(
                        crate::repairs::DESTINATION_PAGE_NUMBER,
                        "local destination page number interpreted as zero-based index \
                         (12.3.2.2, Table 149)",
                        source,
                    )?;
                    page
                } else {
                    return Err(source
                        .error(Error::syntax("local destination requires a page reference"))
                        .into());
                };
                return Ok(Destination {
                    page,
                    view: view(context, operands, source)?,
                });
            }
            _ => {
                let name = key(value)
                    .ok_or_else(|| source.error(Error::syntax("invalid destination object")))?;
                (value, source) = find(context, name)?.ok_or_else(|| {
                    source.error(Error::missing_object("named destination is absent"))
                })?;
            }
        }
    }
    Err(source
        .error(Error::limit("destination resolution depth exceeded"))
        .into())
}

fn find<'a>(
    context: &mut Context<'a>,
    name: DestinationName<'a>,
) -> ReadResult<Option<(&'a Value, Source)>> {
    let (catalog, catalog_source) = context.catalog()?;
    // A name uses the legacy dictionary first; a string uses the name tree
    // first. The other representation is a compatibility fallback by bytes.
    let legacy = |context: &mut Context<'a>| -> ReadResult<Option<(&'a Value, Source)>> {
        let Some(value) = context.present(catalog, b"Dests", catalog_source)? else {
            return Ok(None);
        };
        let (dictionary, source) = context.dictionary(value, catalog_source)?;
        Ok(context
            .present(dictionary, name.as_bytes(), source)?
            .map(|v| (v, source)))
    };
    let tree = |context: &mut Context<'a>| -> ReadResult<Option<(&'a Value, Source)>> {
        let Some(names) = context.present(catalog, b"Names", catalog_source)? else {
            return Ok(None);
        };
        let (names, source) = context.dictionary(names, catalog_source)?;
        context
            .present(names, b"Dests", source)?
            .map(|root| {
                context
                    .document
                    .name_tree(root)
                    .with_source(source)
                    .get_with(context, name.as_bytes())
            })
            .transpose()
            .map(Option::flatten)
    };
    match name {
        DestinationName::Name(_) => match legacy(context)? {
            Some(v) => Ok(Some(v)),
            None => Ok(tree(context)?),
        },
        DestinationName::String(_) => match tree(context)? {
            Some(v) => Ok(Some(v)),
            None => Ok(legacy(context)?),
        },
    }
}

pub(super) fn remote<'a>(
    context: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> ReadResult<RemoteDestination<'a>> {
    let (value, source) = context.resolve(value, source)?;
    if let Some(name) = key(value) {
        return Ok(RemoteDestination::Named(name));
    }
    let values = value.as_array().ok_or_else(|| {
        source.error(Error::syntax("remote destination must be an array or name"))
    })?;
    let Some((page, operands)) = values
        .split_first()
        .and_then(|(page, operands)| Some((usize::try_from(page.as_integer()?).ok()?, operands)))
    else {
        return Err(source
            .error(Error::syntax("remote destination page must be nonnegative"))
            .into());
    };
    Ok(RemoteDestination::Explicit {
        page,
        view: view(context, operands, source)?,
    })
}

pub(super) fn target<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> ReadResult<Option<Target<'a>>> {
    let action = context.present(dictionary, b"A", source)?;
    let dest = context.present(dictionary, b"Dest", source)?;
    if action.is_some() && dest.is_some() {
        context.repair(
            crate::repairs::TARGET_CONFLICT,
            "A and Dest coexist; action takes precedence (12.3.3, 12.5.6.5)",
            source,
        )?;
    }
    if let Some(action) = action {
        return actions::read_with(context, action, source).map(|v| Some(Target::Actions(v)));
    }
    dest.map(|dest| resolve(context, dest, source).map(Target::Destination))
        .transpose()
}

pub(super) fn named(document: &Document) -> ReadResult<Vec<NamedDestination<'_>>> {
    let mut context = Context::new(document);
    let (catalog, source) = context.catalog()?;
    let mut entries = Vec::new();
    if let Some(value) = context.present(catalog, b"Dests", source)? {
        let (dictionary, source) = context.dictionary(value, source)?;
        for (key, _) in dictionary {
            let Some(value) = context.present(dictionary, key.as_bytes(), source)? else {
                continue;
            };
            context.step(0, source)?;
            context.allocate::<NamedDestination<'_>>(1, source)?;
            entries.push(NamedDestination {
                name: DestinationName::Name(key),
                destination: resolve(&mut context, value, source)?,
            });
        }
    }
    if let Some(names) = context.present(catalog, b"Names", source)? {
        let (names, source) = context.dictionary(names, source)?;
        if let Some(root) = context.present(names, b"Dests", source)? {
            for entry in document
                .name_tree(root)
                .with_source(source)
                .entries_with(&mut context)?
            {
                context.allocate::<NamedDestination<'_>>(1, source)?;
                entries.push(NamedDestination {
                    name: DestinationName::String(entry.key),
                    destination: resolve(&mut context, entry.value, entry.source)?,
                });
            }
        }
    }
    Ok(entries)
}
