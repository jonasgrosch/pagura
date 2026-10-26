//! Opening: from cross-reference data to an immutable [`Document`].
//!
//! Security and reconstruction need to read objects before the document is
//! complete. One owned construction retains scratch dependencies across waits,
//! then clears caches only after security or the object table changes. The
//! opened document is never changed, and it
//! publishes only objects parsed against its final object table, trailer and
//! security handler (ARCHITECTURE invariant 8).

mod loading;
pub(crate) use loading::Build;

use crate::source::ReadResult;
use crate::xref::rebuild::StreamSlots;
use crate::xref::{XrefOrigin, XrefTable};
use crate::{
    Document, Location, OpenOptions, Revision, Slot, Validation, diagnostics, history, memory,
    object, security, source,
};
use pagura_core::{
    Diagnostic, Dictionary, INDEX_ENTRY_OVERHEAD, Limits, Name, ObjectId, Recovery, Result, Value,
};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// A document under construction: its object table, trailer and security
/// handler, with no cached object.
pub(crate) struct DocumentBuilder {
    bytes: source::View,
    limits: Limits,
    recovery: Recovery,
    validation: Validation,
    trailer: Dictionary,
    slots: BTreeMap<ObjectId, Slot>,
    revisions: Vec<Revision>,
    object_streams: BTreeMap<u32, OnceLock<Result<object::stream::Contents>>>,
    security: Option<security::Security>,
    diagnostics: diagnostics::Log,
    retained: Arc<memory::Budget>,
    origin: XrefOrigin,
}

impl DocumentBuilder {
    pub(crate) fn new(
        bytes: source::View,
        options: &OpenOptions,
        table: XrefTable,
        reported: Vec<Diagnostic>,
        validation: Validation,
    ) -> Result<Self> {
        let retained = Arc::new(memory::Budget::new(options.limits.document_bytes));
        Self::assemble(
            bytes,
            options.limits,
            options.recovery,
            validation,
            table,
            reported,
            retained,
        )
    }

    /// A strict snapshot of an earlier revision of `source`, reading a prefix
    /// of its bytes and charging its cache to `source`'s budget too (7.5.6).
    pub(crate) fn revision(
        bytes: source::View,
        source: &Document,
        table: XrefTable,
    ) -> Result<Self> {
        let retained = Arc::new(memory::Budget::child(Arc::clone(&source.retained)));
        Self::assemble(
            bytes,
            source.limits,
            Recovery::Strict,
            source.validation,
            table,
            Vec::new(),
            retained,
        )
    }

    /// Charges the table's eager metadata, then builds the slot map from it.
    fn assemble(
        bytes: source::View,
        limits: Limits,
        recovery: Recovery,
        validation: Validation,
        table: XrefTable,
        reported: Vec<Diagnostic>,
        retained: Arc<memory::Budget>,
    ) -> Result<Self> {
        // A snapshot shares its source's revision entries, which the source
        // already charged. Structure belongs to no object.
        let source = retained.is_root();
        retained.charge(structure_bytes(&table, source), None)?;
        let object_streams: BTreeMap<_, _> = table
            .entries
            .values()
            .filter_map(|entry| match entry.location {
                Location::Compressed { stream, .. } => Some((stream, OnceLock::new())),
                _ => None,
            })
            .collect();
        retained.charge(
            object_streams
                .len()
                .saturating_mul(memory::stream_slot_bytes()),
            None,
        )?;
        let slots = table
            .entries
            .into_iter()
            .filter_map(|(number, entry)| match entry.location {
                Location::Free => None,
                location => Some((ObjectId::new(number, entry.generation), Slot::new(location))),
            })
            .collect();
        let mut diagnostics = diagnostics::Log::default();
        for diagnostic in reported {
            diagnostics.push(diagnostic);
        }
        Ok(Self {
            bytes,
            limits,
            recovery,
            validation,
            trailer: table.trailer,
            slots,
            revisions: table.revisions,
            object_streams,
            security: None,
            diagnostics,
            retained,
            origin: table.origin,
        })
    }

    /// Drives the same owned construction for complete-input callers.
    pub(crate) fn build(self, password: &[u8]) -> ReadResult<Document> {
        let mut build = Build::new(self);
        loop {
            if let Some(document) = build.advance(password)? {
                return Ok(document);
            }
        }
    }

    fn freeze(self) -> Document {
        Document {
            token: Arc::new(()),
            bytes: self.bytes,
            limits: self.limits,
            recovery: self.recovery,
            validation: self.validation,
            trailer: self.trailer,
            slots: self.slots,
            revisions: self.revisions,
            object_streams: self.object_streams,
            security: self.security,
            boundaries: OnceLock::new(),
            pages: OnceLock::new(),
            diagnostics: Mutex::new(self.diagnostics),
            retained: self.retained,
            historical: history::Cache::default(),
        }
    }
}

/// Estimated bytes of the table's eager metadata: object-table entries,
/// trailers and, for a source document, each revision's own entries. Per-node
/// tree bookkeeping is an estimate, consistent with the core dictionary index.
fn structure_bytes(table: &XrefTable, revision_entries: bool) -> usize {
    let structure = table
        .entries
        .values()
        .filter(|entry| !matches!(entry.location, Location::Free))
        .count()
        .saturating_mul(memory::slot_bytes())
        .saturating_add(table.trailer.retained_bytes())
        .saturating_add(
            table
                .revisions
                .capacity()
                .saturating_mul(size_of::<Revision>()),
        );
    table.revisions.iter().fold(structure, |sum, revision| {
        let entries = if revision_entries {
            revision.entries.len().saturating_mul(
                size_of::<(u32, xref::XrefEntry)>().saturating_add(INDEX_ENTRY_OVERHEAD),
            )
        } else {
            0
        };
        sum.saturating_add(
            revision
                .trailer()
                .retained_bytes()
                .saturating_sub(size_of::<Dictionary>()),
        )
        .saturating_add(entries)
    })
}

impl Document {
    /// Discards caches only when a completed construction stage changes their
    /// security/table authority. Ordinary missing-range waits keep dependencies.
    fn reset_construction_caches(&mut self, mark: usize) {
        for slot in self.slots.values_mut() {
            slot.object.take();
            *slot
                .waiting
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner) = None;
        }
        for contents in self.object_streams.values_mut() {
            contents.take();
        }
        self.boundaries.take();
        self.pages.take();
        self.historical = history::Cache::default();
        self.retained.rewind(mark);
    }

    /// Adds the compressed definitions and object streams that reconstruction
    /// established, replacing superseded physical slots.
    fn apply(&mut self, plan: StreamSlots) -> Result<()> {
        for stream in plan.streams {
            if let Entry::Vacant(entry) = self.object_streams.entry(stream) {
                self.retained
                    .charge(memory::stream_slot_bytes(), Some(ObjectId::new(stream, 0)))?;
                entry.insert(OnceLock::new());
            }
        }
        for (number, replacement) in plan.slots {
            let id = ObjectId::new(number, 0);
            match replacement.replaced {
                Some(old) => {
                    self.slots.remove(&old);
                }
                None => self.retained.charge(memory::slot_bytes(), Some(id))?,
            }
            self.slots.insert(id, Slot::new(replacement.location));
        }
        Ok(())
    }

    fn set_root(&mut self, root: ObjectId) -> Result<()> {
        let before = self.trailer.retained_bytes();
        self.trailer
            .insert(Name::new(b"Root"), Value::Reference(root));
        let after = self.trailer.retained_bytes();
        if after >= before {
            self.retained.charge(after - before, Some(root))
        } else {
            self.retained.release(before - after);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
