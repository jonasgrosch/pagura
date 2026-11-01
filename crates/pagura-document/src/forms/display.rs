//! Requested-widget field data, sharing the full form view's attribute readers.

use super::{
    ButtonKind, FieldFlags, FieldKind, FieldValue, WidgetField,
    attributes::{self, Inherited, Key},
    values, widgets,
};
use crate::{
    Document,
    view::{Context, Source},
};
use pagura_core::{Dictionary, Error, Name, Result};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
struct Node<'a> {
    dictionary: &'a Dictionary,
    source: Source,
}

pub(super) fn membership(document: &Document, annotation: &Dictionary) -> Result<bool> {
    let mut context = Context::new(document);
    let (catalog, catalog_source) = context.catalog()?;
    let Some(form) = context.present(catalog, b"AcroForm", catalog_source)? else {
        return Ok(false);
    };
    let (form, source) = context.dictionary(form, catalog_source)?;
    ancestry(
        &mut context,
        Node {
            dictionary: annotation,
            source: Source::default(),
        },
        Node {
            dictionary: form,
            source,
        },
    )?;
    Ok(true)
}

pub(super) fn read<'a>(
    document: &'a Document,
    annotation: &'a Dictionary,
) -> Result<Option<WidgetField<'a>>> {
    let mut context = Context::new(document);
    let (catalog, catalog_source) = context.catalog()?;
    let Some(form) = context.present(catalog, b"AcroForm", catalog_source)? else {
        return Ok(None);
    };
    let (form, form_source) = context.dictionary(form, catalog_source)?;
    let (dictionary, source) = (annotation, Source::default());
    let (ancestors, membership) = ancestry(
        &mut context,
        Node { dictionary, source },
        Node {
            dictionary: form,
            source: form_source,
        },
    )?;
    // An unnamed child is a separate representation of its parent field;
    // its own field-like entries do not override the parent (12.7.4.2).
    let separate = ancestors.len() > 1 && context.present(dictionary, b"T", source)?.is_none();
    let field = ancestors
        .get(usize::from(separate))
        .copied()
        .ok_or_else(|| source.error(Error::syntax("widget has no owning field")))?;
    let mut effective = Inherited::default();
    for node in ancestors.iter().skip(usize::from(separate)).rev() {
        effective = effective.overlay(
            &mut context,
            node.dictionary,
            node.source,
            [Key::Ft, Key::Ff],
        )?;
    }
    let kind = values::field_type(&mut context, &effective)?;
    if !kind.is_some_and(|kind| matches!(kind.as_bytes(), b"Tx" | b"Ch" | b"Btn")) {
        return Ok(None);
    }
    let flags = effective.flags(&mut context)?;
    let push = kind.is_some_and(|kind| kind.as_bytes() == b"Btn")
        && flags.contains(FieldFlags::PUSHBUTTON);
    // Push buttons do not use V or DV (12.7.5.2.2). Reuse the already bounded
    // ancestor list to read applicable data, never a second field-tree walk.
    effective = effective.overlay(&mut context, form, form_source, [Key::Da, Key::Q])?;
    for node in ancestors.iter().skip(usize::from(separate)).rev() {
        effective = effective.overlay(
            &mut context,
            node.dictionary,
            node.source,
            [Key::V, Key::Da, Key::Q, Key::Opt, Key::MaxLen]
                .into_iter()
                .filter(|key| !push || !matches!(key, Key::V | Key::Opt)),
        )?;
    }
    if attributes::has_field_children(&mut context, field.dictionary, field.source)? {
        return Err(field
            .source
            .error(Error::syntax("widget belongs to a nonterminal field")));
    }
    let kind = values::kind(
        &mut context,
        field.dictionary,
        &effective,
        flags,
        field.source,
    )?;
    let value = values::read(&mut context, &kind, effective.get(Key::V))?.map(|(value, _)| value);
    let button_state = state(
        &mut context,
        &kind,
        value.as_ref(),
        Node {
            dictionary: annotation,
            source,
        },
        if separate { membership } else { (0, 1) },
    )?;
    context.allocate::<WidgetField<'_>>(1, field.source)?;
    Ok(Some(WidgetField {
        kind,
        flags,
        value,
        button_state,
        default_appearance: attributes::appearance(&mut context, effective.get(Key::Da))?,
        justification: attributes::quadding(&mut context, effective.get(Key::Q))?,
        dictionary: field.dictionary,
        default_resources: context
            .present(form, b"DR", form_source)?
            .map(|value| {
                context
                    .dictionary(value, form_source)
                    .map(|(dictionary, _)| dictionary)
            })
            .transpose()?,
        has_xfa: context.present(form, b"XFA", form_source)?.is_some(),
    }))
}

/// Resolve normal toggle selection without visiting another field subtree.
fn state<'a>(
    context: &mut Context<'a>,
    kind: &FieldKind<'a>,
    value: Option<&FieldValue<'a>>,
    widget: Node<'a>,
    membership: (usize, usize),
) -> Result<Option<&'a Name>> {
    let FieldKind::Button {
        kind: ButtonKind::CheckBox | ButtonKind::Radio,
        export_values,
    } = kind
    else {
        return Ok(None);
    };
    let selected = widgets::state(context, widget.dictionary, widget.source)?;
    let on = if selected.is_some() || export_values.is_empty() {
        None
    } else {
        if export_values.len() != membership.1 {
            return Err(widget
                .source
                .error(Error::syntax("button Opt must have one export per widget")));
        }
        // Charge the decimal String and its Name copy before allocating either.
        context.allocate::<u8>(6 * size_of::<usize>(), widget.source)?;
        Some(Name::new(membership.0.to_string().as_bytes()))
    };
    Ok(values::button_state(value, selected, on.as_ref()))
}

/// Walk only Parent edges, proving each forward membership along the way.
fn ancestry<'a>(
    context: &mut Context<'a>,
    mut current: Node<'a>,
    form: Node<'a>,
) -> Result<(Vec<Node<'a>>, (usize, usize))> {
    let mut membership = (0, 1);
    let mut ancestors = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        context.step(ancestors.len(), current.source)?;
        context.index_entry(current.source)?;
        if !seen.insert(std::ptr::from_ref(current.dictionary).addr()) {
            return Err(current
                .source
                .error(Error::cycle("cyclic widget field ancestry")));
        }
        context.reserve(&mut ancestors, 1, current.source)?;
        ancestors.push(current);
        let Some(parent) = context.present(current.dictionary, b"Parent", current.source)? else {
            break;
        };
        let (dictionary, source) = context.dictionary(parent, current.source)?;
        let (ordinal, siblings) = member(
            context,
            Node { dictionary, source },
            b"Kids",
            current.dictionary,
        )?;
        if ancestors.len() == 1 {
            membership = (ordinal, siblings);
        }
        current = Node { dictionary, source };
    }
    member(context, form, b"Fields", current.dictionary)?;
    Ok((ancestors, membership))
}

/// Check the forward edge without descending into any sibling dictionary.
fn member<'a>(
    context: &mut Context<'a>,
    owner: Node<'a>,
    key: &[u8],
    child: &Dictionary,
) -> Result<(usize, usize)> {
    let entries = context.required(owner.dictionary, key, owner.source)?;
    let entries = attributes::array(context, entries, owner.source)?;
    let mut found = None;
    for (index, entry) in entries.iter().enumerate() {
        context.step(0, owner.source)?;
        let (entry, _) = context.resolve(entry, owner.source)?;
        if entry
            .as_dictionary()
            .is_some_and(|dictionary| std::ptr::eq(dictionary, child))
        {
            if found.is_some() {
                return Err(owner
                    .source
                    .error(Error::cycle("repeated widget field membership")));
            }
            found = Some(index);
        }
    }
    if let Some(index) = found {
        Ok((index, entries.len()))
    } else {
        Err(owner.source.error(Error::syntax(
            "widget Parent disagrees with Fields or Kids membership",
        )))
    }
}
