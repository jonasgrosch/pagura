//! Explicit and named destinations (12.3.2, Table 149; Annex J.3.3–J.3.4).
use super::{
    actions,
    context::{Context, Source, identity, number},
};
use crate::Document;
use pagura_core::{Dictionary, Error, ErrorKind, Name, Rect, Result, Value};
use std::collections::BTreeSet;

/// How the host should fit or position a destination page. Null coordinates
/// remain None; XYZ zoom zero is also None (Table 149).
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
pub struct Destination {
    /// Zero-based page index in this document.
    pub page: usize,
    /// View instruction, interpreted by the host.
    pub view: DestinationView,
}
/// A named destination key, preserving its original PDF kind.
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
pub struct NamedDestination<'a> {
    /// Original key kind and bytes.
    pub name: DestinationName<'a>,
    /// Fully resolved page and view.
    pub destination: Destination,
}
/// A remote destination is data; its page is never resolved locally.
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
fn optional(value: &Value, source: Source) -> Result<Option<f64>> {
    if value.is_null() {
        Ok(None)
    } else {
        number(value, source).map(Some)
    }
}
fn view<'a>(ctx: &mut Context<'a>, values: &'a [Value], source: Source) -> Result<DestinationView> {
    if values.is_empty() || values.len() > 5 {
        return Err(source.error(Error::syntax("incorrect destination operand count")));
    }
    ctx.allocate::<&Value>(values.len(), source)?;
    let values = values
        .iter()
        .map(|v| ctx.resolve(v, source).map(|(v, _)| v))
        .collect::<Result<Vec<_>>>()?;
    let name = values
        .first()
        .and_then(|v| v.as_name())
        .ok_or_else(|| source.error(Error::syntax("destination requires a view name")))?;
    Ok(match (name.as_bytes(), &values[1..]) {
        (b"XYZ", [left, top, zoom]) => {
            let zoom = optional(zoom, source)?.filter(|v| *v != 0.0);
            if zoom.is_some_and(|v| v < 0.0) {
                return Err(source.error(Error::syntax("destination zoom must be nonnegative")));
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
            return Err(source.error(Error::syntax(
                "unknown destination view or incorrect operand count",
            )));
        }
    })
}
pub(super) fn local(doc: &Document, value: &Value) -> Result<Destination> {
    resolve(&mut Context::new(doc), value, Source::default())
}
pub(super) fn resolve<'a>(
    ctx: &mut Context<'a>,
    mut value: &'a Value,
    mut source: Source,
) -> Result<Destination> {
    let mut visited = BTreeSet::new();
    for depth in 0..=ctx.doc.limits().depth {
        ctx.step(depth, source)?;
        (value, source) = ctx.resolve(value, source)?;
        ctx.allocate::<usize>(8, source)?;
        if !visited.insert(identity(value)) {
            return Err(source.error(Error::cycle("cyclic named destination")));
        }
        match value {
            Value::Dictionary(d) => {
                value = d.get(b"D").ok_or_else(|| {
                    source.error(Error::syntax("destination dictionary requires D"))
                })?;
            }
            Value::Array(values) => {
                let page_count = ctx.doc.page_count()?;
                let first = values
                    .first()
                    .ok_or_else(|| source.error(Error::syntax("destination requires a page")))?;
                let page = if let Some(page) = first.as_reference() {
                    ctx.page_index(page, source)?
                } else if let Some(page) = first
                    .as_integer()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| *n < page_count)
                {
                    ctx.repair(crate::tags::DESTINATION_PAGE_NUMBER,"local destination page number interpreted as zero-based index (12.3.2.2, Table 149)",source)?;
                    page
                } else {
                    return Err(
                        source.error(Error::syntax("local destination requires a page reference"))
                    );
                };
                return Ok(Destination {
                    page,
                    view: view(ctx, &values[1..], source)?,
                });
            }
            _ => {
                let name = key(value)
                    .ok_or_else(|| source.error(Error::syntax("invalid destination object")))?;
                (value, source) = find(ctx, name)?.ok_or_else(|| {
                    source.error(Error::new(
                        ErrorKind::MissingObject,
                        "named destination is absent",
                    ))
                })?;
            }
        }
    }
    Err(source.error(Error::limit("destination resolution depth exceeded")))
}
fn find<'a>(
    ctx: &mut Context<'a>,
    name: DestinationName<'a>,
) -> Result<Option<(&'a Value, Source)>> {
    let (catalog, catalog_source) = ctx.catalog()?;
    // A name uses the legacy dictionary first; a string uses the name tree
    // first. The other representation is a compatibility fallback by bytes.
    let legacy = |ctx: &mut Context<'a>| -> Result<Option<(&'a Value, Source)>> {
        let Some(value) = ctx.present(catalog, b"Dests", catalog_source)? else {
            return Ok(None);
        };
        let (dictionary, source) = ctx.dictionary(value, catalog_source)?;
        Ok(ctx
            .present(dictionary, name.as_bytes(), source)?
            .map(|v| (v, source)))
    };
    let tree = |ctx: &mut Context<'a>| -> Result<Option<(&'a Value, Source)>> {
        let Some(names) = ctx.present(catalog, b"Names", catalog_source)? else {
            return Ok(None);
        };
        let (names, source) = ctx.dictionary(names, catalog_source)?;
        ctx.present(names, b"Dests", source)?
            .map(|root| {
                ctx.doc
                    .name_tree(root)
                    .with_source(source)
                    .get_with(ctx, name.as_bytes())
            })
            .transpose()
            .map(Option::flatten)
    };
    match name {
        DestinationName::Name(_) => match legacy(ctx)? {
            Some(v) => Ok(Some(v)),
            None => tree(ctx),
        },
        DestinationName::String(_) => match tree(ctx)? {
            Some(v) => Ok(Some(v)),
            None => legacy(ctx),
        },
    }
}
pub(super) fn remote<'a>(
    ctx: &mut Context<'a>,
    value: &'a Value,
    source: Source,
) -> Result<RemoteDestination<'a>> {
    let (value, source) = ctx.resolve(value, source)?;
    if let Some(name) = key(value) {
        return Ok(RemoteDestination::Named(name));
    }
    let values = value.as_array().ok_or_else(|| {
        source.error(Error::syntax("remote destination must be an array or name"))
    })?;
    let page = values
        .first()
        .and_then(Value::as_integer)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| {
            source.error(Error::syntax("remote destination page must be nonnegative"))
        })?;
    Ok(RemoteDestination::Explicit {
        page,
        view: view(ctx, &values[1..], source)?,
    })
}
pub(super) fn target<'a>(
    ctx: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
) -> Result<Option<Target<'a>>> {
    let action = ctx.present(dictionary, b"A", source)?;
    let dest = ctx.present(dictionary, b"Dest", source)?;
    if action.is_some() && dest.is_some() {
        ctx.repair(
            crate::tags::TARGET_CONFLICT,
            "A and Dest coexist; action takes precedence (12.3.3, 12.5.6.5)",
            source,
        )?;
    }
    if let Some(action) = action {
        return actions::read_with(ctx, action, source).map(|v| Some(Target::Actions(v)));
    }
    dest.map(|dest| resolve(ctx, dest, source).map(Target::Destination))
        .transpose()
}
pub(super) fn named(doc: &Document) -> Result<Vec<NamedDestination<'_>>> {
    let mut ctx = Context::new(doc);
    let (catalog, source) = ctx.catalog()?;
    let mut entries = Vec::new();
    if let Some(value) = ctx.present(catalog, b"Dests", source)? {
        let (dictionary, source) = ctx.dictionary(value, source)?;
        for (key, _) in dictionary {
            let Some(value) = ctx.present(dictionary, key.as_bytes(), source)? else {
                continue;
            };
            ctx.step(0, source)?;
            ctx.allocate::<NamedDestination<'_>>(1, source)?;
            entries.push(NamedDestination {
                name: DestinationName::Name(key),
                destination: resolve(&mut ctx, value, source)?,
            });
        }
    }
    if let Some(names) = ctx.present(catalog, b"Names", source)? {
        let (names, source) = ctx.dictionary(names, source)?;
        if let Some(root) = ctx.present(names, b"Dests", source)? {
            for entry in doc
                .name_tree(root)
                .with_source(source)
                .entries_with(&mut ctx)?
            {
                ctx.allocate::<NamedDestination<'_>>(1, source)?;
                entries.push(NamedDestination {
                    name: DestinationName::String(entry.key),
                    destination: resolve(&mut ctx, entry.value, entry.source)?,
                });
            }
        }
    }
    Ok(entries)
}
