//! Bounded inert annotation rich text (12.5.6.6; XFA 3.3 chapters 5 and 27).
//! XML/CSS select appearance data only. Font decoding is shared with forms;
//! the result is ordinary PDF text and path operators for the one interpreter.

mod layout;
mod style;
mod xml;

use crate::features;
use crate::fonts::FontSource;
use crate::form::da::DefaultAppearance;
use crate::values::optional;
use pagura_core::{Diagnostics, Dictionary, Error, Limits, Meter, Result, Value, text_string};
use pagura_document::{Document, forms::Justification};
use style::Style;

pub(crate) use layout::Layout;

/// Prepares plain or rich annotation text using the common bounded layout.
pub(crate) fn prepare(
    document: &Document,
    annotation: &Dictionary,
    text: &str,
    appearance: &DefaultAppearance,
    alignment: Justification,
    width: f64,
    fonts: &dyn FontSource,
) -> Result<Layout> {
    let rc = optional(document, annotation, b"RC")?;
    let ds = if optional(document, annotation, b"Subtype")?
        .and_then(Value::as_name)
        .is_some_and(|name| name.as_bytes() == b"FreeText")
    {
        optional(document, annotation, b"DS")?
    } else {
        None
    };
    let limits = document.limits();
    // Source, parsed styles, encoded layout and font/resource state coexist.
    // Each phase gets one quarter of the caller's storage allowance.
    let limits = Limits {
        document_bytes: limits.document_bytes / 4,
        ..limits
    };
    let mut work = Meter::new(limits.operations, "rich text work budget exceeded");
    let family = std::str::from_utf8(appearance.font.as_bytes())
        .map_err(|_| unsupported("non-Unicode rich text font resource name"))?
        .to_owned();
    let mut base = Style::new(family, appearance.size, alignment);
    base.leading = (appearance.leading > 0.0).then_some(appearance.leading);
    base.rise = appearance.rise;
    base.spacing = appearance.character_space;
    if let Some(ds) = ds {
        let bytes = ds
            .as_string()
            .ok_or(Error::syntax("rich text DS must be a text string"))?;
        let css = decode(document, bytes, &limits)?;
        work.charge_product(css.len(), 4)?;
        base.apply(&css)?;
    }
    let parsed = if rc.is_some() {
        let source = rich_content(document, annotation, &limits)?;
        work.charge_product(source.len(), 4)?;
        xml::Text::parse(&source, base, &limits, &mut work)?
    } else {
        work.charge(text.len())?;
        xml::Text::plain(text, base, &limits, &mut work)?
    };
    layout::prepare(
        document, parsed, appearance, width, fonts, &limits, &mut work,
    )
}

/// Shared PDF Contents decoding; unused Contents is never read when RC wins.
pub(crate) fn contents(
    document: &Document,
    annotation: &Dictionary,
    storage: &mut Meter,
) -> Result<String> {
    let Some(value) = optional(document, annotation, b"Contents")? else {
        return Ok(String::new());
    };
    let bytes = value
        .as_string()
        .ok_or(Error::syntax("annotation Contents must be a text string"))?;
    let limits = document.limits();
    storage.charge(bytes.len().saturating_mul(3).min(limits.string_bytes))?;
    let mut diagnostics = Diagnostics::new(document.recovery());
    let result = text_string::decode(bytes, &limits, &mut diagnostics)?;
    document.report(diagnostics.take());
    Ok(result)
}

fn rich_content(document: &Document, annotation: &Dictionary, limits: &Limits) -> Result<String> {
    let value = annotation
        .get(b"RC")
        .ok_or(Error::syntax("missing rich text content"))?;
    if let Some(id) = value.as_reference() {
        let object = document.object(id)?;
        if object.is_stream() {
            let bytes = document.stream_data(object)?;
            if bytes.len().saturating_mul(4) > limits.document_bytes
                || bytes.len() > limits.string_bytes
            {
                return Err(Error::limit("rich text source storage exceeded"));
            }
            if bytes.starts_with(&[0xfe, 0xff]) || bytes.starts_with(&[0xff, 0xfe]) {
                // XML streams follow their Unicode BOM; PDF text-string
                // decoding is different and accepts only big-endian UTF-16.
                let (units, remainder) = bytes[2..].as_chunks::<2>();
                if !remainder.is_empty() {
                    return Err(Error::syntax("incomplete rich text UTF-16 unit"));
                }
                let units = units.iter().map(|pair| {
                    if bytes[0] == 0xff {
                        u16::from_le_bytes(*pair)
                    } else {
                        u16::from_be_bytes(*pair)
                    }
                });
                let mut source = String::new();
                for character in char::decode_utf16(units) {
                    let character = character
                        .map_err(|_| Error::syntax("invalid rich text UTF-16 sequence"))?;
                    if source.len().saturating_add(character.len_utf8()) > limits.string_bytes {
                        return Err(Error::limit("rich text string budget exceeded"));
                    }
                    source.push(character);
                }
                return Ok(source);
            }
            return std::str::from_utf8(&bytes)
                .map(str::to_owned)
                .map_err(|_| Error::syntax("rich text stream must contain XML UTF-8 or UTF-16"));
        }
    }
    let bytes = document.resolve(value)?.as_string().ok_or(Error::syntax(
        "rich text RC must be a text string or stream",
    ))?;
    decode(document, bytes, limits)
}

fn decode(document: &Document, bytes: &[u8], limits: &Limits) -> Result<String> {
    if bytes.len().saturating_mul(3) > limits.document_bytes {
        return Err(Error::limit("rich text source storage exceeded"));
    }
    let mut diagnostics = Diagnostics::new(document.recovery());
    let result = text_string::decode(bytes, limits, &mut diagnostics)?;
    document.report(diagnostics.take());
    Ok(result)
}

fn unsupported(message: &'static str) -> Error {
    Error::unsupported(features::ANNOTATION_APPEARANCE, message)
}

#[cfg(test)]
mod tests;
