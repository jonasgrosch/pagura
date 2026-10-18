//! Lazy object loading, dependency budgets and reference resolution (7.3.10).

use crate::xref::rebuild;
use crate::{Document, IndirectObject, Location, OpenOptions, Slot, object, security, xref};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Error, ObjectId, Result, Value};
use std::sync::{Arc, Mutex, OnceLock};

impl Document {
    pub(crate) fn from_table(
        bytes: Arc<[u8]>,
        options: &OpenOptions,
        table: xref::XrefTable,
        diagnostics: Vec<pagura_core::Diagnostic>,
    ) -> Result<Self> {
        Self::from_source(bytes.into(), options, table, diagnostics, None)
    }

    pub(crate) fn from_source(
        bytes: crate::history::Bytes,
        options: &OpenOptions,
        table: xref::XrefTable,
        diagnostics: Vec<pagura_core::Diagnostic>,
        parent: Option<&Document>,
    ) -> Result<Self> {
        let root = table
            .trailer
            .get(b"Root")
            .and_then(Value::as_reference)
            .or_else(|| {
                table
                    .entries
                    .iter()
                    .find(|(number, _)| **number != 0)
                    .map(|(&number, entry)| ObjectId::new(number, entry.generation))
            })
            .unwrap_or(ObjectId::new(0, 0));
        let retained = Arc::new(parent.map_or_else(
            || crate::memory::Budget::new(options.limits.document_bytes),
            |parent| crate::memory::Budget::child(parent.retained.clone()),
        ));
        retained.charge(structure_bytes(&table, parent.is_none()), root)?;
        let rebuilt = table.rebuild_remaining;
        let rebuild_streams = table.rebuild_streams;
        let object_streams: std::collections::BTreeMap<_, _> = table
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
                .saturating_mul(crate::memory::stream_slot_bytes()),
            root,
        )?;
        let slots = table
            .entries
            .into_iter()
            .filter_map(|(number, entry)| match entry.location {
                Location::Free => None,
                location => Some((
                    ObjectId::new(number, entry.generation),
                    Slot {
                        location,
                        object: OnceLock::new(),
                    },
                )),
            })
            .collect();
        let mut document = Self {
            bytes,
            options: options.clone(),
            trailer: table.trailer,
            slots,
            revisions: table.revisions,
            object_streams,
            security: None,
            pages: OnceLock::new(),
            diagnostics: Mutex::new(crate::diagnostics::Log::default()),
            retained,
            historical: crate::history::Cache::default(),
        };
        document.report(diagnostics);
        document.security = security::read(&document)?;
        document.options.password.clear();
        if let Some(remaining) = rebuilt {
            rebuild::load_streams(&mut document, rebuild_streams, remaining)?;
            match document.catalog() {
                Ok(catalog) if crate::catalog::candidate(&document, catalog)? => {}
                Err(error) if !xref::structural(&error) => return Err(error),
                _ => rebuild::catalog(&mut document)?,
            }
        }
        crate::catalog::validate(&document)?;
        Ok(document)
    }

    pub(crate) fn object_inner(
        &self,
        id: ObjectId,
        path: &mut Vec<ObjectId>,
    ) -> Result<&IndirectObject> {
        if path.contains(&id) {
            return Err(
                Error::new(pagura_core::ErrorKind::Cycle, "object loading cycle").in_object(id),
            );
        }
        if path.len() >= self.limits().depth {
            return Err(Error::limit("object loading depth exceeded").in_object(id));
        }
        let slot = self.slots.get(&id).ok_or_else(|| {
            Error::missing_object("object is not in use or its generation differs").in_object(id)
        })?;
        if let Some(object) = slot.object.get() {
            return object.as_ref().map_err(Clone::clone);
        }
        path.push(id);
        let result = match slot.location {
            Location::Offset(offset) => object::parse_at(self, id, offset, path),
            Location::Compressed { stream, index } => {
                object::stream::parse(self, id, stream, index, path)
            }
            Location::Free => Err(Error::missing_object("object is free").in_object(id)),
        };
        path.pop();
        let (mut object, diagnostics) = result.map_err(|error| error.in_object(id))?;
        if let Some(security) = &self.security {
            security.decrypt_object(&mut object)?;
        }
        slot.object
            .get_or_init(|| {
                self.retained.charge(object.retained_bytes(), id)?;
                self.report(diagnostics);
                Ok(object)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    pub(crate) fn resolve_inner<'a>(
        &'a self,
        value: &'a Value,
        path: &mut Vec<ObjectId>,
    ) -> Result<&'a Value> {
        static NULL: Value = Value::Null;
        let mut current = value;
        for _ in 0..=self.limits().depth {
            let Value::Reference(id) = current else {
                return Ok(current);
            };
            if !self.slots.contains_key(id) {
                return Ok(&NULL);
            }
            current = self.object_inner(*id, path)?.value();
        }
        Err(Error::limit("reference chain too long"))
    }

    /// Resolves `value` and the elements and entries one level below it,
    /// for small parameter structures such as `/DecodeParms`.
    pub(crate) fn resolve_deep(&self, value: &Value, path: &mut Vec<ObjectId>) -> Result<Value> {
        Ok(match self.resolve_inner(value, path)? {
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| self.resolve_deep_entry(value, path))
                    .collect::<Result<_>>()?,
            ),
            other => self.resolve_deep_entry(other, path)?,
        })
    }

    pub(crate) fn resolve_deep_entry(
        &self,
        value: &Value,
        path: &mut Vec<ObjectId>,
    ) -> Result<Value> {
        Ok(match self.resolve_inner(value, path)? {
            Value::Dictionary(dictionary) => Value::Dictionary(
                dictionary
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.resolve_inner(value, path)?.clone())))
                    .collect::<Result<_>>()?,
            ),
            other => other.clone(),
        })
    }

    /// A parser over the original file, configured with this document's
    /// limits and recovery mode.
    pub(crate) fn file_parser(&self, offset: usize) -> Result<Parser<'_>> {
        Ok(
            Parser::with_offset(&self.bytes, offset, Origin::File, self.limits())?
                .with_recovery(self.recovery()),
        )
    }
}

// Charge shared xref deltas once, at the source document. Snapshot slot maps
// and trailers remain separate retained allocations (ADR 0012).
fn structure_bytes(table: &xref::XrefTable, include_entries: bool) -> usize {
    let structure = table
        .entries
        .values()
        .filter(|entry| !matches!(entry.location, Location::Free))
        .count()
        .saturating_mul(crate::memory::slot_bytes())
        .saturating_add(table.trailer.retained_bytes())
        .saturating_add(
            table
                .revisions
                .capacity()
                .saturating_mul(size_of::<crate::Revision>()),
        );
    let structure = table.revisions.iter().fold(structure, |sum, revision| {
        sum.saturating_add(
            revision
                .trailer()
                .retained_bytes()
                .saturating_sub(size_of::<pagura_core::Dictionary>()),
        )
    });
    if include_entries {
        table.revisions.iter().fold(structure, |sum, revision| {
            sum.saturating_add(
                revision
                    .entries
                    .len()
                    .saturating_mul(size_of::<(u32, xref::XrefEntry)>() + 64),
            )
        })
    } else {
        structure
    }
}
