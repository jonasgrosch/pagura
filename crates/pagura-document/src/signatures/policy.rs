//! Freeze permissions and decoded field selections in the signed revision.
use super::{FieldSelection, MdpPermission, Signature, Transform, roles::Roles, transforms};
use crate::{
    Document,
    view::{Context, Source, text},
};
use pagura_core::{Dictionary, Error, ObjectId, Result, Value};
use std::collections::BTreeSet;

pub(super) struct Selection {
    all: bool,
    exclude: bool,
    names: BTreeSet<String>,
}
impl Selection {
    fn read(ctx: &mut Context<'_>, selection: &FieldSelection<'_>) -> Result<Self> {
        let (all, exclude, raw) = match selection {
            FieldSelection::All => (true, false, &[][..]),
            FieldSelection::Include(names) => (false, false, names.as_slice()),
            FieldSelection::Exclude(names) => (false, true, names.as_slice()),
        };
        let mut names = BTreeSet::new();
        for name in raw {
            ctx.allocate::<usize>(12, Source::default())?;
            names.insert(text::decode(ctx, name, Source::default())?);
        }
        Ok(Self {
            all,
            exclude,
            names,
        })
    }
    fn locks(&self, ctx: &mut Context<'_>, name: &str) -> Result<bool> {
        ctx.step(0, Source::default())?;
        if self.all {
            return Ok(true);
        }
        let mut included = self.names.contains(name);
        if !included {
            for (at, _) in name.match_indices('.') {
                ctx.step(0, Source::default())?;
                if self.names.contains(&name[..at]) {
                    included = true;
                    break;
                }
            }
        }
        Ok(included != self.exclude)
    }
}
pub(super) struct Policy {
    pub permission: Option<MdpPermission>,
    pub selections: Vec<Selection>,
    pub unknown: bool,
    pub field_unknown: bool,
}
impl Policy {
    pub(super) fn locked(
        &self,
        ctx: &mut Context<'_>,
        id: ObjectId,
        signed: &Roles,
        later: &Roles,
    ) -> Result<bool> {
        // Original identity and name cannot be escaped by renaming a locked
        // field. New fields are tested too, particularly for All and Exclude.
        for name in [signed.fields.get(&id), later.fields.get(&id)]
            .into_iter()
            .flatten()
        {
            for selection in &self.selections {
                if selection.locks(ctx, name)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
fn minimum(a: MdpPermission, b: MdpPermission) -> MdpPermission {
    match (a, b) {
        (MdpPermission::NoChanges, _) | (_, MdpPermission::NoChanges) => MdpPermission::NoChanges,
        (MdpPermission::FormsAndSignatures, _) | (_, MdpPermission::FormsAndSignatures) => {
            MdpPermission::FormsAndSignatures
        }
        _ => MdpPermission::FormsSignaturesAndAnnotations,
    }
}
fn references<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    policy: &mut Policy,
    own: bool,
) -> Result<()> {
    for reference in transforms::references(ctx, d, Source::default())? {
        match reference.transform {
            Transform::DocMdp(p) => {
                policy.permission = Some(policy.permission.map_or(p, |old| minimum(old, p)));
            }
            Transform::FieldMdp(selection) if own => {
                if reference.data.and_then(Value::as_reference)
                    != ctx.doc.trailer().get(b"Root").and_then(Value::as_reference)
                {
                    policy.field_unknown = true;
                    continue;
                }
                policy.selections.push(Selection::read(ctx, &selection)?);
            }
            Transform::FieldMdp(_) => {}
            _ => policy.unknown = true,
        }
    }
    Ok(())
}
pub(super) fn read(
    doc: &Document,
    signature: &Signature<'_>,
    shared: &Context<'_>,
) -> Result<Policy> {
    let mut ctx = shared.for_document(doc);
    let mut policy = Policy {
        permission: None,
        selections: Vec::new(),
        unknown: false,
        field_unknown: false,
    };
    let dictionary = if let Some(id) = signature.object {
        doc.object(id)?
            .dictionary()
            .filter(|d| **d == *signature.dictionary)
    } else {
        None
    }
    .ok_or_else(|| Error::syntax("signature identity does not bind to signed revision"))?;
    references(&mut ctx, dictionary, &mut policy, true)?;
    let catalog = doc.catalog()?;
    if let Some(perms) = catalog.get(b"Perms") {
        let perms = doc.resolve_dictionary(perms)?;
        if let Some(certification) = perms.get(b"DocMDP") {
            references(
                &mut ctx,
                doc.resolve_dictionary(certification)?,
                &mut policy,
                false,
            )?;
        }
    }
    // Lock is signing-time data; the current revision's mutable Lock never
    // supplies policy. Its optional P can tighten, never relax certification.
    if let Some(form) = doc.acro_form()? {
        for field in &form.fields {
            ctx.step(0, Source::default())?;
            if !matches!(field.value.as_ref(), Some(crate::forms::FieldValue::Signature(d)) if **d == *dictionary)
            {
                continue;
            }
            if let Some(lock) = field.dictionary.get(b"Lock") {
                let lock =
                    transforms::lock(&mut ctx, doc.resolve_dictionary(lock)?, Source::default())?;
                policy
                    .selections
                    .push(Selection::read(&mut ctx, &lock.fields)?);
                if let Some(p) = lock.permission {
                    policy.permission = Some(policy.permission.map_or(p, |old| minimum(old, p)));
                }
            }
        }
    }
    Ok(policy)
}
