//! PDF syntax for validated objects, the cross-reference table and the trailer.

use crate::NewObject;
use pagura_core::{Error, Name, ObjectId, Result, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;

pub(crate) struct Writer {
    pub(crate) output: Vec<u8>,
    pub(crate) limit: usize,
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

    pub(crate) fn object(&mut self, id: ObjectId, object: &NewObject) -> Result<()> {
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
                for (key, value) in dictionary {
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
    pub(crate) fn xref_and_trailer(
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
    use pagura_core::Dictionary;

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
    }
}
