//! Collecting objects and validating them before serialization.

use crate::NewObject;
use crate::writer::{STREAM_LENGTH, Writer};
use pagura_core::{Error, Limits, Meter, Name, ObjectId, Result, ResultExt, Value};
use std::collections::BTreeMap;

/// The `Limit` message of the per-object value budget.
const VALUE_BUDGET: &str = "writer value budget exceeded";

/// Collects objects and writes them as one new document.
#[derive(Debug, Default)]
pub struct Builder {
    limits: Limits,
    objects: BTreeMap<u32, (u16, NewObject)>,
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
            return Err(Error::invalid_argument(
                "object number is zero or already used",
            ));
        }
        if id.generation != 0 {
            return Err(
                Error::invalid_argument("new document objects require generation zero")
                    .in_object(id),
            );
        }
        if self.objects.len() >= self.limits.objects {
            return Err(Error::limit("writer object budget exceeded"));
        }
        self.objects.insert(id.number, (id.generation, object));
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
        let (_, catalog) = self
            .objects
            .get(&root.number)
            .filter(|(generation, _)| *generation == root.generation)
            .ok_or(Error::missing_object("Root object is missing"))?;
        let is_catalog = matches!(
            catalog,
            NewObject::Value(Value::Dictionary(dictionary))
                if dictionary.is_name(b"Type", b"Catalog")
        );
        if !is_catalog {
            return Err(Error::invalid_argument("Root must be a Catalog dictionary"));
        }
        for (&number, (generation, object)) in &self.objects {
            let id = ObjectId::new(number, *generation);
            self.validate_object(object).in_object(id)?;
        }
        // Every sparse number still needs a 20-byte xref entry (7.5.4).
        // Bound the range before allocating or iterating over it.
        let size = self
            .objects
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::limit("writer cross-reference range is too large"))?;
        let size = usize::try_from(size)
            .map_err(|_| Error::limit("writer cross-reference range is too large"))?;
        if size > self.limits.objects
            || size
                .checked_mul(20)
                .is_none_or(|bytes| bytes > self.limits.file_bytes)
        {
            return Err(Error::limit("writer cross-reference budget exceeded"));
        }
        let mut writer = Writer::new(self.limits.file_bytes);
        writer.header()?;
        let mut offsets = BTreeMap::new();
        for (&number, (generation, object)) in &self.objects {
            let id = ObjectId::new(number, *generation);
            offsets.insert(number, (writer.offset(), *generation));
            writer.object(id, object).in_object(id)?;
        }
        writer.xref_and_trailer(&offsets, root)?;
        Ok(writer.into_bytes())
    }

    /// Checks what the writer will write for `object`, with the budgets the
    /// parser applies when it reads the object back.
    fn validate_object(&self, object: &NewObject) -> Result<()> {
        let mut nodes = Meter::new(self.limits.value_nodes, VALUE_BUDGET);
        match object {
            NewObject::Value(value) => self.validate(value, 0, &mut nodes),
            NewObject::Stream { dictionary, data } => {
                if data.len() > self.limits.stream_bytes {
                    return Err(Error::limit("writer stream budget exceeded"));
                }
                // The dictionary, and the `/Length` written in place of any given.
                nodes.charge(2)?;
                let entries = dictionary
                    .into_iter()
                    .filter(|(key, _)| key.as_bytes() != STREAM_LENGTH);
                self.validate_entries(entries, 1, &mut nodes)
            }
        }
    }

    fn validate(&self, value: &Value, depth: usize, nodes: &mut Meter) -> Result<()> {
        if depth > self.limits.depth {
            return Err(Error::limit("writer nesting depth exceeded"));
        }
        nodes.charge(1)?;
        match value {
            Value::Real(value) if !value.is_finite() => {
                Err(Error::invalid_argument("PDF reals must be finite"))
            }
            Value::String(bytes) if bytes.len() > self.limits.string_bytes => {
                Err(Error::limit("writer string budget exceeded"))
            }
            Value::Name(name) => self.validate_name(name),
            Value::Reference(id)
                if self
                    .objects
                    .get(&id.number)
                    .map(|(generation, _)| *generation)
                    != Some(id.generation) =>
            {
                Err(Error::missing_object("dangling object reference"))
            }
            Value::Array(values) => values
                .iter()
                .try_for_each(|value| self.validate(value, depth + 1, nodes)),
            Value::Dictionary(dictionary) => self.validate_entries(dictionary, depth + 1, nodes),
            _ => Ok(()),
        }
    }

    fn validate_entries<'a>(
        &self,
        entries: impl IntoIterator<Item = (&'a Name, &'a Value)>,
        depth: usize,
        nodes: &mut Meter,
    ) -> Result<()> {
        entries.into_iter().try_for_each(|(key, value)| {
            self.validate_name(key)?;
            self.validate(value, depth, nodes)
        })
    }

    fn validate_name(&self, name: &Name) -> Result<()> {
        if name.len() > self.limits.string_bytes {
            return Err(Error::limit("writer name budget exceeded"));
        }
        if name.as_bytes().contains(&0) {
            return Err(Error::invalid_argument("PDF names cannot contain NUL"));
        }
        Ok(())
    }
}
