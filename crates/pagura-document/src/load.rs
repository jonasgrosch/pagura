//! Lazy object loading, dependency budgets and reference resolution (7.3.10).

use crate::{Document, IndirectObject, Location, object};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Error, ObjectId, Result, Value};

impl Document {
    /// Loads `id` while the objects in `path` are being loaded.
    ///
    /// A load that starts with an empty `path` depends only on the document,
    /// so its result is cached even when it fails: recovery such as offset
    /// correction or terminator scans then runs at most once per object.
    /// Nested failures can depend on `path` (cycles, depth) and are not cached.
    pub(crate) fn object_inner(
        &self,
        id: ObjectId,
        path: &mut Vec<ObjectId>,
    ) -> Result<&IndirectObject> {
        self.bytes.check()?;
        if path.contains(&id) {
            return Err(Error::cycle("object loading cycle").in_object(id));
        }
        if path.len() >= self.limits.depth {
            return Err(Error::limit("object loading depth exceeded").in_object(id));
        }
        let slot = self.slots.get(&id).ok_or_else(|| {
            Error::missing_object("object is not in use or its generation differs").in_object(id)
        })?;
        if let Some(object) = slot.object.get() {
            return object.as_ref().map_err(Clone::clone);
        }
        let top_level = path.is_empty();
        path.push(id);
        let parsed = match slot.location {
            Location::Offset(offset) => object::parse_at(self, id, offset, path),
            Location::Compressed { stream, index } => {
                object::stream::parse(self, id, stream, index, path)
            }
            Location::Free => Err(Error::missing_object("object is free")),
        };
        path.pop();
        let loaded = parsed
            .and_then(|(mut object, diagnostics)| {
                if let Some(security) = &self.security {
                    security.decrypt_object(&mut object)?;
                }
                Ok((object, diagnostics))
            })
            .map_err(|error| error.in_object(id));
        let loaded = match loaded {
            Err(error) if !top_level => return Err(error),
            loaded => loaded,
        };
        slot.object
            .get_or_init(|| {
                let (object, diagnostics) = loaded?;
                self.retained.charge(object.retained_bytes(), Some(id))?;
                self.report(diagnostics);
                Ok(object)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Follows references from `value` to a direct value, also returning the
    /// object that holds it, if it is not `value` itself.
    pub(crate) fn resolve_inner<'a>(
        &'a self,
        value: &'a Value,
        path: &mut Vec<ObjectId>,
    ) -> Result<(&'a Value, Option<ObjectId>)> {
        static NULL: Value = Value::Null;
        self.bytes.check()?;
        let mut current = value;
        let mut origin = None;
        for _ in 0..=self.limits.depth {
            let Value::Reference(id) = current else {
                return Ok((current, origin));
            };
            if !self.slots.contains_key(id) {
                return Ok((&NULL, origin));
            }
            current = self.object_inner(*id, path)?.value();
            origin = Some(*id);
        }
        Err(Error::limit("reference chain too long").in_object_opt(origin))
    }

    /// Resolves `value` and the elements and entries one level below it,
    /// for small parameter structures such as `/DecodeParms`.
    pub(crate) fn resolve_deep(&self, value: &Value, path: &mut Vec<ObjectId>) -> Result<Value> {
        Ok(match self.resolve_inner(value, path)?.0 {
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| self.resolve_deep_entry(value, path))
                    .collect::<Result<_>>()?,
            ),
            other => self.resolve_deep_entry(other, path)?,
        })
    }

    fn resolve_deep_entry(&self, value: &Value, path: &mut Vec<ObjectId>) -> Result<Value> {
        Ok(match self.resolve_inner(value, path)?.0 {
            Value::Dictionary(dictionary) => Value::Dictionary(
                dictionary
                    .iter()
                    .map(|(key, value)| {
                        Ok((key.clone(), self.resolve_inner(value, path)?.0.clone()))
                    })
                    .collect::<Result<_>>()?,
            ),
            other => other.clone(),
        })
    }

    /// A parser over the original file, configured with this document's
    /// limits and recovery mode.
    pub(crate) fn file_parser(&self, offset: usize) -> Result<Parser<'_>> {
        self.bytes.check()?;
        Ok(
            Parser::with_offset(&self.bytes, offset, Origin::File, self.limits)?
                .with_recovery(self.recovery),
        )
    }
}
