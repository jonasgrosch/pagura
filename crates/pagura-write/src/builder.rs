//! Collecting objects and validating them before serialization.

use crate::NewObject;
use crate::validation;
use crate::writer::{Writer, XREF_ENTRY_BYTES};
use pagura_core::{Error, Limits, ObjectId, Result, ResultExt, Value, convert};
use std::collections::BTreeMap;

/// The `Limit` message of the cross-reference table's size.
const XREF_BUDGET: &str = "writer cross-reference budget exceeded";

/// Collects objects and writes them as one new document.
///
/// Every object of a new document has generation 0 (7.3.10), so objects are
/// kept by number alone.
#[derive(Debug, Default)]
pub struct Builder {
    limits: Limits,
    objects: BTreeMap<u32, NewObject>,
}

impl Builder {
    /// Creates an empty builder with the given budgets.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            objects: BTreeMap::new(),
        }
    }

    /// Adds an object.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for object number 0, a duplicate number or a
    /// nonzero generation (new files require generation 0, clause 7.3.10),
    /// and a limit error beyond [`Limits::objects`].
    pub fn insert(&mut self, id: ObjectId, object: NewObject) -> Result<()> {
        if id.number == 0 || self.objects.contains_key(&id.number) {
            return Err(
                Error::invalid_argument("object number is zero or already used").in_object(id),
            );
        }
        if id.generation != 0 {
            return Err(
                Error::invalid_argument("new document objects require generation zero")
                    .in_object(id),
            );
        }
        if self.objects.len() >= self.limits.objects {
            return Err(Error::limit("writer object budget exceeded").in_object(id));
        }
        self.objects.insert(id.number, object);
        Ok(())
    }

    /// Serializes all objects with `root` as the catalog.
    ///
    /// # Errors
    ///
    /// Returns `MissingObject` for a missing root or dangling reference,
    /// `InvalidArgument` for values PDF cannot represent or an invalid catalog,
    /// and `Limit` for output, stream or cross-reference budgets.
    pub fn finish(&self, root: ObjectId) -> Result<Vec<u8>> {
        let catalog = self
            .get(root)
            .ok_or(Error::missing_object("root object is missing").in_object(root))?;
        let is_catalog = matches!(
            catalog,
            NewObject::Value(Value::Dictionary(dictionary))
                if dictionary.is_name(b"Type", b"Catalog")
        );
        if !is_catalog {
            return Err(
                Error::invalid_argument("root object must be a catalog dictionary").in_object(root),
            );
        }
        for (&number, object) in &self.objects {
            validation::object(object, &self.limits, &|id| self.get(id).is_some())
                .in_object(ObjectId::new(number, 0))?;
        }
        let size = self.xref_size()?;
        let mut offsets = Vec::new();
        offsets
            .try_reserve_exact(self.objects.len())
            .map_err(|_| Error::limit(XREF_BUDGET))?;
        let mut writer = Writer::new(self.limits.file_bytes);
        writer.header()?;
        for (&number, object) in &self.objects {
            let id = ObjectId::new(number, 0);
            offsets.push((number, writer.offset()));
            writer.object(id, object).in_object(id)?;
        }
        writer.xref_and_trailer(&offsets, size, root)?;
        Ok(writer.into_bytes())
    }

    fn get(&self, id: ObjectId) -> Option<&NewObject> {
        self.objects.get(&id.number).filter(|_| id.generation == 0)
    }

    /// The cross-reference table's `/Size`, one above the largest object
    /// number (7.5.5 Table 15). Every number below it needs an entry, used or
    /// not (7.5.4), so the range is bounded before anything is written.
    fn xref_size(&self) -> Result<u32> {
        let size = self
            .objects
            .last_key_value()
            .map_or(0, |(&number, _)| number)
            .checked_add(1)
            .ok_or(Error::limit(XREF_BUDGET))?;
        let entries = convert::u32_to_usize(size);
        if entries > self.limits.objects
            || entries
                .checked_mul(XREF_ENTRY_BYTES)
                .is_none_or(|bytes| bytes > self.limits.file_bytes)
        {
            return Err(Error::limit(XREF_BUDGET));
        }
        Ok(size)
    }
}
