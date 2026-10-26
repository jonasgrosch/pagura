//! Lazy strict revision snapshots sharing source storage and cache accounting.
use crate::builder::DocumentBuilder;
use crate::features::{HISTORICAL_ENCRYPTION, HISTORICAL_SOURCE};
use crate::{
    Document, IndirectObject,
    view::{Context, Source},
};
use pagura_core::{Error, ErrorKind, ObjectId, Position, Result};
use std::sync::{Mutex, OnceLock, PoisonError};

#[derive(Debug, Default)]
pub(crate) struct Cache {
    snapshots: OnceLock<Box<[Snapshot]>>,
    loading: Mutex<()>,
}

#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    value: OnceLock<Result<Document>>,
    loading: Mutex<()>,
}

impl Document {
    /// Loads an object as it existed in a source revision (7.5.6).
    /// `revision` indexes [`Self::revisions`], newest first. Identity includes
    /// generation; freed objects are absent. Historical objects and their stream
    /// dependencies use that revision's xref data and strict source prefix.
    /// Snapshots share original bytes and the aggregate retained-cache budget.
    /// Use the same revision index for referenced identities; the ordinary
    /// resolution and stream-decoding methods retain their current-revision context.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for an unknown revision, `MissingObject` for an
    /// absent identity, and parsing/cache-limit errors. Historical access to
    /// earlier encrypted or repaired revisions is unsupported; no newer object is used
    /// as a fallback for an inaccessible earlier revision.
    pub fn object_at_revision(&self, revision: usize, id: ObjectId) -> Result<&IndirectObject> {
        self.revision_document(revision)?.object(id)
    }

    pub(crate) fn revision_document(&self, index: usize) -> Result<&Document> {
        self.revision_with_context(index, &mut Context::new(self))
    }

    pub(crate) fn revision_with_context(
        &self,
        index: usize,
        ctx: &mut Context<'_>,
    ) -> Result<&Document> {
        self.bytes.check()?;
        if index >= self.revisions.len() {
            return Err(Error::invalid_argument("revision index outside document"));
        }
        if index == 0 {
            return Ok(self);
        }
        if self.security.is_some()
            || !self.diagnostics().is_empty()
            || self.suppressed_diagnostics() != 0
        {
            return Err(Error::unsupported(
                HISTORICAL_SOURCE,
                "historical access requires unrepaired unencrypted source",
            ));
        }
        let store = &self.historical;
        if store.snapshots.get().is_none() {
            let _loading = store.loading.lock().unwrap_or_else(PoisonError::into_inner);
            if store.snapshots.get().is_none() {
                let bytes = self
                    .revisions
                    .len()
                    .checked_mul(size_of::<Snapshot>())
                    .ok_or_else(|| Error::limit("revision cache metadata overflow"))?;
                self.retained.charge(bytes, None)?;
                store.snapshots.get_or_init(|| {
                    (0..self.revisions.len())
                        .map(|_| Snapshot::default())
                        .collect::<Vec<_>>()
                        .into_boxed_slice()
                });
            }
        }
        let cache = store
            .snapshots
            .get()
            .ok_or_else(|| Error::syntax("revision cache is uninitialized"))?;
        // Extend the oldest confirmed prefix once per revision. Xref entries
        // were already parsed on open; never reparse a prefix's Prev chain.
        for next in (index..self.revisions.len()).rev() {
            let entry = &cache[next];
            if entry.value.get().is_none() {
                // Serialize publication, but never retain a transient traversal
                // limit. A fresh analysis may have more work remaining.
                let _loading = entry.loading.lock().unwrap_or_else(PoisonError::into_inner);
                if entry.value.get().is_none() {
                    let previous = cache.get(next + 1).and_then(|entry| entry.value.get());
                    let parsed = self.open_revision(
                        next,
                        previous
                            .map(|r| r.as_ref().map_err(Clone::clone))
                            .transpose()?,
                        ctx,
                    );
                    if let Err(error) = &parsed
                        && error.kind == ErrorKind::Limit
                    {
                        return Err(error.clone());
                    }
                    entry.value.get_or_init(|| parsed);
                }
            }
            entry
                .value
                .get()
                .ok_or_else(|| Error::syntax("revision cache is uninitialized"))?
                .as_ref()
                .map_err(Clone::clone)?;
        }
        cache[index]
            .value
            .get()
            .ok_or_else(|| Error::syntax("revision cache is uninitialized"))?
            .as_ref()
            .map_err(Clone::clone)
    }

    fn open_revision(
        &self,
        index: usize,
        previous: Option<&Document>,
        ctx: &mut Context<'_>,
    ) -> Result<Document> {
        let revision = &self.revisions[index];
        let source = Source {
            object: None,
            position: Position::File(revision.offset()),
        };
        let upper = self.revisions[index - 1].offset();
        let (boundary, consumed) = crate::revision_bounds::boundary(ctx, revision, upper, source)?;
        ctx.scan(consumed, source)?;
        let bytes = self.bytes.prefix(boundary.eol)?;
        let mut table = crate::xref::XrefTable::default();
        if let Some(previous) = previous {
            ctx.allocate::<usize>(previous.slots.len().saturating_mul(12), source)?;
            for (id, slot) in &previous.slots {
                ctx.step(0, source)?;
                table.entries.insert(
                    id.number,
                    crate::xref::XrefEntry {
                        generation: id.generation,
                        location: slot.location,
                    },
                );
            }
            ctx.allocate::<u8>(previous.trailer.retained_bytes(), source)?;
            table.trailer = previous.trailer.clone();
        }
        ctx.allocate::<usize>(revision.entries.len().saturating_mul(12), source)?;
        for (&number, &entry) in revision.entries.iter() {
            ctx.step(0, source)?;
            if matches!(entry.location, crate::Location::Offset(at) if at >= boundary.eol) {
                return Err(
                    source.error(Error::syntax("historical xref points outside its revision"))
                );
            }
            table.entries.insert(number, entry);
        }
        for (key, value) in &revision.trailer {
            ctx.step(0, source)?;
            ctx.allocate::<u8>(
                value.retained_bytes().saturating_add(key.as_bytes().len()),
                source,
            )?;
            table.trailer.insert(key.clone(), value.clone());
        }
        ctx.allocate::<crate::Revision>(self.revisions.len() - index, source)?;
        for earlier in &self.revisions[index..] {
            ctx.step(0, source)?;
            ctx.allocate::<u8>(earlier.trailer.retained_bytes(), source)?;
        }
        table.revisions = self.revisions[index..].to_vec();
        if table.trailer.get(b"Encrypt").is_some_and(|v| !v.is_null()) {
            return Err(source.error(Error::unsupported(
                HISTORICAL_ENCRYPTION,
                "encrypted historical revision is unsupported",
            )));
        }
        DocumentBuilder::revision(bytes, self, table)?.build(&[])
    }
}
