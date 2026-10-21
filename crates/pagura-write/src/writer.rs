//! PDF syntax for validated objects, the cross-reference table and the trailer.

use crate::NewObject;
use pagura_core::{Dictionary, Error, Meter, ObjectId, Result, Value, convert};
use std::fmt;

/// The version line and a binary comment of four high-bit bytes (7.5.2).
const HEADER: &[u8] = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n";

/// Bytes per classic cross-reference entry (7.5.4).
pub(crate) const XREF_ENTRY_BYTES: usize = 20;

/// The largest byte offset the ten digits of an entry hold (7.5.4).
const MAX_XREF_OFFSET: u64 = 9_999_999_999;

/// The generation of object 0, the head of the free list (7.5.4).
const FREE_HEAD_GENERATION: u16 = u16::MAX;

/// The `Limit` message of the output budget.
const FILE_BUDGET: &str = "writer file budget exceeded";

/// The stream dictionary key the writer fills in (7.3.8.2 Table 5).
pub(crate) const STREAM_LENGTH: &[u8] = b"Length";

/// Hexadecimal digits by value; strings are written in uppercase (7.3.4.3).
const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// String bytes hex-encoded per [`Writer::put`].
const HEX_CHUNK: usize = 512;

/// The output file. Every byte goes through [`Writer::put`], which charges
/// the file budget before the allocation it protects.
pub(crate) struct Writer {
    output: Vec<u8>,
    budget: Meter,
}

impl Writer {
    /// An empty file that may grow to `file_bytes`.
    pub(crate) const fn new(file_bytes: usize) -> Self {
        Self {
            output: Vec::new(),
            budget: Meter::new(file_bytes, FILE_BUDGET),
        }
    }

    /// The offset of the next byte written.
    pub(crate) const fn offset(&self) -> usize {
        self.output.len()
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.output
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.output
    }

    /// Appends `bytes` after charging them to the file budget. Capacity
    /// grows geometrically, but never beyond the budget.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error, leaving the output unchanged, if the file
    /// would exceed its budget or the allocation fails.
    pub(crate) fn put(&mut self, bytes: &[u8]) -> Result<()> {
        self.budget.charge(bytes.len())?;
        // The meter has charged exactly the bytes written, these included.
        let required = self.budget.used();
        if required > self.output.capacity() {
            let target = required
                .max(self.output.capacity().saturating_mul(2))
                .min(self.budget.max());
            self.output
                .try_reserve_exact(target - self.output.len())
                .map_err(|_| Error::limit(FILE_BUDGET))?;
        }
        self.output.extend_from_slice(bytes);
        Ok(())
    }

    /// Writes formatted text through [`Writer::put`].
    pub(crate) fn text(&mut self, arguments: fmt::Arguments<'_>) -> Result<()> {
        // Numbers, names and identities format without error, so formatting
        // fails only when `put` does, and `put` fails only on the budget.
        fmt::Write::write_fmt(self, arguments).map_err(|_| Error::limit(FILE_BUDGET))
    }

    pub(crate) fn header(&mut self) -> Result<()> {
        self.put(HEADER)
    }

    /// Writes an indirect object at the current offset, which its
    /// cross-reference entry must be able to hold.
    pub(crate) fn object(&mut self, id: ObjectId, object: &NewObject) -> Result<()> {
        if convert::usize_to_u64(self.offset()) > MAX_XREF_OFFSET {
            return Err(Error::limit(
                "classic cross-reference offsets require ten digits",
            ));
        }
        self.text(format_args!("{id} obj\n"))?;
        match object {
            NewObject::Value(value) => self.value(value)?,
            NewObject::Stream { dictionary, data } => {
                self.dictionary(dictionary, Some(data.len()))?;
                self.put(b"\nstream\n")?;
                self.put(data)?;
                self.put(b"\nendstream")?;
            }
        }
        self.put(b"\nendobj\n")
    }

    /// Writes a validated value.
    fn value(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Null => self.put(b"null"),
            Value::Boolean(true) => self.put(b"true"),
            Value::Boolean(false) => self.put(b"false"),
            Value::Integer(value) => self.text(format_args!("{value}")),
            Value::Real(value) => self.real(*value),
            Value::Name(name) => self.text(format_args!("{name}")),
            Value::String(bytes) => self.hex_string(bytes),
            Value::Array(values) => {
                self.put(b"[")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        self.put(b" ")?;
                    }
                    self.value(value)?;
                }
                self.put(b"]")
            }
            Value::Dictionary(dictionary) => self.dictionary(dictionary, None),
            Value::Reference(id) => self.text(format_args!("{id} R")),
        }
    }

    /// Writes a dictionary. A stream's `length` replaces the value of its
    /// `/Length` entry, or is appended if there is none.
    pub(crate) fn dictionary(
        &mut self,
        dictionary: &Dictionary,
        mut length: Option<usize>,
    ) -> Result<()> {
        self.put(b"<<")?;
        for (key, value) in dictionary {
            self.text(format_args!(" {key} "))?;
            if key.as_bytes() == STREAM_LENGTH
                && let Some(length) = length.take()
            {
                self.text(format_args!("{length}"))?;
            } else {
                self.value(value)?;
            }
        }
        if let Some(length) = length {
            self.text(format_args!(" /Length {length}"))?;
        }
        self.put(b" >>")
    }

    /// Writes a finite real. `Display` for `f64` never uses an exponent,
    /// which PDF forbids (7.3.3), but it omits the point of whole numbers.
    fn real(&mut self, value: f64) -> Result<()> {
        let start = self.offset();
        self.text(format_args!("{value}"))?;
        if self.output[start..].contains(&b'.') {
            Ok(())
        } else {
            self.put(b".0")
        }
    }

    /// Writes a byte string as two hexadecimal digits per byte (7.3.4.3).
    fn hex_string(&mut self, bytes: &[u8]) -> Result<()> {
        self.put(b"<")?;
        let mut digits = [0; 2 * HEX_CHUNK];
        for chunk in bytes.chunks(HEX_CHUNK) {
            for (pair, &byte) in digits.as_chunks_mut::<2>().0.iter_mut().zip(chunk) {
                *pair = [
                    HEX_DIGITS[usize::from(byte >> 4)],
                    HEX_DIGITS[usize::from(byte & 0x0F)],
                ];
            }
            self.put(&digits[..2 * chunk.len()])?;
        }
        self.put(b">")
    }

    /// Writes a single-section table whose free entries form a linked list
    /// starting at object 0 (7.5.4), then the trailer (7.5.5). `offsets`
    /// holds each object's number and offset in ascending order.
    pub(crate) fn xref_and_trailer(
        &mut self,
        offsets: &[(u32, usize)],
        size: u32,
        root: ObjectId,
    ) -> Result<()> {
        let xref = self.offset();
        self.text(format_args!("xref\n0 {size}\n"))?;
        let in_use = |number| {
            offsets
                .binary_search_by_key(&number, |&(used, _)| used)
                .is_ok()
        };
        // Each free entry names the next free number; the last names 0.
        let mut next_free = (1..size)
            .filter(|&number| !in_use(number))
            .chain(std::iter::once(0));
        let mut objects = offsets.iter().peekable();
        for number in 0..size {
            if let Some((_, offset)) = objects.next_if(|(used, _)| *used == number) {
                self.text(format_args!("{offset:010} 00000 n \n"))?;
            } else {
                let next = next_free.next().unwrap_or(0);
                let generation = if number == 0 { FREE_HEAD_GENERATION } else { 0 };
                self.text(format_args!("{next:010} {generation:05} f \n"))?;
            }
        }
        self.text(format_args!(
            "trailer\n<< /Size {size} /Root {root} R >>\nstartxref\n{xref}\n%%EOF\n"
        ))
    }
}

/// Lets `write!` format straight into the output, through [`Writer::put`].
impl fmt::Write for Writer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.put(text.as_bytes()).map_err(|_| fmt::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{ErrorKind, Name};

    fn written(value: &Value) -> Vec<u8> {
        let mut writer = Writer::new(1 << 20);
        writer.value(value).unwrap();
        writer.into_bytes()
    }

    #[test]
    fn exact_serialized_values_have_standard_syntax() {
        // Expected bytes follow 7.3.2–7.3.9 directly, without a parser oracle.
        let mut dictionary = Dictionary::new();
        dictionary.insert(Name::new(b"A B#"), Value::String(vec![0, 0xff]));
        assert_eq!(
            written(&Value::Dictionary(dictionary)),
            b"<< /A#20B#23 <00FF> >>"
        );
        let array = Value::Array(vec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Null,
            Value::Integer(-42),
            Value::Real(1.25),
            Value::name(b""),
        ]);
        assert_eq!(written(&array), b"[true false null -42 1.25 /]");
        assert_eq!(written(&Value::Real(3.0)), b"3.0");
        for value in [1e-300, 1e300, -0.0] {
            let output = written(&Value::Real(value));
            assert!(output.contains(&b'.'));
            assert!(!output.contains(&b'e'));
            assert!(!output.contains(&b'E'));
        }
    }

    #[test]
    fn strings_are_two_uppercase_digits_per_byte_across_chunks() {
        // Longer than two chunks, with every byte value at every chunk offset.
        let bytes: Vec<u8> = (0..=u8::MAX).cycle().take(2 * HEX_CHUNK + 7).collect();
        let expected = format!("<{}>", pagura_testkit::hex(&bytes).to_ascii_uppercase());
        assert_eq!(written(&Value::String(bytes)), expected.as_bytes());
        assert_eq!(written(&Value::String(vec![])), b"<>");
    }

    #[test]
    fn output_budget_is_charged_before_appending() {
        let mut writer = Writer::new(10);
        writer.put(b"0123456789").unwrap();
        let error = writer.put(b"x").unwrap_err();
        assert_eq!((error.kind, error.message), (ErrorKind::Limit, FILE_BUDGET));
        assert_eq!(writer.output, b"0123456789");
        assert!(writer.output.capacity() <= 10, "capacity beyond the budget");

        // A long string is refused before any of it is appended past the budget.
        let mut writer = Writer::new(3 * HEX_CHUNK);
        let error = writer
            .value(&Value::String(vec![0; 2 * HEX_CHUNK]))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(writer.offset() <= 3 * HEX_CHUNK);
        assert!(writer.output.capacity() <= 3 * HEX_CHUNK);
    }

    #[test]
    fn streams_use_stored_length_and_exclude_separator_eol() {
        let stream = |entries: &[(&[u8], Value)]| {
            let dictionary = entries
                .iter()
                .map(|(key, value)| (Name::new(key), value.clone()))
                .collect();
            let data = b"FF>".to_vec();
            let mut writer = Writer::new(1024);
            let object = NewObject::Stream { dictionary, data };
            writer.object(ObjectId::new(2, 0), &object).unwrap();
            String::from_utf8(writer.into_bytes()).unwrap()
        };
        let filter = (&b"Filter"[..], Value::name(b"ASCIIHexDecode"));
        let length = (&b"Length"[..], Value::Integer(999));
        assert_eq!(
            stream(&[filter.clone(), length.clone()]),
            "2 0 obj\n<< /Filter /ASCIIHexDecode /Length 3 >>\nstream\nFF>\nendstream\nendobj\n"
        );
        // A given /Length keeps its place; without one, it comes last.
        assert!(
            stream(&[length, filter.clone()]).contains("<< /Length 3 /Filter /ASCIIHexDecode >>")
        );
        assert!(stream(&[filter]).contains("<< /Filter /ASCIIHexDecode /Length 3 >>"));
    }
}
