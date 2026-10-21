//! Structured inputs and the small, well-formed PDF files wrapped around
//! them, so that a target spends its executions on the part under test
//! instead of on file structure.

use pagura_core::{Dictionary, Limits, Value, syntax::Origin, syntax::Parser};

/// Separates the parts of a structured input: a line holding only `%%%%`,
/// which reads as a PDF comment wherever it lands.
pub const SEPARATOR: &[u8] = b"\n%%%%\n";

/// The most objects a wrapper takes from one input.
pub const MAX_OBJECTS: usize = 32;

/// The first object number taken from the input; 1 is the catalog and 2 the
/// page tree, as in the generated fixtures.
pub const FIRST_OBJECT: u32 = 3;

/// Splits `data` at every [`SEPARATOR`]; there is always at least one part.
pub fn parts(data: &[u8]) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut rest = data;
    while let Some(at) = find(rest, SEPARATOR) {
        let (part, tail) = rest.split_at(at);
        parts.push(part);
        rest = tail.get(SEPARATOR.len()..).unwrap_or_default();
    }
    parts.push(rest);
    parts
}

/// The offset of the first `needle` in `haystack`.
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parses a direct dictionary such as an inline image dictionary.
pub fn dictionary(data: &[u8], limits: Limits) -> Option<Dictionary> {
    match super::checked(Parser::new(data, Origin::Buffer, limits).parse())? {
        Value::Dictionary(dictionary) => Some(dictionary),
        _ => None,
    }
}

/// A classic-xref PDF built object by object (ISO 32000-2 clause 7.5).
#[derive(Debug)]
pub struct Pdf {
    data: Vec<u8>,
    offsets: Vec<usize>,
}

impl Default for Pdf {
    fn default() -> Self {
        Self::new()
    }
}

impl Pdf {
    /// A header and no objects yet.
    pub fn new() -> Self {
        Self {
            data: b"%PDF-2.0\n%\xe2\xe3\xcf\xd3\n".to_vec(),
            offsets: Vec::new(),
        }
    }

    /// A catalog (object 1) over an empty page tree (object 2).
    pub fn without_pages() -> Self {
        let mut pdf = Self::new();
        pdf.object(b"<< /Type /Catalog /Pages 2 0 R >>");
        pdf.object(b"<< /Type /Pages /Kids [] /Count 0 >>");
        pdf
    }

    /// Appends the next object, numbered from 1.
    pub fn object(&mut self, body: &[u8]) {
        self.offsets.push(self.data.len());
        let number = self.offsets.len();
        self.data
            .extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        self.data.extend_from_slice(body);
        self.data.extend_from_slice(b"\nendobj\n");
    }

    /// Appends a stream object with an exact `/Length` before `entries`,
    /// which are further dictionary entries.
    pub fn stream(&mut self, entries: &[u8], data: &[u8]) {
        let mut tail = entries.to_vec();
        tail.extend_from_slice(b" >>\n");
        self.stream_object(&tail, data);
    }

    /// Appends one input part. `<< entries >> stream` followed by a line
    /// break and data becomes a stream whose `/Length` is set here; any
    /// other part is an object body exactly as given.
    pub fn part(&mut self, part: &[u8]) {
        if let Some(at) = find(part, b"stream\n") {
            let (head, data) = part.split_at(at);
            if let Some(entries) = head.trim_ascii_start().strip_prefix(b"<<") {
                let data = data.get(b"stream\n".len()..).unwrap_or_default();
                return self.stream_object(entries, data);
            }
        }
        self.object(part);
    }

    /// Appends up to [`MAX_OBJECTS`] parts; after [`Self::without_pages`]
    /// they are objects [`FIRST_OBJECT`], [`FIRST_OBJECT`] + 1, ….
    pub fn parts(&mut self, parts: &[&[u8]]) {
        for part in parts.iter().take(MAX_OBJECTS) {
            self.part(part);
        }
    }

    /// `tail` is the stream dictionary after `<< /Length n`, up to and
    /// including its closing `>>`.
    fn stream_object(&mut self, tail: &[u8], data: &[u8]) {
        let mut body = format!("<< /Length {} ", data.len()).into_bytes();
        body.extend_from_slice(tail);
        body.extend_from_slice(b"stream\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.object(&body);
    }

    /// The complete file: cross-reference table, trailer and `startxref`.
    pub fn finish(mut self) -> Vec<u8> {
        let xref = self.data.len();
        let size = self.offsets.len() + 1;
        self.data
            .extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in &self.offsets {
            self.data
                .extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        self.data.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        self.data
    }
}
