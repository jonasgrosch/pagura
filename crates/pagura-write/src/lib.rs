#![forbid(unsafe_code)]
//! Deterministic serialization of new PDF documents (ISO 32000-2 clause 7.5).
//!
//! Output depends only on the inserted objects: object order is ascending
//! by number, dictionary entries keep their insertion order, and strings
//! are written in hexadecimal. Output intentionally targets PDF 1.7; it is not
//! a PDF 2.0 writer (7.5.2). Incremental saving is not implemented yet.

use pagura_core::{Error, Limits, Name, ObjectId, Result, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// An object to write: a value, plus data if it is a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct NewObject {
    /// The value; a stream's value must be its dictionary.
    pub value: Value,
    /// Exact stored stream bytes. If `/Filter` is supplied, the caller must
    /// encode these bytes accordingly (7.3.8.2); this serializer does not encode.
    /// `/Length` is written automatically from the stored byte count.
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

    /// A stream object whose data is already encoded if its dictionary has filters.
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
        if !is_catalog || catalog.1.stream.is_some() {
            return Err(Error::invalid_argument("Root must be a Catalog dictionary"));
        }
        let mut nodes = 0;
        for (&number, (generation, object)) in &self.objects {
            let id = ObjectId::new(number, *generation);
            self.validate(&object.value, 0, &mut nodes)
                .map_err(|error| error.in_object(id))?;
            if let Some(data) = &object.stream {
                if object.value.as_dictionary().is_none() {
                    return Err(
                        Error::invalid_argument("a stream's value must be a dictionary")
                            .in_object(id),
                    );
                }
                if data.len() > self.limits.stream_bytes {
                    return Err(Error::limit("writer stream budget exceeded").in_object(id));
                }
            }
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
            Value::Dictionary(dictionary) => dictionary.iter().try_for_each(|(key, value)| {
                self.validate_name(key)?;
                self.validate(value, depth + 1, nodes)
            }),
            _ => Ok(()),
        }
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
        let size = offsets
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::limit("writer cross-reference range is too large"))?;
        let xref = self.output.len();
        self.text(format_args!("xref\n0 {size}\n"));
        let mut next_free = (0..size)
            .filter(|n| !offsets.contains_key(n))
            .skip(1)
            .chain(std::iter::once(0));
        for number in 0..size {
            if let Some((offset, generation)) = offsets.get(&number) {
                if *offset as u128 > 9_999_999_999 {
                    return Err(Error::limit(
                        "classic cross-reference offsets require ten digits",
                    ));
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{Dictionary, ErrorKind};

    fn catalog() -> NewObject {
        let mut dictionary = Dictionary::new();
        dictionary.insert(Name::new(b"Type"), Value::name(b"Catalog"));
        NewObject::value(Value::Dictionary(dictionary))
    }
    fn builder(limits: Limits) -> Builder {
        let mut builder = Builder::new(limits);
        builder.insert(ObjectId::new(1, 0), catalog()).unwrap();
        builder
    }
    #[test]
    fn trailer_root_is_a_dictionary_not_a_stream() {
        let mut dictionary = Dictionary::new();
        dictionary.insert(Name::new(b"Type"), Value::name(b"Catalog"));
        let mut output = Builder::default();
        output
            .insert(ObjectId::new(1, 0), NewObject::stream(dictionary, vec![]))
            .unwrap();
        assert_eq!(
            output.finish(ObjectId::new(1, 0)).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
    }
    #[test]
    fn new_documents_require_zero_generations() {
        let mut output = Builder::default();
        for generation in [1, 7, u16::MAX] {
            let id = ObjectId::new(1, generation);
            let error = output.insert(id, catalog()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidArgument);
            assert_eq!(error.object, Some(id));
        }
    }
    #[test]
    fn names_exclude_nul_in_values_and_keys() {
        for value in [Value::name(b"bad\0name"), {
            let mut dictionary = Dictionary::new();
            dictionary.insert(Name::new(b"bad\0key"), Value::Null);
            Value::Dictionary(dictionary)
        }] {
            let mut output = builder(Limits::default());
            output
                .insert(ObjectId::new(2, 0), NewObject::value(value))
                .unwrap();
            let error = output.finish(ObjectId::new(1, 0)).unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidArgument);
            assert_eq!(error.object, Some(ObjectId::new(2, 0)));
        }
    }
    #[test]
    fn sparse_cross_reference_ranges_are_bounded_before_serializing() {
        for number in [u32::MAX, u32::MAX - 1, 4_000_000] {
            let mut output = builder(Limits::default());
            output
                .insert(ObjectId::new(number, 0), NewObject::value(Value::Null))
                .unwrap();
            assert_eq!(
                output.finish(ObjectId::new(1, 0)).unwrap_err().kind,
                ErrorKind::Limit
            );
        }
    }
    #[test]
    fn exact_serialized_values_have_standard_syntax() {
        // Expected bytes follow 7.3.2–7.3.9 directly, without a parser oracle.
        let mut writer = Writer {
            output: vec![],
            limit: 1024,
        };
        let mut dictionary = Dictionary::new();
        dictionary.insert(Name::new(b"A B#"), Value::String(vec![0, 0xff]));
        writer.value(&Value::Dictionary(dictionary));
        assert_eq!(writer.output, b"<< /A#20B#23 <00FF> >>");
        writer.output.clear();
        writer.value(&Value::Array(vec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Null,
            Value::Integer(-42),
            Value::Real(1.25),
            Value::name(b""),
        ]));
        assert_eq!(writer.output, b"[true false null -42 1.25 /]");
        for value in [1e-300, 1e300, -0.0] {
            writer.output.clear();
            writer.value(&Value::Real(value));
            assert!(writer.output.contains(&b'.'));
            assert!(!writer.output.contains(&b'e'));
            assert!(!writer.output.contains(&b'E'));
        }
    }
    #[test]
    fn xref_entries_free_chain_and_trailer_match_independent_offsets() {
        let mut output = builder(Limits::default());
        output
            .insert(ObjectId::new(3, 0), NewObject::value(Value::Null))
            .unwrap();
        let bytes = output.finish(ObjectId::new(1, 0)).unwrap();
        let prefix=b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<< /Type /Catalog >>\nendobj\n3 0 obj\nnull\nendobj\n";
        assert!(bytes.starts_with(prefix));
        let expected = format!(
            "xref\n0 4\n0000000002 65535 f \n0000000015 00000 n \n0000000000 00000 f \n{:010} 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            prefix
                .windows(7)
                .position(|bytes| bytes == b"3 0 obj")
                .unwrap(),
            prefix.len()
        );
        assert_eq!(&bytes[prefix.len()..], expected.as_bytes());
    }
    #[test]
    fn streams_use_stored_length_and_exclude_separator_eol() {
        let mut dictionary = Dictionary::new();
        dictionary.insert(Name::new(b"Filter"), Value::name(b"ASCIIHexDecode"));
        dictionary.insert(Name::new(b"Length"), Value::Integer(999));
        let mut writer = Writer {
            output: vec![],
            limit: 1024,
        };
        writer
            .object(
                ObjectId::new(2, 0),
                &NewObject::stream(dictionary, b"FF>".to_vec()),
            )
            .unwrap();
        assert_eq!(
            writer.output,
            b"2 0 obj\n<< /Filter /ASCIIHexDecode /Length 3 >>\nstream\nFF>\nendstream\nendobj\n"
        );
        let mut output = builder(Limits {
            stream_bytes: 2,
            ..Limits::default()
        });
        output
            .insert(
                ObjectId::new(2, 0),
                NewObject::stream(Dictionary::new(), vec![0; 3]),
            )
            .unwrap();
        assert_eq!(
            output.finish(ObjectId::new(1, 0)).unwrap_err().kind,
            ErrorKind::Limit
        );
    }
}
