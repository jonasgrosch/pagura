//! Requested-widget field data, sharing the full form view's attribute readers.

use super::{
    WidgetText,
    attributes::{self, Inherited, Key},
    values,
};
use crate::{
    Document,
    view::{Context, Source},
};
use pagura_core::{Dictionary, Error, Result};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
struct Node<'a> {
    dictionary: &'a Dictionary,
    source: Source,
}

pub(super) fn read<'a>(
    document: &'a Document,
    annotation: &'a Dictionary,
) -> Result<Option<WidgetText<'a>>> {
    let mut context = Context::new(document);
    let (catalog, catalog_source) = context.catalog()?;
    let Some(form) = context.present(catalog, b"AcroForm", catalog_source)? else {
        return Ok(None);
    };
    let (form, form_source) = context.dictionary(form, catalog_source)?;
    let (dictionary, source) = (annotation, Source::default());
    let mut current = Node { dictionary, source };
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
        member(
            &mut context,
            Node { dictionary, source },
            b"Kids",
            current.dictionary,
        )?;
        current = Node { dictionary, source };
    }
    member(
        &mut context,
        Node {
            dictionary: form,
            source: form_source,
        },
        b"Fields",
        current.dictionary,
    )?;
    // An unnamed child is a separate representation of its parent field;
    // its own field-like entries do not override the parent (12.7.4.2).
    let separate = ancestors.len() > 1 && context.present(dictionary, b"T", source)?.is_none();
    let field = ancestors
        .get(usize::from(separate))
        .copied()
        .ok_or_else(|| source.error(Error::syntax("widget has no owning field")))?;
    let mut effective = Inherited::defaults(&mut context, form, form_source)?;
    for node in ancestors.iter().skip(usize::from(separate)).rev() {
        effective = effective.overlay(
            &mut context,
            node.dictionary,
            node.source,
            Key::ALL.into_iter().filter(|key| *key != Key::Dv),
        )?;
    }
    let kind = values::field_type(&mut context, &effective)?;
    if !kind.is_some_and(|kind| matches!(kind.as_bytes(), b"Tx" | b"Ch")) {
        return Ok(None);
    }
    if attributes::has_field_children(&mut context, field.dictionary, field.source)? {
        return Err(field
            .source
            .error(Error::syntax("widget belongs to a nonterminal field")));
    }
    let flags = effective.flags(&mut context)?;
    let kind = values::kind(
        &mut context,
        field.dictionary,
        &effective,
        flags,
        field.source,
    )?;
    let value = values::read(&mut context, &kind, effective.get(Key::V))?.map(|(value, _)| value);
    context.allocate::<WidgetText<'_>>(1, field.source)?;
    Ok(Some(WidgetText {
        kind,
        flags,
        value,
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

/// Check the forward edge without descending into any sibling dictionary.
fn member<'a>(
    context: &mut Context<'a>,
    owner: Node<'a>,
    key: &[u8],
    child: &Dictionary,
) -> Result<()> {
    let entries = context.required(owner.dictionary, key, owner.source)?;
    let entries = attributes::array(context, entries, owner.source)?;
    let mut found = false;
    for entry in entries {
        context.step(0, owner.source)?;
        let (entry, _) = context.resolve(entry, owner.source)?;
        if entry
            .as_dictionary()
            .is_some_and(|dictionary| std::ptr::eq(dictionary, child))
        {
            if found {
                return Err(owner
                    .source
                    .error(Error::cycle("repeated widget field membership")));
            }
            found = true;
        }
    }
    if found {
        Ok(())
    } else {
        Err(owner.source.error(Error::syntax(
            "widget Parent disagrees with Fields or Kids membership",
        )))
    }
}
