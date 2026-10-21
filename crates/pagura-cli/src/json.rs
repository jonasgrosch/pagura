//! The JSON the commands print: one object per line, written straight to
//! the output. Each report carries a `schema` number that changes only when
//! a field changes meaning or disappears.

use crate::failure::Failure;
use pagura::{Diagnostic, DiagnosticKind, Document};
use std::borrow::Cow;
use std::io::{self, Write};

/// Schema of `inspect`.
pub(crate) const INSPECT_SCHEMA: u32 = 2;
/// Schema of `scan`.
pub(crate) const SCAN_SCHEMA: u32 = 1;
/// Schema of the stage timings of `render` and `convert`.
pub(crate) const TIMINGS_SCHEMA: u32 = 1;
/// Schema of `microbench`.
pub(crate) const MICROBENCH_SCHEMA: u32 = 2;

/// Writes `text` as a JSON string literal.
pub(crate) fn string(out: &mut impl Write, text: &str) -> io::Result<()> {
    out.write_all(b"\"")?;
    let mut rest = text;
    while let Some(at) = rest.find(|c: char| c == '"' || c == '\\' || c.is_control()) {
        out.write_all(&rest.as_bytes()[..at])?;
        let mut characters = rest[at..].chars();
        match characters.next() {
            Some('"') => out.write_all(b"\\\"")?,
            Some('\\') => out.write_all(b"\\\\")?,
            Some(control) => write!(out, "\\u{:04x}", u32::from(control))?,
            None => {}
        }
        rest = characters.as_str();
    }
    out.write_all(rest.as_bytes())?;
    out.write_all(b"\"")
}

/// Writes `items` as a JSON array of strings.
pub(crate) fn strings<'a>(
    out: &mut impl Write,
    items: impl IntoIterator<Item = &'a str>,
) -> io::Result<()> {
    out.write_all(b"[")?;
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        string(out, item)?;
    }
    out.write_all(b"]")
}

/// Writes `"repairs":[…],"information":[…]` for the document's diagnostics.
pub(crate) fn diagnostics(out: &mut impl Write, document: &Document) -> io::Result<()> {
    let diagnostics = document.diagnostics();
    let information = |item: &&Diagnostic| matches!(item.kind, DiagnosticKind::Information(_));
    out.write_all(b"\"repairs\":")?;
    strings(
        out,
        diagnostics
            .iter()
            .filter_map(Diagnostic::repair)
            .map(pagura::Repair::as_str),
    )?;
    out.write_all(b",\"information\":")?;
    strings(
        out,
        diagnostics.iter().filter(information).map(Diagnostic::tag),
    )
}

/// Writes an object describing a failure: its kind, the feature and refused
/// repair tags of an engine error, and its message without the context the
/// error's `Display` adds.
pub(crate) fn failure(out: &mut impl Write, failure: &Failure) -> io::Result<()> {
    let (feature, repair, message): (_, _, Cow<'_, str>) = match failure {
        Failure::Engine(error) => (error.feature(), error.repair, error.message.into()),
        Failure::Io(_) | Failure::Request(_) => (None, None, failure.to_string().into()),
    };
    write!(out, "{{\"kind\":\"{}\",\"feature\":", failure.kind())?;
    optional(out, feature.map(pagura::Feature::as_str))?;
    out.write_all(b",\"repair\":")?;
    optional(out, repair.map(pagura::Repair::as_str))?;
    out.write_all(b",\"message\":")?;
    string(out, &message)?;
    out.write_all(b"}")
}

fn optional(out: &mut impl Write, text: Option<&str>) -> io::Result<()> {
    match text {
        Some(text) => string(out, text),
        None => out.write_all(b"null"),
    }
}

#[cfg(test)]
mod tests {
    use super::{failure, string, strings};
    use crate::failure::Failure;

    fn written(write: impl FnOnce(&mut Vec<u8>) -> std::io::Result<()>) -> String {
        let mut out = Vec::new();
        write(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn strings_escape_quotes_backslashes_and_controls() {
        assert_eq!(
            written(|out| string(out, "a\"b\\c\nd\u{7f}é")),
            "\"a\\\"b\\\\c\\u000ad\\u007fé\""
        );
        assert_eq!(written(|out| strings(out, ["x", "y"])), "[\"x\",\"y\"]");
        assert_eq!(written(|out| strings(out, [])), "[]");
    }

    #[test]
    fn failures_keep_their_kind_and_tags() {
        let limit = Failure::Engine(pagura::Error::limit("input exceeds the file size budget"));
        assert_eq!(
            written(|out| failure(out, &limit)),
            "{\"kind\":\"limit\",\"feature\":null,\"repair\":null,\
             \"message\":\"input exceeds the file size budget\"}"
        );
        let io = Failure::Io(std::io::Error::other("disk"));
        assert!(written(|out| failure(out, &io)).starts_with("{\"kind\":\"io\""));
        let request = Failure::Request("page 9 outside the document's 2 pages".into());
        assert!(written(|out| failure(out, &request)).starts_with("{\"kind\":\"request\""));
    }
}
