//! Lazy object loading, dependency budgets and reference resolution (7.3.10).

use crate::{
    Document, IndirectObject, Location, OpenOptions, Slot, object, object_stream, rebuild,
    security, xref,
};
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
        let rebuilt = table.rebuild_remaining;
        let rebuild_streams = table.rebuild_streams;
        let object_streams = table
            .entries
            .values()
            .filter_map(|entry| match entry.location {
                Location::Compressed { stream, .. } => Some((stream, OnceLock::new())),
                _ => None,
            })
            .collect();
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
            open_diagnostics: diagnostics,
            lazy_diagnostics: Mutex::new(Vec::new()),
        };
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
            return Ok(object);
        }
        path.push(id);
        let result = match slot.location {
            Location::Offset(offset) => object::parse_at(self, id, offset, path),
            Location::Compressed { stream, index } => {
                object_stream::parse(self, id, stream, index, path)
            }
            Location::Free => Err(Error::missing_object("object is free").in_object(id)),
        };
        path.pop();
        let (mut object, diagnostics) = result.map_err(|error| error.in_object(id))?;
        if let Some(security) = &self.security {
            security.decrypt_object(&mut object)?;
        }
        Ok(slot.object.get_or_init(|| {
            self.record(diagnostics);
            object
        }))
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
            Parser::at(&self.bytes, offset, Origin::File, self.limits())?
                .with_recovery(self.recovery()),
        )
    }
}
