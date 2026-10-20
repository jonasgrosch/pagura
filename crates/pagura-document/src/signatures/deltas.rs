//! Candidate identities from update xrefs and implicit stream dependencies.
use super::roles::Roles;
use crate::{
    Document,
    view::{Context, Source},
};
use pagura_core::{ObjectId, Result};
use std::collections::BTreeSet;

pub(super) fn read(
    ctx: &mut Context<'_>,
    (before, old): (&Document, &Roles),
    (after, new): (&Document, &Roles),
) -> Result<BTreeSet<ObjectId>> {
    let mut ids = BTreeSet::new();
    for &number in after.revisions()[0].entries.keys() {
        ctx.step(0, Source::default())?;
        for doc in [before, after] {
            for (&id, _) in doc
                .slots
                .range(ObjectId::new(number, 0)..=ObjectId::new(number, u16::MAX))
            {
                insert(ctx, &mut ids, id)?;
            }
        }
    }
    // A container rewrite can replace compressed members without listing their
    // type-2 entries again. An indirect Length can similarly alter stream bytes
    // with an unchanged stream xref entry. Follow both dependencies to closure.
    ctx.allocate::<ObjectId>(ids.len(), Source::default())?;
    let mut pending: Vec<_> = ids.iter().copied().collect();
    while let Some(id) = pending.pop() {
        ctx.step(0, Source::default())?;
        for roles in [old, new] {
            for dependent in roles
                .stream_members
                .get(&id)
                .into_iter()
                .flatten()
                .chain(roles.length_dependents.get(&id).into_iter().flatten())
            {
                if insert(ctx, &mut ids, *dependent)? {
                    ctx.allocate::<ObjectId>(1, Source::default())?;
                    pending.push(*dependent);
                }
            }
        }
    }
    Ok(ids)
}
fn insert(ctx: &mut Context<'_>, ids: &mut BTreeSet<ObjectId>, id: ObjectId) -> Result<bool> {
    if ids.contains(&id) {
        return Ok(false);
    }
    ctx.allocate::<usize>(12, Source::default())?;
    Ok(ids.insert(id))
}
