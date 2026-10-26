//! Reachability-derived roles. A shared object retains every use and field owner.
use super::ObjectRole;
use crate::source::{ReadFault, ReadResult};
use crate::{
    Document, Location,
    forms::{FieldKind, FieldValue},
    view::{Context, Source},
};
use pagura_core::{Dictionary, Error, ErrorKind, ObjectId, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Usage {
    pub role: ObjectRole,
    pub field: Option<ObjectId>,
}
#[derive(Default)]
pub(super) struct Roles {
    pub uses: BTreeMap<ObjectId, BTreeSet<Usage>>,
    pub fields: BTreeMap<ObjectId, String>,
    pub signatures: BTreeSet<ObjectId>,
    pub signed_fields: BTreeSet<ObjectId>,
    pub field_values: BTreeMap<ObjectId, Option<Value>>,
    pub widgets: BTreeMap<ObjectId, ObjectId>,
    pub widget_fields: BTreeSet<ObjectId>,
    pub pages: BTreeSet<ObjectId>,
    pub page_annotations: BTreeMap<ObjectId, Vec<ObjectId>>,
    pub annotation_members: BTreeMap<ObjectId, BTreeSet<ObjectId>>,
    pub value_sources: BTreeMap<ObjectId, BTreeSet<ObjectId>>,
    pub signature_ends: BTreeMap<ObjectId, usize>,
    pub source_end: usize,
    pub timestamp_signatures: BTreeSet<ObjectId>,
    pub annotation_sources: BTreeMap<ObjectId, BTreeSet<ObjectId>>,
    pub length_dependents: BTreeMap<ObjectId, BTreeSet<ObjectId>>,
    pub stream_members: BTreeMap<ObjectId, BTreeSet<ObjectId>>,
    pub templates_present: bool,
    pub ambiguous: bool,
}
impl Roles {
    pub(super) fn get(&self, id: ObjectId) -> impl Iterator<Item = Usage> + '_ {
        self.uses.get(&id).into_iter().flatten().copied()
    }
}

pub(super) fn read(
    doc: &Document,
    ctx: &mut Context<'_>,
    facts: &super::history::Facts,
) -> ReadResult<Roles> {
    let mut roles = Roles {
        source_end: doc.source_len(),
        ..Roles::default()
    };
    read_form(doc, ctx, &mut roles)?;
    read_pages(doc, ctx, &mut roles)?;
    ctx.allocate::<usize>(
        (facts.signature_ends.len() + facts.timestamps.len()).saturating_mul(12),
        Source::default(),
    )?;
    roles.signature_ends.clone_from(&facts.signature_ends);
    roles.timestamp_signatures.clone_from(&facts.timestamps);
    if let Some(names) = doc.catalog()?.get(b"Names") {
        let names = doc.resolve(names)?;
        if !names.is_null() {
            roles.templates_present = names
                .as_dictionary()
                .ok_or_else(|| Error::syntax("catalog Names must be a dictionary"))?
                .contains_key(b"Templates");
        }
    }
    let root = doc
        .trailer()
        .get(b"Root")
        .ok_or_else(|| Error::syntax("revision has no catalog"))?;
    walk(
        doc,
        ctx,
        &mut roles,
        root,
        Usage {
            role: ObjectRole::Catalog,
            field: None,
        },
        0,
    )?;
    trailer(doc, ctx, &mut roles)?;
    // A field/widget can exist outside the page's Annots. Its ownership still
    // comes from the AcroForm, and aliases from elsewhere remain additional uses.
    ctx.allocate::<ObjectId>(roles.fields.len(), Source::default())?;
    let fields: Vec<_> = roles.fields.keys().copied().collect();
    for id in fields {
        walk(
            doc,
            ctx,
            &mut roles,
            &Value::Reference(id),
            Usage {
                role: ObjectRole::Field,
                field: Some(id),
            },
            0,
        )?;
    }
    read_storage(doc, ctx, &mut roles)?;
    Ok(roles)
}

fn read_pages(doc: &Document, ctx: &mut Context<'_>, roles: &mut Roles) -> ReadResult<()> {
    for page in doc.pages()? {
        ctx.step(0, Source::default())?;
        ctx.allocate::<usize>(8, Source::default())?;
        roles.pages.insert(page.id());
        roles.ambiguous |= !page.source_path().is_empty();
        let annotations = if let Some(value) = page.dictionary().get(b"Annots") {
            if let Some(id) = value.as_reference() {
                ctx.allocate::<usize>(16, Source::default())?;
                roles
                    .annotation_sources
                    .entry(id)
                    .or_default()
                    .insert(page.id());
            }
            let value = doc.resolve(value)?;
            if value.is_null() {
                Vec::new()
            } else {
                let array = value
                    .as_array()
                    .ok_or_else(|| Error::syntax("page Annots must be an array"))?;
                ctx.allocate::<ObjectId>(array.len(), Source::default())?;
                array
                    .iter()
                    .map(|v| {
                        v.as_reference()
                            .ok_or_else(|| Error::syntax("annotation must be indirect"))
                    })
                    .collect::<pagura_core::Result<Vec<_>>>()?
            }
        } else {
            Vec::new()
        };
        ctx.allocate::<usize>(annotations.len().saturating_mul(8), Source::default())?;
        roles
            .annotation_members
            .insert(page.id(), annotations.iter().copied().collect());
        roles.page_annotations.insert(page.id(), annotations);
    }
    Ok(())
}
fn read_storage(doc: &Document, ctx: &mut Context<'_>, roles: &mut Roles) -> ReadResult<()> {
    ctx.allocate::<usize>(doc.revisions().len().saturating_mul(16), Source::default())?;
    let xrefs: BTreeSet<_> = doc
        .revisions()
        .iter()
        .flat_map(|r| {
            std::iter::once(r.offset())
                .chain(std::iter::once(r.end_section))
                .chain(
                    r.trailer()
                        .get(b"XRefStm")
                        .and_then(Value::as_integer)
                        .and_then(|n| usize::try_from(n).ok()),
                )
        })
        .collect();
    for (&id, slot) in &doc.slots {
        ctx.step(0, Source::default())?;
        let bookkeeping = matches!(slot.location, Location::Offset(at) if xrefs.contains(&at))
            || doc.object_streams.contains_key(&id.number);
        if bookkeeping {
            ctx.allocate::<usize>(12, Source::default())?;
            roles.uses.entry(id).or_default().insert(Usage {
                role: ObjectRole::CrossReference,
                field: None,
            });
        }
        if let Location::Compressed { stream, .. } = slot.location {
            ctx.allocate::<usize>(16, Source::default())?;
            roles
                .stream_members
                .entry(ObjectId::new(stream, 0))
                .or_default()
                .insert(id);
        }
    }
    Ok(())
}

// A reachable stream can change extent through an indirect Length without a
// new xref entry for the stream. Do not parse unrelated xref slots to find it.
fn read_length(
    doc: &Document,
    ctx: &mut Context<'_>,
    roles: &mut Roles,
    stream: ObjectId,
    value: &Value,
    depth: usize,
) -> ReadResult<()> {
    let mut length = value.as_dictionary().and_then(|d| d.get(b"Length"));
    let mut depth = depth;
    while let Some(source) = length.and_then(Value::as_reference) {
        ctx.step(depth, Source::default())?;
        ctx.allocate::<usize>(16, Source::default())?;
        if !roles
            .length_dependents
            .entry(source)
            .or_default()
            .insert(stream)
        {
            break;
        }
        length = match doc.object(source) {
            Ok(object) => Some(object.value()),
            Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::MissingObject => None,
            Err(error) => return Err(error),
        };
        depth += 1;
    }
    Ok(())
}

fn walk(
    doc: &Document,
    ctx: &mut Context<'_>,
    roles: &mut Roles,
    value: &Value,
    usage: Usage,
    depth: usize,
) -> ReadResult<()> {
    ctx.step(depth, Source::default())?;
    if let Value::Reference(id) = value {
        ctx.allocate::<usize>(16, Source::default())?;
        if !roles.uses.entry(*id).or_default().insert(usage) {
            return Ok(());
        }
        if usage.role == ObjectRole::Field
            && let Some(owners) = roles.value_sources.get(id)
        {
            for owner in owners {
                ctx.allocate::<usize>(12, Source::default())?;
                roles.uses.entry(*id).or_default().insert(Usage {
                    role: ObjectRole::Field,
                    field: Some(*owner),
                });
            }
        }
        let object = match doc.object(*id) {
            Ok(object) => object,
            // Undefined references have the value null (7.3.10).
            Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::MissingObject => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if object.is_stream() {
            read_length(doc, ctx, roles, *id, object.value(), depth + 1)?;
        }
        return walk(doc, ctx, roles, object.value(), usage, depth + 1);
    }
    match value {
        Value::Array(values) => {
            for value in values {
                let mut child = usage;
                if usage.role == ObjectRole::PageTree {
                    child.role = if value
                        .as_reference()
                        .is_some_and(|id| roles.pages.contains(&id))
                    {
                        ObjectRole::Page
                    } else {
                        ObjectRole::PageTree
                    };
                } else if usage.role == ObjectRole::AcroForm {
                    child.role = ObjectRole::Field;
                    child.field = value.as_reference();
                    if let Some(id) = value
                        .as_reference()
                        .filter(|id| !roles.fields.contains_key(id))
                        && let Some(owner) = roles.widgets.get(&id)
                    {
                        child.role = ObjectRole::Widget;
                        child.field = Some(*owner);
                    }
                } else if usage.role == ObjectRole::Annotation
                    && let Some(owner) = value.as_reference().and_then(|id| roles.widgets.get(&id))
                {
                    child = Usage {
                        role: if value
                            .as_reference()
                            .is_some_and(|id| roles.fields.contains_key(&id))
                        {
                            ObjectRole::Field
                        } else {
                            ObjectRole::Widget
                        },
                        field: Some(*owner),
                    };
                }
                walk(doc, ctx, roles, value, child, depth + 1)?;
            }
        }
        Value::Dictionary(d) => dictionary(doc, ctx, roles, d, usage, depth + 1)?,
        _ => {}
    }
    Ok(())
}
fn dictionary(
    doc: &Document,
    ctx: &mut Context<'_>,
    roles: &mut Roles,
    d: &Dictionary,
    usage: Usage,
    depth: usize,
) -> ReadResult<()> {
    use ObjectRole as R;
    if usage.role == R::Signature {
        return signature_dependencies(doc, ctx, roles, d, depth);
    }
    for (key, value) in d {
        let key = key.as_bytes();
        let role = match (usage.role, key) {
            (R::Catalog, b"Pages") | (R::PageTree, b"Kids") => R::PageTree,
            (R::Catalog, b"AcroForm") | (R::AcroForm, b"Fields") | (R::Field, b"Kids") => {
                R::AcroForm
            }
            (R::Catalog, b"Perms") => R::Permissions,
            (R::Permissions, b"DocMDP" | b"UR3") => R::Signature,
            (R::Page | R::PageTree, b"Parent")
                if value
                    .as_reference()
                    .is_some_and(|id| roles.get(id).any(|u| u.role == R::PageTree)) =>
            {
                continue;
            }
            (R::Widget | R::Field, b"Parent")
                if value
                    .as_reference()
                    .is_some_and(|id| roles.fields.contains_key(&id)) =>
            {
                continue;
            }
            (R::Annotation | R::Widget, b"P")
                if value
                    .as_reference()
                    .is_some_and(|id| roles.pages.contains(&id)) =>
            {
                continue;
            }
            (R::Field, b"P")
                if d.is_name(b"Subtype", b"Widget")
                    && value
                        .as_reference()
                        .is_some_and(|id| roles.pages.contains(&id)) =>
            {
                continue;
            }
            (R::Page, b"Annots") => R::Annotation,
            (R::Page | R::PageTree, _) => R::PageContent,
            (R::Field, b"V") => {
                if usage
                    .field
                    .is_some_and(|id| roles.signed_fields.contains(&id))
                {
                    R::Signature
                } else {
                    R::FieldValue
                }
            }
            (R::Field, b"I" | b"RV") => R::FieldValue,
            (R::Widget, b"AP" | b"AS") => R::WidgetAppearance,
            (R::Field, b"AP" | b"AS") if d.is_name(b"Subtype", b"Widget") => R::WidgetAppearance,
            (R::Annotation, b"AP") => R::AnnotationAppearance,
            (
                R::DocumentInformation
                | R::WidgetAppearance
                | R::AnnotationAppearance
                | R::PageContent
                | R::FieldValue
                | R::Other,
                _,
            ) => usage.role,
            _ => R::Other,
        };
        walk(
            doc,
            ctx,
            roles,
            value,
            Usage {
                role,
                field: usage.field,
            },
            depth,
        )?;
    }
    Ok(())
}

fn read_form(doc: &Document, ctx: &mut Context<'_>, roles: &mut Roles) -> ReadResult<()> {
    if let Some(form) = doc.acro_form()? {
        roles.ambiguous = form.has_xfa;
        let mut names = BTreeSet::new();
        for field in &form.fields {
            ctx.step(0, Source::default())?;
            let Some(id) = field.object else {
                roles.ambiguous = true;
                continue;
            };
            if !names.insert(field.qualified_name.as_str()) {
                roles.ambiguous = true;
            }
            ctx.allocate::<usize>(24 + field.qualified_name.len(), Source::default())?;
            roles.fields.insert(id, field.qualified_name.clone());
            roles.ambiguous |= matches!(field.kind, FieldKind::Other(_));
            // Keep raw values borrowed in the graph, but snapshot effective field
            // values below by following the bounded parent chain.
            let mut current = Some(field);
            let mut value = None;
            for depth in 0..=doc.limits().depth {
                let Some(node) = current else {
                    break;
                };
                ctx.step(depth, Source::default())?;
                if let Some(v) = node.dictionary.get(b"V").filter(|v| !v.is_null()) {
                    ctx.allocate::<u8>(v.retained_bytes(), Source::default())?;
                    value = Some(v.clone());
                    if let Some(source) = node.object {
                        roles.value_sources.entry(source).or_default().insert(id);
                    }
                    break;
                }
                current = node.parent.and_then(|index| form.fields.get(index));
            }
            if let Some(FieldValue::Text(text)) = &field.value {
                ctx.allocate::<u8>(text.len(), Source::default())?;
                value = Some(Value::String(text.as_bytes().to_vec()));
            }
            if let Some(FieldValue::Name(name)) = &field.value {
                value = Some(Value::Name((*name).clone()));
            }
            if let Some(FieldValue::Texts(texts)) = &field.value {
                ctx.allocate::<Value>(texts.len(), Source::default())?;
                for text in texts {
                    ctx.allocate::<u8>(text.len(), Source::default())?;
                }
                value = Some(Value::Array(
                    texts
                        .iter()
                        .map(|t| Value::String(t.as_bytes().to_vec()))
                        .collect(),
                ));
            }
            roles.field_values.insert(id, value);
            if matches!(field.kind, FieldKind::Signature { signed: true }) {
                roles.signed_fields.insert(id);
                if let Some(id) = roles
                    .field_values
                    .get(&id)
                    .and_then(Option::as_ref)
                    .and_then(Value::as_reference)
                {
                    roles.signatures.insert(id);
                }
            }
        }
        for widget in &form.widgets {
            if let (Some(id), Some(owner)) = (
                widget.object,
                form.fields.get(widget.field).and_then(|f| f.object),
            ) {
                ctx.allocate::<usize>(24, Source::default())?;
                roles.widgets.insert(id, owner);
                roles.widget_fields.insert(owner);
            } else {
                roles.ambiguous = true;
            }
        }
    }
    Ok(())
}

fn signature_dependencies(
    doc: &Document,
    ctx: &mut Context<'_>,
    roles: &mut Roles,
    d: &Dictionary,
    depth: usize,
) -> ReadResult<()> {
    let usage = Usage {
        role: ObjectRole::Other,
        field: None,
    };
    for (key, value) in d {
        if key.as_bytes() != b"Reference" {
            walk(doc, ctx, roles, value, usage, depth)?;
            continue;
        }
        let Some(references) = value.as_array() else {
            roles.ambiguous = true;
            continue;
        };
        for reference in references {
            let Some(reference) = reference.as_dictionary() else {
                roles.ambiguous = true;
                continue;
            };
            for (key, value) in reference {
                if key.as_bytes() == b"Data" && Some(value) == doc.trailer().get(b"Root") {
                    continue;
                }
                walk(doc, ctx, roles, value, usage, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn trailer(doc: &Document, ctx: &mut Context<'_>, roles: &mut Roles) -> ReadResult<()> {
    for (key, value) in doc.trailer() {
        if key.as_bytes() == b"Root" || super::equality::TRAILER_KEYS.contains(&key.as_bytes()) {
            continue;
        }
        let role = if key.as_bytes() == b"Info" {
            ObjectRole::DocumentInformation
        } else {
            ObjectRole::Other
        };
        walk(doc, ctx, roles, value, Usage { role, field: None }, 0)?;
    }
    Ok(())
}
