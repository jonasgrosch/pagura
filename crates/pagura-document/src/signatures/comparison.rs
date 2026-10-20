//! Conservative comparisons under policy frozen at signing.
use super::{
    ChangeDecision as D, ChangeKind, ChangeTarget, ModificationRule as R, ObjectChange, ObjectRole,
    classification, equality,
    policy::Policy,
    roles::{Roles, Usage},
};
use crate::{
    Document, IndirectObject,
    view::{Context, Source},
};
use pagura_core::{ObjectId, Result};
use std::collections::BTreeSet;
pub(super) fn read(
    ctx: &mut Context<'_>,
    (before, old): (&Document, &Roles),
    (after, new): (&Document, &Roles),
    (signed, policy): (&Roles, &Policy),
    ids: &BTreeSet<ObjectId>,
    past: &[Option<Roles>],
) -> Result<Vec<ObjectChange>> {
    let mut changed_fields = BTreeSet::new();
    for id in old.fields.keys().chain(new.fields.keys()) {
        ctx.step(0, Source::default())?;
        if old.fields.get(id) != new.fields.get(id) {
            ctx.allocate::<usize>(8, Source::default())?;
            changed_fields.insert(*id);
        }
    }
    let mut changes = Vec::new();
    for &id in ids {
        ctx.step(0, Source::default())?;
        let a = before
            .slots
            .contains_key(&id)
            .then(|| before.object(id))
            .transpose()?;
        let b = after
            .slots
            .contains_key(&id)
            .then(|| after.object(id))
            .transpose()?;
        let kind = match (a, b) {
            (None, None) => continue,
            (Some(a), Some(b)) if equality::equal(ctx, before, after, a, b)? => continue,
            (Some(_), Some(_)) => ChangeKind::Modified,
            (None, Some(_)) => ChangeKind::Added,
            _ => ChangeKind::Removed,
        };
        ctx.allocate::<ObjectChange>(1, Source::default())?;
        let (object_roles, doc_mdp, field_mdp, rule) = decision(
            ctx,
            id,
            (a, b),
            (old, new, signed, policy, &changed_fields),
            past,
        )?;
        changes.push(ObjectChange {
            target: ChangeTarget::Object(id),
            kind,
            visible_overlay: object_roles_overlay(
                ctx,
                &object_roles,
                rule,
                (before, a),
                (after, b),
            )?,
            roles: object_roles,
            doc_mdp,
            field_mdp,
            rule,
        });
    }
    if let Some(change) = trailer_change(ctx, before, after, policy)? {
        changes.push(change);
    }
    Ok(changes)
}

fn decision(
    ctx: &mut Context<'_>,
    id: ObjectId,
    (a, b): (Option<&IndirectObject>, Option<&IndirectObject>),
    (old, new, signed, policy, changed_fields): (
        &Roles,
        &Roles,
        &Roles,
        &Policy,
        &BTreeSet<ObjectId>,
    ),
    past: &[Option<Roles>],
) -> Result<(Vec<ObjectRole>, D, D, R)> {
    let mut usages = BTreeSet::new();
    for usage in old
        .get(id)
        .chain(new.get(id))
        .chain(past.iter().flatten().flat_map(|roles| roles.get(id)))
    {
        ctx.step(0, Source::default())?;
        if !usages.contains(&usage) {
            ctx.allocate::<Usage>(4, Source::default())?;
            usages.insert(usage);
        }
    }
    ctx.allocate::<ObjectRole>(usages.len(), Source::default())?;
    let mut object_roles: Vec<_> = usages.iter().map(|u| u.role).collect();
    object_roles.sort_unstable();
    object_roles.dedup();
    if object_roles.is_empty() {
        object_roles.push(ObjectRole::Other);
    }
    let (mut doc_mdp, rule) =
        classification::read(id, (a, b), (old, new), &object_roles, policy.permission);
    if !matches!(rule, R::CrossReference) {
        doc_mdp = match (policy.permission, doc_mdp) {
            (Some(super::MdpPermission::NoChanges), _) if rule != R::Unclassified => D::Disallowed,
            (Some(super::MdpPermission::FormsAndSignatures), D::Allowed)
                if rule == R::Annotation =>
            {
                D::Disallowed
            }
            (None, D::Allowed) => D::NotApplicable,
            _ => doc_mdp,
        };
    }
    if policy.unknown && doc_mdp != D::Disallowed {
        doc_mdp = D::Inconclusive;
    }
    let field_mdp = field_decision(
        ctx,
        id,
        &object_roles,
        &usages,
        (old, new, signed, policy),
        changed_fields,
        rule == R::DocumentStructure,
    )?;
    let rule =
        if policy.permission == Some(super::MdpPermission::NoChanges) && doc_mdp == D::Disallowed {
            R::NoChanges
        } else {
            rule
        };
    Ok((object_roles, doc_mdp, field_mdp, rule))
}

fn descendant(parent: ObjectId, child: ObjectId, old: &Roles, new: &Roles) -> bool {
    [old, new].into_iter().any(|roles| {
        roles
            .fields
            .get(&parent)
            .zip(roles.fields.get(&child))
            .is_some_and(|(parent, child)| {
                child
                    .strip_prefix(parent.as_str())
                    .is_some_and(|tail| tail.starts_with('.'))
            })
    })
}

fn trailer_change(
    ctx: &mut Context<'_>,
    before: &Document,
    after: &Document,
    policy: &Policy,
) -> Result<Option<ObjectChange>> {
    let a = before.trailer();
    let b = after.trailer();
    if equality::same_dictionary(ctx, a, b, equality::TRAILER_KEYS)? {
        return Ok(None);
    }
    ctx.allocate::<ObjectChange>(1, Source::default())?;
    let mut ignored = equality::TRAILER_KEYS.to_vec();
    ignored.push(b"ID");
    let only_id = equality::same_dictionary(ctx, a, b, &ignored)?;
    let stable_id = match (
        a.get(b"ID").and_then(pagura_core::Value::as_array),
        b.get(b"ID").and_then(pagura_core::Value::as_array),
    ) {
        (Some([first, second]), Some([new_first, new_second])) => {
            first.as_string().is_some()
                && second.as_string().is_some()
                && new_second.as_string().is_some()
                && first == new_first
        }
        _ => false,
    };
    let forbidden = [b"Root".as_slice(), b"Encrypt"]
        .iter()
        .any(|key| a.get(key) != b.get(key))
        || (!stable_id && a.get(b"ID") != b.get(b"ID"));
    let (mut doc_mdp, rule) = if only_id && stable_id {
        (D::Allowed, R::CrossReference)
    } else if forbidden {
        (D::Disallowed, R::DocumentStructure)
    } else {
        (D::Inconclusive, R::Unclassified)
    };
    if policy.unknown && doc_mdp != D::Disallowed {
        doc_mdp = D::Inconclusive;
    }
    Ok(Some(ObjectChange {
        target: ChangeTarget::Trailer,
        kind: ChangeKind::Modified,
        roles: vec![if only_id && stable_id {
            ObjectRole::CrossReference
        } else {
            ObjectRole::Other
        }],
        doc_mdp,
        field_mdp: if policy.field_unknown {
            D::Inconclusive
        } else if policy.selections.is_empty() {
            D::NotApplicable
        } else if only_id && stable_id {
            D::Allowed
        } else {
            D::Inconclusive
        },
        rule,
        visible_overlay: false,
    }))
}

fn field_decision(
    ctx: &mut Context<'_>,
    id: ObjectId,
    roles: &[ObjectRole],
    usages: &BTreeSet<Usage>,
    (old, new, signed, policy): (&Roles, &Roles, &Roles, &Policy),
    changed_fields: &BTreeSet<ObjectId>,
    structural: bool,
) -> Result<D> {
    let mut locked = false;
    for owner in usages
        .iter()
        .filter_map(|u| u.field)
        .chain(classification::changed_annotation_owners(id, old, new))
    {
        locked |= policy.locked(ctx, owner, signed, new)?;
    }
    // A container can remove or rename selected descendants while their own
    // indirect dictionaries stay unchanged.
    for owner in changed_fields {
        ctx.step(0, Source::default())?;
        if roles.contains(&ObjectRole::AcroForm)
            || (roles.contains(&ObjectRole::Field) && descendant(id, *owner, old, new))
        {
            locked |= policy.locked(ctx, *owner, signed, new)?;
        }
    }
    if locked {
        return Ok(D::Disallowed);
    }
    if policy.selections.is_empty() && !policy.field_unknown {
        return Ok(D::NotApplicable);
    }
    let mut uncertain = policy.field_unknown
        || old.ambiguous
        || new.ambiguous
        || signed.ambiguous
        || roles.contains(&ObjectRole::Other);
    if structural {
        uncertain |= roles.contains(&ObjectRole::AcroForm);
        if roles.contains(&ObjectRole::Field) {
            for owner in old
                .fields
                .keys()
                .filter(|owner| descendant(id, **owner, old, new))
            {
                ctx.step(0, Source::default())?;
                if policy.locked(ctx, *owner, signed, new)? {
                    return Ok(D::Disallowed);
                }
            }
        }
    }
    Ok(if uncertain {
        D::Inconclusive
    } else {
        D::Allowed
    })
}

fn object_roles_overlay(
    ctx: &Context<'_>,
    roles: &[ObjectRole],
    rule: R,
    before: (&Document, Option<&IndirectObject>),
    after: (&Document, Option<&IndirectObject>),
) -> Result<bool> {
    if roles.contains(&ObjectRole::AnnotationAppearance)
        || (matches!(rule, R::Signing | R::Annotation)
            && roles.iter().any(|r| {
                matches!(
                    r,
                    ObjectRole::Widget | ObjectRole::Field | ObjectRole::WidgetAppearance
                )
            }))
    {
        return Ok(true);
    }
    let has_appearance = |(doc, object): (&Document, Option<&IndirectObject>)| -> Result<bool> {
        let Some(value) = object
            .and_then(IndirectObject::dictionary)
            .and_then(|d| d.get(b"AP"))
        else {
            return Ok(false);
        };
        let mut source = ctx.for_document(doc);
        Ok(!source.resolve(value, Source::default())?.0.is_null())
    };
    Ok(has_appearance(after)?
        && (roles.contains(&ObjectRole::Annotation) || !has_appearance(before)?))
}
