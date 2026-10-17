#![forbid(unsafe_code)]
//! Deterministic serialization of new PDF documents (ISO 32000-2 clause 7.5).
//!
//! Output depends only on the inserted objects: object order is ascending
//! by number, dictionary entries keep their insertion order, and strings
//! are written in hexadecimal. Incremental saving is not implemented yet.

use pagura_core::{Error, Limits, Name, ObjectId, Result, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// An object to write: a value, plus data if it is a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct NewObject {
    /// The value; a stream's value must be its dictionary.
    pub value: Value,
    /// Unencoded stream data. `/Length` is written automatically.
    pub stream: Option<Vec<u8>>,
}

impl NewObject {
    /// A non-stream object.
    pub fn value(value: Value) -> Self {
        Self {
            value,
            stream: None,
        }
    }

    /// A stream object.
    pub fn stream(dictionary: pagura_core::Dictionary, data: Vec<u8>) -> Self {
        Self {
            value: Value::Dictionary(dictionary),
            stream: Some(data),
        }
    }
}

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
    /// Returns `InvalidArgument` for object number 0 or a number already
    /// used, and a limit error beyond [`Limits::objects`].
    pub fn insert(&mut self, id: ObjectId, object: NewObject) -> Result<()> {
        if id.number == 0 || self.objects.contains_key(&id.number) {
            return Err(Error::invalid_argument(
                "object number is zero or already used",
            ));
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
    /// syntax errors for values PDF cannot represent, and limit errors for
    /// output beyond [`Limits::file_bytes`].
    pub fn finish(&self, root: ObjectId) -> Result<Vec<u8>> {
        let catalog = self
            .objects
            .get(&root.number)
            .filter(|(generation, _)| *generation == root.generation)
            .ok_or(Error::missing_object("Root object is missing"))?;
        let is_catalog = catalog
            .1
            .value
            .as_dictionary()
            .is_some_and(|dictionary| dictionary.is_name(b"Type", b"Catalog"));
        if !is_catalog {
            return Err(Error::invalid_argument("Root must be a Catalog dictionary"));
        }
        let mut nodes = 0;
        for (_, object) in self.objects.values() {
            self.validate(&object.value, 0, &mut nodes)?;
        }
        let mut writer = Writer {
            output: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(),
            limit: self.limits.file_bytes,
        };
        let mut offsets = BTreeMap::new();
        for (&number, (generation, object)) in &self.objects {
            let id = ObjectId::new(number, *generation);
            offsets.insert(number, (writer.output.len(), *generation));
            writer.object(id, object)?;
        }
        writer.xref_and_trailer(&offsets, root)?;
        Ok(writer.output)
    }

    fn validate(&self, value: &Value, depth: usize, nodes: &mut usize) -> Result<()> {
        if depth > self.limits.depth || *nodes >= self.limits.value_nodes {
            return Err(Error::limit("writer value budget exceeded"));
        }
        *nodes += 1;
        match value {
            Value::Real(value) if !value.is_finite() => {
                Err(Error::invalid_argument("PDF reals must be finite"))
            }
            Value::String(bytes) if bytes.len() > self.limits.string_bytes => {
                Err(Error::limit("writer string budget exceeded"))
            }
            Value::Name(name) if name.len() > self.limits.string_bytes => {
                Err(Error::limit("writer name budget exceeded"))
            }
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
            Value::Dictionary(dictionary) => dictionary
                .iter()
                .try_for_each(|(_, value)| self.validate(value, depth + 1, nodes)),
            _ => Ok(()),
        }
    }
}

struct Writer {
    output: Vec<u8>,
    limit: usize,
}

impl Writer {
    fn check(&self) -> Result<()> {
        if self.output.len() > self.limit {
            return Err(Error::limit("writer file budget exceeded"));
        }
        Ok(())
    }

    fn text(&mut self, text: std::fmt::Arguments<'_>) {
        // Writing to a String cannot fail.
        let mut buffer = String::new();
        let _ = buffer.write_fmt(text);
        self.output.extend_from_slice(buffer.as_bytes());
    }

    fn object(&mut self, id: ObjectId, object: &NewObject) -> Result<()> {
        self.text(format_args!("{id} obj\n"));
        match &object.stream {
            None => self.value(&object.value),
            Some(data) => {
                let Value::Dictionary(dictionary) = &object.value else {
                    return Err(Error::invalid_argument(
                        "a stream's value must be a dictionary",
                    ));
                };
                let mut dictionary = dictionary.clone();
                let length =
                    i64::try_from(data.len()).map_err(|_| Error::limit("stream too long"))?;
                dictionary.insert(Name::new(b"Length"), Value::Integer(length));
                self.value(&Value::Dictionary(dictionary));
                self.output.extend_from_slice(b"\nstream\n");
                self.output.extend_from_slice(data);
                self.output.extend_from_slice(b"\nendstream");
            }
        }
        self.output.extend_from_slice(b"\nendobj\n");
        self.check()
    }

    /// Writes a validated value.
    fn value(&mut self, value: &Value) {
        match value {
            Value::Null => self.output.extend_from_slice(b"null"),
            Value::Boolean(true) => self.output.extend_from_slice(b"true"),
            Value::Boolean(false) => self.output.extend_from_slice(b"false"),
            Value::Integer(value) => self.text(format_args!("{value}")),
            Value::Real(value) => {
                // `Display` for f64 never uses exponents; PDF forbids them.
                let mut text = value.to_string();
                if !text.contains('.') {
                    text.push_str(".0");
                }
                self.output.extend_from_slice(text.as_bytes());
            }
            Value::Name(name) => self.text(format_args!("{name}")),
            Value::String(bytes) => {
                self.output.push(b'<');
                for byte in bytes {
                    self.text(format_args!("{byte:02X}"));
                }
                self.output.push(b'>');
            }
            Value::Array(values) => {
                self.output.push(b'[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        self.output.push(b' ');
                    }
                    self.value(value);
                }
                self.output.push(b']');
            }
            Value::Dictionary(dictionary) => {
                self.output.extend_from_slice(b"<<");
                for (key, value) in dictionary.iter() {
                    self.text(format_args!(" {key} "));
                    self.value(value);
                }
                self.output.extend_from_slice(b" >>");
            }
            Value::Reference(id) => self.text(format_args!("{id} R")),
        }
    }

    /// Writes a single-section table whose free entries form a linked list
    /// starting at object 0 (clause 7.5.4).
    fn xref_and_trailer(
        &mut self,
        offsets: &BTreeMap<u32, (usize, u16)>,
        root: ObjectId,
    ) -> Result<()> {
        let size = offsets.keys().last().map_or(1, |&last| last + 1);
        let xref = self.output.len();
        self.text(format_args!("xref\n0 {size}\n"));
        let free: Vec<u32> = (0..size).filter(|n| !offsets.contains_key(n)).collect();
        let mut next_free = free.iter().skip(1).copied().chain(std::iter::once(0));
        for number in 0..size {
            if let Some((offset, generation)) = offsets.get(&number) {
                self.text(format_args!("{offset:010} {generation:05} n \n"));
            } else {
                let next = next_free.next().unwrap_or(0);
                let generation = if number == 0 { 65535 } else { 0 };
                self.text(format_args!("{next:010} {generation:05} f \n"));
            }
        }
        self.text(format_args!(
            "trailer\n<< /Size {size} /Root {root} R >>\nstartxref\n{xref}\n%%EOF\n"
        ));
        self.check()
    }
}
