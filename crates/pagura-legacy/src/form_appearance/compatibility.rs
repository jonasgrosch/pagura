//! Explicit viewer policy for incomplete resources and flattened field text.
use super::Program;
use pagura_core::{
    Dictionary, Error, Limits, Name, Result, Value,
    syntax::{Origin, Parser},
};
use pagura_document::Document;

/// Supply only an exact standard-14 name. Never replace a present resource or
/// guess a custom font/subset encoding from a similarly named font.
pub(super) fn supply_font(
    document: &Document,
    resources: &mut Dictionary,
    name: &[u8],
) -> Result<()> {
    let mut fonts = match resources.get(b"Font") {
        Some(value) => document.resolve_dictionary(value)?.clone(),
        None => Dictionary::new(),
    };
    if fonts.contains_key(name) {
        return Ok(());
    }
    if !matches!(
        name,
        b"Helvetica"
            | b"Helvetica-Bold"
            | b"Helvetica-Oblique"
            | b"Helvetica-BoldOblique"
            | b"Times-Roman"
            | b"Times-Bold"
            | b"Times-Italic"
            | b"Times-BoldItalic"
            | b"Courier"
            | b"Courier-Bold"
            | b"Courier-Oblique"
            | b"Courier-BoldOblique"
            | b"Symbol"
            | b"ZapfDingbats"
    ) {
        return Err(Error::syntax(
            "missing form font is not a named standard font",
        ));
    }
    let mut font = Dictionary::new();
    font.insert(Name::new(b"Type"), Value::name(b"Font"));
    font.insert(Name::new(b"Subtype"), Value::name(b"Type1"));
    font.insert(Name::new(b"BaseFont"), Value::name(name));
    if !matches!(name, b"Symbol" | b"ZapfDingbats") {
        font.insert(Name::new(b"Encoding"), Value::name(b"WinAnsiEncoding"));
    }
    fonts.insert(Name::new(name), Value::Dictionary(font));
    resources.insert(Name::new(b"Font"), Value::Dictionary(fonts));
    if resources.retained_bytes() > document.limits().document_bytes {
        return Err(Error::limit("appearance font resource budget exceeded"));
    }
    Ok(())
}

/// Remove old text-show operations while preserving non-text artwork verbatim.
/// The caller first validates scopes and excludes inline images. Clipping text
/// and external XObjects could carry inseparable artwork, so remain unsupported.
pub(super) fn without_text(bytes: &[u8], limits: Limits) -> Result<Vec<u8>> {
    let mut parser = Parser::new(bytes, Origin::Buffer, limits);
    let mut out = Program::new(limits);
    let (mut start, mut copied, mut tokens) = (0, 0, 0);
    let mut operands = Vec::new();
    while let Some(next) = parser.peek() {
        if tokens >= limits.operations {
            return Err(Error::limit("appearance text replacement budget exceeded"));
        }
        tokens += 1;
        if operands.is_empty() {
            start = parser.offset();
        }
        if matches!(
            next,
            b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.' | b'0'..=b'9'
        ) || parser.data().get(parser.offset()..).is_some_and(|b| {
            b.starts_with(b"true") || b.starts_with(b"false") || b.starts_with(b"null")
        }) {
            if operands.len() >= limits.value_nodes {
                return Err(Error::limit("appearance operand budget exceeded"));
            }
            operands.push(parser.parse()?);
            continue;
        }
        let op = parser.word()?;
        if op == b"Do" || op == b"Tr" && !matches!(operands.as_slice(), [Value::Integer(0..=3)]) {
            return Err(Error::unsupported(
                crate::features::APPEARANCE_UPDATE,
                "unmarked appearance contains inseparable text clipping or XObject artwork",
            ));
        }
        if matches!(op, b"Tj" | b"TJ" | b"'" | b"\"") {
            out.bytes(&bytes[copied..start])?;
            copied = parser.offset();
        }
        operands.clear();
    }
    out.bytes(&bytes[copied..])?;
    Ok(out.finish())
}
