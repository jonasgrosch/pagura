//! Recognized semantic operations (12.8.2.2, Table 257).
use super::{ChangeDecision as D, ModificationRule as R, ObjectRole, roles::Roles};
use crate::IndirectObject;
use pagura_core::{Dictionary, ObjectId, Value};
use std::collections::BTreeSet;
fn unchanged_except(
    a: Option<&IndirectObject>,
    b: Option<&IndirectObject>,
    keys: &[&[u8]],
) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return false;
    };
    let (Some(a), Some(b)) = (a.dictionary(), b.dictionary()) else {
        return false;
    };
    a.iter()
        .chain(b.iter())
        .all(|(k, _)| keys.contains(&k.as_bytes()) || a.get(k.as_bytes()) == b.get(k.as_bytes()))
}
fn changed_field(id: ObjectId, old: &Roles, new: &Roles) -> bool {
    old.field_values.get(&id) != new.field_values.get(&id)
}
pub(super) fn read(
    id: ObjectId,
    (a, b): (Option<&IndirectObject>, Option<&IndirectObject>),
    (old, new): (&Roles, &Roles),
    roles: &[ObjectRole],
    permission: Option<super::MdpPermission>,
) -> (D, R) {
    use ObjectRole as T;
    // Rendering dependencies remain forbidden even alongside an unsupported
    // template operation; uncertainty cannot hide a proven content violation.
    if roles.contains(&T::PageContent) {
        return (D::Disallowed, R::DocumentStructure);
    }
    if (old.templates_present || new.templates_present)
        && old.pages.len() != new.pages.len()
        && ((roles == [T::PageTree] && unchanged_except(a, b, &[b"Kids", b"Count"]))
            || (roles == [T::Page]
                && (a.is_none() || b.is_none() || unchanged_except(a, b, &[b"Parent"]))))
    {
        return (D::Inconclusive, R::Unclassified);
    }
    if roles == [T::CrossReference] {
        return (D::Allowed, R::CrossReference);
    }
    if old.ambiguous || new.ambiguous || roles.contains(&T::Other) {
        return (D::Inconclusive, R::Unclassified);
    }
    if timestamp_change(id, roles, old, new) {
        return (D::Inconclusive, R::Unclassified);
    }

    if roles == [ObjectRole::Page]
        && unchanged_except(a, b, &[b"Annots"])
        && signing_annotations(id, old, new)
    {
        return (D::Allowed, R::Signing);
    }
    if annotation_array(id, (a, b), (old, new), roles) {
        return (D::Allowed, R::Signing);
    }
    operation(id, (a, b), (old, new), roles, permission)
}

fn operation(
    id: ObjectId,
    (a, b): (Option<&IndirectObject>, Option<&IndirectObject>),
    (old, new): (&Roles, &Roles),
    roles: &[ObjectRole],
    permission: Option<super::MdpPermission>,
) -> (D, R) {
    use ObjectRole as T;
    if roles.iter().all(|r| matches!(r, T::Field | T::Widget)) {
        return field_operation(id, (a, b), (old, new), permission);
    }
    if roles == [T::FieldValue]
        && a.into_iter()
            .chain(b)
            .all(|o| !matches!(o.value(), Value::Dictionary(_) | Value::Array(_)) || o.is_stream())
    {
        return (D::Allowed, R::FormFill);
    }
    if roles == [T::WidgetAppearance] {
        let owners: BTreeSet<_> = old
            .get(id)
            .chain(new.get(id))
            .filter_map(|u| u.field)
            .collect();
        if !owners.is_empty() && owners.iter().all(|owner| changed_field(*owner, old, new)) {
            let signing = owners.iter().all(|owner| {
                new.field_values
                    .get(owner)
                    .and_then(Option::as_ref)
                    .and_then(Value::as_reference)
                    .is_some_and(|id| later_signature(id, old, new))
            });
            return (D::Allowed, if signing { R::Signing } else { R::FormFill });
        }
        if permission == Some(super::MdpPermission::FormsSignaturesAndAnnotations)
            && widget_appearance(id, old, new)
        {
            return (D::Allowed, R::Annotation);
        }
        return (D::Inconclusive, R::Unclassified);
    }
    if roles == [T::Signature] && a.is_none() {
        return if later_signature(id, old, new) {
            (D::Allowed, R::Signing)
        } else {
            (D::Inconclusive, R::Unclassified)
        };
    }
    if roles == [T::AcroForm]
        && unchanged_except(a, b, &[b"Fields", b"SigFlags"])
        && signing_fields(a, b, old, new)
    {
        return (D::Allowed, R::Signing);
    }
    if roles == [T::Page] && unchanged_except(a, b, &[b"Annots"]) {
        return (D::Allowed, R::Annotation);
    }
    if roles
        .iter()
        .all(|r| matches!(r, T::Annotation | T::AnnotationAppearance))
    {
        let annotations = a
            .into_iter()
            .chain(b)
            .filter_map(IndirectObject::dictionary);
        if roles == [T::AnnotationAppearance] || annotations.into_iter().all(simple_annotation) {
            return (D::Allowed, R::Annotation);
        }
        return (D::Inconclusive, R::Unclassified);
    }
    if roles == [T::Catalog] && unchanged_except(a, b, &[b"DSS"]) {
        return (D::Inconclusive, R::Unclassified);
    }
    if roles.iter().any(|r| {
        matches!(
            r,
            T::Page | T::PageTree | T::Catalog | T::AcroForm | T::Permissions | T::Signature
        )
    }) {
        return (D::Disallowed, R::DocumentStructure);
    }
    (D::Inconclusive, R::Unclassified)
}
fn simple_annotation(d: &Dictionary) -> bool {
    // Interactive actions, optional content, rich media and extensions require
    // additional semantics; a convenient /Type spelling is insufficient.
    d.get(b"Subtype").and_then(Value::as_name).is_some_and(|n| {
        matches!(
            n.as_bytes(),
            b"Text"
                | b"FreeText"
                | b"Line"
                | b"Square"
                | b"Circle"
                | b"Polygon"
                | b"PolyLine"
                | b"Highlight"
                | b"Underline"
                | b"Squiggly"
                | b"StrikeOut"
                | b"Stamp"
                | b"Ink"
                | b"Popup"
                | b"Caret"
        )
    }) && d.iter().all(|(k, _)| {
        [
            b"Type".as_slice(),
            b"Subtype",
            b"Rect",
            b"Contents",
            b"P",
            b"NM",
            b"M",
            b"F",
            b"AP",
            b"AS",
            b"Border",
            b"C",
            b"CA",
            b"T",
            b"Subj",
            b"Open",
            b"Name",
            b"QuadPoints",
            b"InkList",
            b"L",
            b"BS",
            b"IC",
            b"Vertices",
            b"LE",
            b"DA",
            b"Q",
            b"Parent",
            b"Popup",
        ]
        .contains(&k.as_bytes())
    })
}

fn later_signature(id: ObjectId, old: &Roles, new: &Roles) -> bool {
    // An old dictionary reattached to a new field is not a signing operation.
    // Net comparisons can still include signatures from intermediate updates.
    new.signature_ends
        .get(&id)
        .is_some_and(|&end| end > old.source_end)
}
fn valid_new_field(
    id: ObjectId,
    object: Option<&IndirectObject>,
    old: &Roles,
    roles: &Roles,
) -> bool {
    roles.signed_fields.contains(&id)
        && roles
            .field_values
            .get(&id)
            .and_then(Option::as_ref)
            .and_then(Value::as_reference)
            .is_some_and(|id| later_signature(id, old, roles))
        && object
            .and_then(IndirectObject::dictionary)
            .is_some_and(|d| {
                d.iter().all(|(k, _)| {
                    [
                        b"FT".as_slice(),
                        b"T",
                        b"V",
                        b"Ff",
                        b"Type",
                        b"Subtype",
                        b"Rect",
                        b"P",
                        b"AP",
                        b"AS",
                        b"F",
                        b"Lock",
                    ]
                    .contains(&k.as_bytes())
                })
            })
}
fn signing_fields(
    a: Option<&IndirectObject>,
    b: Option<&IndirectObject>,
    old: &Roles,
    new: &Roles,
) -> bool {
    if !old
        .fields
        .iter()
        .all(|(id, name)| new.fields.get(id) == Some(name))
    {
        return false;
    }
    let (Some(a), Some(b)) = (
        a.and_then(IndirectObject::dictionary),
        b.and_then(IndirectObject::dictionary),
    ) else {
        return false;
    };
    let prior_flags = a.get(b"SigFlags").and_then(Value::as_integer).unwrap_or(0);
    let next_flags = b.get(b"SigFlags").and_then(Value::as_integer).unwrap_or(0);
    // Table 225: signing can set the two defined hints. Clearing existing
    // hints or changing extension bits is not an incidental signing update.
    if prior_flags & !next_flags != 0 || (prior_flags ^ next_flags) & !3 != 0 {
        return false;
    }
    let (Some(a), Some(b)) = (
        a.get(b"Fields").and_then(Value::as_array),
        b.get(b"Fields").and_then(Value::as_array),
    ) else {
        return false;
    };
    let mut old_roots = a.iter();
    for value in b {
        let Some(id) = value.as_reference() else {
            return false;
        };
        if old.fields.contains_key(&id) {
            if old_roots.next() != Some(value) {
                return false;
            }
        } else if !new.signed_fields.contains(&id)
            || !new
                .field_values
                .get(&id)
                .and_then(Option::as_ref)
                .and_then(Value::as_reference)
                .is_some_and(|signature| later_signature(signature, old, new))
        {
            return false;
        }
    }
    old_roots.next().is_none()
        && new
            .signed_fields
            .iter()
            .any(|id| !old.signed_fields.contains(id))
}
pub(super) fn changed_annotation_owners<'a>(
    id: ObjectId,
    old: &'a Roles,
    new: &'a Roles,
) -> impl Iterator<Item = ObjectId> + 'a {
    let pages = std::iter::once(id)
        .chain(
            old.annotation_sources
                .get(&id)
                .into_iter()
                .flatten()
                .copied(),
        )
        .chain(
            new.annotation_sources
                .get(&id)
                .into_iter()
                .flatten()
                .copied(),
        );
    pages.flat_map(|page| {
        let a = old.annotation_members.get(&page);
        let b = new.annotation_members.get(&page);
        a.into_iter()
            .flatten()
            .filter(move |id| b.is_none_or(|b| !b.contains(id)))
            .chain(
                b.into_iter()
                    .flatten()
                    .filter(move |id| a.is_none_or(|a| !a.contains(id))),
            )
            .filter_map(|id| old.widgets.get(id).or_else(|| new.widgets.get(id)))
            .copied()
    })
}

fn signing_annotations(page: ObjectId, old: &Roles, new: &Roles) -> bool {
    let a = old
        .page_annotations
        .get(&page)
        .map_or(&[][..], Vec::as_slice);
    let b = new
        .page_annotations
        .get(&page)
        .map_or(&[][..], Vec::as_slice);
    let members = old.annotation_members.get(&page);
    let mut prior = a.iter();
    for id in b {
        if members.is_some_and(|set| set.contains(id)) {
            if prior.next() != Some(id) {
                return false;
            }
        } else if !new.widgets.get(id).is_some_and(|owner| {
            new.signed_fields.contains(owner) && !old.signed_fields.contains(owner)
        }) {
            return false;
        }
    }
    prior.next().is_none() && a != b
}

fn timestamp_change(id: ObjectId, roles: &[ObjectRole], old: &Roles, new: &Roles) -> bool {
    if new.timestamp_signatures.contains(&id) && !old.timestamp_signatures.contains(&id) {
        return true;
    }
    let newly_timestamped = |field: ObjectId| {
        new.field_values
            .get(&field)
            .and_then(Option::as_ref)
            .and_then(Value::as_reference)
            .is_some_and(|sig| {
                new.timestamp_signatures.contains(&sig) && !old.timestamp_signatures.contains(&sig)
            })
    };
    if old
        .get(id)
        .chain(new.get(id))
        .filter_map(|u| u.field)
        .any(newly_timestamped)
    {
        return true;
    }
    roles == [ObjectRole::AcroForm] && new.signed_fields.iter().copied().any(newly_timestamped)
}

fn field_operation(
    id: ObjectId,
    (a, b): (Option<&IndirectObject>, Option<&IndirectObject>),
    (old, new): (&Roles, &Roles),
    permission: Option<super::MdpPermission>,
) -> (D, R) {
    if a.is_none() && new.signed_fields.contains(&id) {
        return if valid_new_field(id, b, old, new) {
            (D::Allowed, R::Signing)
        } else {
            (D::Inconclusive, R::Unclassified)
        };
    }
    if old.signed_fields.contains(&id) && changed_field(id, old, new) {
        return (D::Disallowed, R::DocumentStructure);
    }
    if unchanged_except(a, b, &[b"V", b"I", b"RV", b"AP", b"AS"]) {
        let owners: BTreeSet<_> = old
            .get(id)
            .chain(new.get(id))
            .filter_map(|u| u.field)
            .collect();
        if !owners.is_empty() && owners.iter().all(|id| changed_field(*id, old, new)) {
            if new.signed_fields.contains(&id) && !valid_new_field(id, b, old, new) {
                return (D::Inconclusive, R::Unclassified);
            }
            return (
                D::Allowed,
                if new.signed_fields.contains(&id) {
                    R::Signing
                } else {
                    R::FormFill
                },
            );
        }
    }
    if old.widgets.contains_key(&id) && old.widgets.get(&id) == new.widgets.get(&id) {
        if unchanged_except(
            a,
            b,
            &[
                b"Rect", b"F", b"Border", b"MK", b"BS", b"H", b"P", b"NM", b"M",
            ],
        ) {
            return (D::Allowed, R::Annotation);
        }
        if permission == Some(super::MdpPermission::FormsSignaturesAndAnnotations)
            && unchanged_except(a, b, &[b"AP", b"AS"])
        {
            return (D::Allowed, R::Annotation);
        }
    }
    (D::Disallowed, R::DocumentStructure)
}
fn widget_appearance(id: ObjectId, old: &Roles, new: &Roles) -> bool {
    let mut found = false;
    for owner in old
        .get(id)
        .chain(new.get(id))
        .filter_map(|usage| usage.field)
    {
        found = true;
        if !old.widget_fields.contains(&owner) && !new.widget_fields.contains(&owner) {
            return false;
        }
    }
    found
}

fn annotation_array(
    id: ObjectId,
    (a, b): (Option<&IndirectObject>, Option<&IndirectObject>),
    (old, new): (&Roles, &Roles),
    roles: &[ObjectRole],
) -> bool {
    if roles != [ObjectRole::Annotation]
        || !a
            .into_iter()
            .chain(b)
            .all(|o| o.value().as_array().is_some())
    {
        return false;
    }
    let mut found = false;
    for page in old
        .annotation_sources
        .get(&id)
        .into_iter()
        .flatten()
        .chain(new.annotation_sources.get(&id).into_iter().flatten())
    {
        found = true;
        if !signing_annotations(*page, old, new) {
            return false;
        }
    }
    found
}
