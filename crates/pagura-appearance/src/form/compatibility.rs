//! Explicit viewer policy for incomplete resources and flattened field text.

use pagura_core::{Dictionary, Error, Name, Result, Value};
use pagura_document::Document;

/// The standard 14 font names (9.6.2.2).
const STANDARD_FONTS: [&[u8]; 14] = [
    b"Helvetica",
    b"Helvetica-Bold",
    b"Helvetica-Oblique",
    b"Helvetica-BoldOblique",
    b"Times-Roman",
    b"Times-Bold",
    b"Times-Italic",
    b"Times-BoldItalic",
    b"Courier",
    b"Courier-Bold",
    b"Courier-Oblique",
    b"Courier-BoldOblique",
    b"Symbol",
    b"ZapfDingbats",
];

/// Supplies only an exact standard-14 name. Never replaces a present
/// resource or guesses a custom font or subset encoding from a similarly
/// named font.
pub(crate) fn supply_font(
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
    if !STANDARD_FONTS.contains(&name) {
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
