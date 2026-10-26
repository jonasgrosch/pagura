//! Discover signatures hidden by later field removal or replacement (7.5.6).
use super::{HistoricalSignature, RevisionCoverage, Signature, Signatures, read};
use crate::source::{ReadFault, ReadResult};
use crate::{
    Document, ObjectSource,
    view::{Context, Source},
};
use pagura_core::{ErrorKind, ObjectId, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Facts {
    pub signature_ends: BTreeMap<ObjectId, usize>,
    pub timestamps: BTreeSet<ObjectId>,
}
impl Facts {
    fn read(signatures: &Signatures<'_>, ctx: &mut Context<'_>) -> Result<Self> {
        let mut facts = Self::default();
        for signature in signatures
            .fields
            .iter()
            .filter_map(|f| f.signature.as_ref())
            .chain(signatures.usage_rights.iter())
        {
            ctx.step(0, Source::default())?;
            if matches!(
                signature.results.revision_coverage,
                RevisionCoverage::Covered { .. }
            ) && let Some(id) = signature.object
                && let Some(range) = &signature.byte_range
            {
                ctx.allocate::<usize>(16, Source::default())?;
                facts.signature_ends.insert(id, range.signed_end());
                if signature.sub_filter == super::SubFilter::Rfc3161 {
                    facts.timestamps.insert(id);
                }
            }
        }
        Ok(facts)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Identity {
    object: ObjectId,
    offset: usize,
    contents: Option<usize>,
}
fn key(doc: &Document, signature: &Signature<'_>) -> ReadResult<Option<Identity>> {
    let Some(id) = signature.object else {
        return Ok(None);
    };
    let object = doc.object(id)?;
    // Compressed dictionaries have no physical Contents token binding. Keep
    // their observations, but never claim complete source deduplication.
    let ObjectSource::File(range) = object.source() else {
        return Ok(None);
    };
    Ok(Some(Identity {
        object: id,
        offset: range.start,
        contents: signature
            .byte_range
            .as_ref()
            .map(|range| range.ranges()[0].end),
    }))
}
pub(super) fn discover<'a>(
    doc: &'a Document,
    ctx: &mut Context<'_>,
    result: &mut Signatures<'a>,
) -> ReadResult<Vec<Option<Facts>>> {
    ctx.allocate::<Option<Facts>>(doc.revisions().len(), Source::default())?;
    let mut facts = vec![Some(Facts::read(result, ctx)?)];
    result.history_complete = !doc.revisions().is_empty()
        && doc.diagnostics().is_empty()
        && doc.suppressed_diagnostics() == 0;
    let mut seen = BTreeSet::new();
    for signature in result
        .fields
        .iter()
        .filter_map(|f| f.signature.as_ref())
        .chain(result.usage_rights.iter())
    {
        ctx.allocate::<usize>(16, Source::default())?;
        if let Some(key) = key(doc, signature)? {
            seen.insert(key);
        } else {
            result.history_complete = false;
        }
    }
    for revision in 1..doc.revisions().len() {
        ctx.step(0, Source::default())?;
        let (snapshot, old) = match doc
            .revision_with_context(revision, ctx)
            .and_then(|snapshot| read::read(snapshot, ctx).map(|old| (snapshot, old)))
        {
            Ok(pair) => pair,
            Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Limit => {
                result.history_complete = false;
                facts.resize_with(doc.revisions().len(), || None);
                return Ok(facts);
            }
            Err(ReadFault::Unavailable(missing)) => return Err(ReadFault::Unavailable(missing)),
            Err(ReadFault::Invalid(_)) => {
                result.history_complete = false;
                facts.push(None);
                continue;
            }
        };
        facts.push(Some(Facts::read(&old, ctx)?));
        for (field_name, field_object, signature) in old
            .fields
            .into_iter()
            .filter_map(|f| f.signature.map(|s| (Some(f.qualified_name), f.object, s)))
            .chain(old.usage_rights.map(|s| (None, None, s)))
        {
            ctx.step(0, Source::default())?;
            ctx.allocate::<usize>(16, Source::default())?;
            if let Some(key) = key(snapshot, &signature)? {
                if !seen.insert(key) {
                    continue;
                }
            } else {
                result.history_complete = false;
            }
            ctx.allocate::<HistoricalSignature<'_>>(1, Source::default())?;
            if let Some(name) = &field_name {
                ctx.allocate::<u8>(name.len(), Source::default())?;
            }
            let mut signature = signature;
            rebase(doc, &mut signature, revision);
            result.historical.push(HistoricalSignature {
                revision,
                field_name,
                field_object,
                signature,
            });
        }
    }
    Ok(facts)
}

fn rebase<'a>(doc: &'a Document, signature: &mut Signature<'a>, revision: usize) {
    if let RevisionCoverage::Covered { index, .. } = signature.results.revision_coverage {
        let index = revision.saturating_add(index);
        if let Some(source) = doc.revisions().get(index) {
            signature.results.revision_coverage = RevisionCoverage::Covered {
                index,
                revision: source,
                later_revisions: &doc.revisions()[..index],
            };
        } else {
            signature.results.revision_coverage = RevisionCoverage::Indeterminate;
        }
    }
}
