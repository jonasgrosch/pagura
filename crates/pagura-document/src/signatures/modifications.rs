//! Shared revision roles and bounded comparisons under frozen signature policy.
use super::{
    ChangeDecision as D, ModificationAnalysis, ModificationOutcome as O, ObjectChange,
    PermittedModifications, RevisionChanges, RevisionCoverage, Signature, Signatures, comparison,
    deltas,
    history::Facts,
    policy,
    roles::{self, Roles},
};
use crate::{
    Document,
    view::{Context, Source},
};
use pagura_core::Result;
use std::collections::BTreeSet;

pub(super) fn apply(
    doc: &Document,
    signatures: &mut Signatures<'_>,
    facts: Result<Vec<Option<Facts>>>,
) {
    // Structural source checks have their own shared budget. Changes cannot
    // erase an already established ByteRange or revision coverage result.
    let mut ctx = Context::new(doc);
    let oldest_signed = signatures
        .fields
        .iter()
        .filter_map(|f| f.signature.as_ref())
        .chain(signatures.usage_rights.iter())
        .chain(signatures.historical.iter().map(|h| &h.signature))
        .filter_map(|s| match s.results.revision_coverage {
            RevisionCoverage::Covered { index, .. } => Some(index),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let prepared = facts.and_then(|facts| {
        if signatures.history_complete && oldest_signed > 0 {
            prepare(doc, &mut ctx, &facts, oldest_signed)
        } else {
            Ok((Vec::new(), Vec::new()))
        }
    });
    for signature in signatures
        .fields
        .iter_mut()
        .filter_map(|f| f.signature.as_mut())
        .chain(signatures.usage_rights.iter_mut())
        .chain(signatures.historical.iter_mut().map(|h| &mut h.signature))
    {
        signature.results.permitted_modifications = match signature.results.revision_coverage {
            RevisionCoverage::Covered { index, .. } if signatures.history_complete => {
                if index == 0 {
                    PermittedModifications::Analysed(ModificationAnalysis {
                        signed_revision: 0,
                        revisions: Vec::new(),
                        outcome: O::NoChanges,
                    })
                } else if let Ok((roles, updates)) = &prepared {
                    analyse(doc, &mut ctx, signature, index, roles, updates)
                        .unwrap_or(PermittedModifications::Inconclusive)
                } else {
                    PermittedModifications::Inconclusive
                }
            }
            RevisionCoverage::Covered { .. }
            | RevisionCoverage::RepairedDocument
            | RevisionCoverage::Indeterminate => PermittedModifications::Inconclusive,
            _ => PermittedModifications::NotAnalysedYet,
        };
    }
}

type Prepared = (Vec<Option<Roles>>, Vec<BTreeSet<pagura_core::ObjectId>>);
fn prepare(
    doc: &Document,
    ctx: &mut Context<'_>,
    facts: &[Option<Facts>],
    oldest_signed: usize,
) -> Result<Prepared> {
    let count = oldest_signed + 1;
    ctx.allocate::<Option<Roles>>(count, Source::default())?;
    let mut roles = Vec::new();
    for (index, facts) in facts.iter().take(count).enumerate() {
        let snapshot = doc.revision_with_context(index, ctx)?;
        roles.push(
            facts
                .as_ref()
                .map(|facts| roles::read(snapshot, ctx, facts))
                .transpose()?,
        );
    }
    ctx.allocate::<BTreeSet<pagura_core::ObjectId>>(count, Source::default())?;
    let mut updates = Vec::new();
    for revision in 0..count.saturating_sub(1) {
        let current = doc.revision_with_context(revision, ctx)?;
        let previous = doc.revision_with_context(revision + 1, ctx)?;
        let (Some(old), Some(new)) = (&roles[revision + 1], &roles[revision]) else {
            return Err(pagura_core::Error::syntax("revision roles unavailable"));
        };
        updates.push(deltas::read(ctx, (previous, old), (current, new))?);
    }
    Ok((roles, updates))
}
fn analyse(
    doc: &Document,
    ctx: &mut Context<'_>,
    signature: &Signature<'_>,
    index: usize,
    roles: &[Option<Roles>],
    updates: &[BTreeSet<pagura_core::ObjectId>],
) -> Result<PermittedModifications> {
    let mut result = ModificationAnalysis {
        signed_revision: index,
        revisions: Vec::new(),
        outcome: O::NoChanges,
    };
    let signed = doc.revision_with_context(index, ctx)?;
    let policy = policy::read(signed, signature, ctx)?;
    let signed_roles = roles[index]
        .as_ref()
        .ok_or_else(|| pagura_core::Error::syntax("signed roles unavailable"))?;
    let mut net_ids = BTreeSet::new();
    for revision in (0..index).rev() {
        ctx.step(0, Source::default())?;
        let current = doc.revision_with_context(revision, ctx)?;
        let previous = doc.revision_with_context(revision + 1, ctx)?;
        let old = roles[revision + 1]
            .as_ref()
            .ok_or_else(|| pagura_core::Error::syntax("previous roles unavailable"))?;
        let new = roles[revision]
            .as_ref()
            .ok_or_else(|| pagura_core::Error::syntax("current roles unavailable"))?;
        let ids = &updates[revision];
        ctx.allocate::<usize>(ids.len().saturating_mul(12), Source::default())?;
        net_ids.extend(ids);
        let changes = comparison::read(
            ctx,
            (previous, old),
            (current, new),
            (signed_roles, &policy),
            ids,
            &[],
        )?;
        let net_changes = comparison::read(
            ctx,
            (signed, signed_roles),
            (current, new),
            (signed_roles, &policy),
            &net_ids,
            &roles[revision..=index],
        )?;
        result.outcome = combine(result.outcome, outcome(&changes));
        result.outcome = combine(result.outcome, outcome(&net_changes));
        ctx.allocate::<RevisionChanges>(1, Source::default())?;
        result.revisions.push(RevisionChanges {
            revision,
            changes,
            net_changes,
            outcome: result.outcome,
        });
    }
    Ok(PermittedModifications::Analysed(result))
}
fn combine(a: O, b: O) -> O {
    match (a, b) {
        (O::Disallowed, _) | (_, O::Disallowed) => O::Disallowed,
        (O::Inconclusive, _) | (_, O::Inconclusive) => O::Inconclusive,
        (O::Allowed, _) | (_, O::Allowed) => O::Allowed,
        _ => O::NoChanges,
    }
}
fn outcome(changes: &[ObjectChange]) -> O {
    changes.iter().fold(O::NoChanges, |result, change| {
        let decision = match (change.doc_mdp, change.field_mdp) {
            (D::Disallowed, _) | (_, D::Disallowed) => O::Disallowed,
            (D::Inconclusive, _) | (_, D::Inconclusive) => O::Inconclusive,
            _ => O::Allowed,
        };
        combine(result, decision)
    })
}
