//! Opening: from cross-reference data to an immutable [`Document`].
//!
//! Security and reconstruction need to read objects before the document is
//! complete. They read through scratch documents built from the builder's
//! object table; dissolving a scratch document discards its caches and their
//! retained-memory charges. The opened document is never changed, and it
//! publishes only objects parsed against its final object table, trailer and
//! security handler (ARCHITECTURE invariant 8).

use crate::xref::rebuild::{self, StreamSlots};
use crate::xref::{self, XrefOrigin, XrefTable};
use crate::{
    Document, Location, OpenOptions, Revision, Slot, catalog, diagnostics, history, memory, object,
    security,
};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, INDEX_ENTRY_OVERHEAD, Limits, Name, ObjectId, Recovery,
    Result, Value,
};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Opens `bytes`: from the primary cross-reference data, then, in repair
/// mode after a structural failure of that data or of the document built
/// from it, once from reconstructed data (ADR 0003).
pub(crate) fn open(bytes: Arc<[u8]>, options: &OpenOptions) -> Result<Document> {
    let mut diagnostics = Diagnostics::new(options.recovery);
    let header = xref::header(&bytes, &mut diagnostics)?;
    let mut attempt = Diagnostics::new(options.recovery);
    let primary = xref::read(&bytes, header, &options.limits, &mut attempt).and_then(|table| {
        let reported = diagnostics
            .as_slice()
            .iter()
            .cloned()
            .chain(attempt.take())
            .collect();
        DocumentBuilder::new(Arc::clone(&bytes), options, table, reported)?.build(&options.password)
    });
    match primary {
        Err(error) if options.recovery == Recovery::Repair && xref::structural(&error) => {
            let table = rebuild::read(&bytes, &options.limits, &mut diagnostics, error.position)?;
            DocumentBuilder::new(bytes, options, table, diagnostics.take())?
                .build(&options.password)
        }
        result => result,
    }
}

/// A document under construction: its object table, trailer and security
/// handler, with no cached object.
pub(crate) struct DocumentBuilder {
    bytes: history::Bytes,
    limits: Limits,
    recovery: Recovery,
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
    fn new(
        bytes: Arc<[u8]>,
        options: &OpenOptions,
        table: XrefTable,
        reported: Vec<Diagnostic>,
    ) -> Result<Self> {
        let retained = Arc::new(memory::Budget::new(options.limits.document_bytes));
        Self::assemble(
            bytes.into(),
            options.limits,
            options.recovery,
            table,
            reported,
            retained,
        )
    }

    /// A strict snapshot of an earlier revision of `source`, reading a prefix
    /// of its bytes and charging its cache to `source`'s budget too (7.5.6).
    pub(crate) fn revision(
        bytes: history::Bytes,
        source: &Document,
        table: XrefTable,
    ) -> Result<Self> {
        let retained = Arc::new(memory::Budget::child(Arc::clone(&source.retained)));
        Self::assemble(
            bytes,
            source.limits,
            Recovery::Strict,
            table,
            Vec::new(),
            retained,
        )
    }

    /// Charges the table's eager metadata, then builds the slot map from it.
    fn assemble(
        bytes: history::Bytes,
        limits: Limits,
        recovery: Recovery,
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

    /// Authenticates, reads reconstructed object streams and catalog, then
    /// freezes the document and validates its catalog.
    pub(crate) fn build(mut self, password: &[u8]) -> Result<Document> {
        let origin = std::mem::take(&mut self.origin);
        if self
            .trailer
            .get(b"Encrypt")
            .is_some_and(|value| !value.is_null())
        {
            let (builder, security) =
                self.with_scratch(|document| security::read(document, password));
            self = builder;
            self.security = security?;
        }
        if let XrefOrigin::Rebuilt { streams, remaining } = origin {
            let (builder, slots) =
                self.with_scratch(|document| rebuild::object_streams(document, streams, remaining));
            self = builder;
            self.apply(slots?)?;
            let (builder, root) = self.with_scratch(rebuild::root);
            self = builder;
            if let Some(root) = root? {
                self.set_root(root)?;
            }
        }
        let document = self.freeze();
        catalog::validate(&document)?;
        Ok(document)
    }

    /// Reads through a scratch document, then dissolves it with its caches.
    fn with_scratch<T>(self, read: impl FnOnce(&Document) -> T) -> (Self, T) {
        let mark = self.retained.used();
        let document = self.freeze();
        let result = read(&document);
        (document.thaw(mark), result)
    }

    fn freeze(self) -> Document {
        Document {
            token: Arc::new(()),
            bytes: self.bytes,
            limits: self.limits,
            recovery: self.recovery,
            trailer: self.trailer,
            slots: self.slots,
            revisions: self.revisions,
            object_streams: self.object_streams,
            security: self.security,
            repaired: self.diagnostics.has_repairs(),
            boundaries: OnceLock::new(),
            pages: OnceLock::new(),
            diagnostics: Mutex::new(self.diagnostics),
            retained: self.retained,
            historical: history::Cache::default(),
        }
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
    /// Dissolves a scratch document: its caches go, and the budget returns
    /// to `mark`, its use before the document was frozen.
    fn thaw(self, mark: usize) -> DocumentBuilder {
        let Self {
            bytes,
            limits,
            recovery,
            trailer,
            mut slots,
            revisions,
            mut object_streams,
            security,
            diagnostics,
            retained,
            ..
        } = self;
        for slot in slots.values_mut() {
            slot.object.take();
        }
        for contents in object_streams.values_mut() {
            contents.take();
        }
        retained.rewind(mark);
        DocumentBuilder {
            bytes,
            limits,
            recovery,
            trailer,
            slots,
            revisions,
            object_streams,
            security,
            diagnostics: diagnostics
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
            retained,
            origin: XrefOrigin::Primary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DocumentBuilder;
    use crate::xref::{XrefEntry, XrefTable};
    use crate::{Location, OpenOptions, memory};
    use pagura_core::{ErrorKind, Limits, Name, ObjectId, Value};
    use std::sync::Arc;

    #[test]
    fn structure_charges_belong_to_no_object() {
        let mut table = XrefTable::default();
        table.entries.insert(
            1,
            XrefEntry {
                generation: 0,
                location: Location::Offset(9),
            },
        );
        table
            .trailer
            .insert(Name::new(b"Root"), Value::Reference(ObjectId::new(1, 0)));
        let options = OpenOptions {
            limits: Limits {
                document_bytes: memory::slot_bytes(),
                ..Limits::default()
            },
            ..OpenOptions::default()
        };
        let bytes: Arc<[u8]> = Arc::from(b"%PDF-1.7\n".as_slice());
        let Err(error) = DocumentBuilder::new(bytes, &options, table, Vec::new()) else {
            panic!("the trailer does not fit beside one slot");
        };
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.object, None);
    }
}
