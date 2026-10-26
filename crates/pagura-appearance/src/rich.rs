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
use pagura_core::{Diagnostics, Dictionary, Error, Limits, Meter, Result, text_string};
use pagura_document::{Document, forms::Justification};
use style::Style;

pub(crate) use layout::Layout;

/// `None` means that the caller should use its existing plain-text path.
pub(crate) fn prepare(
    document: &Document,
    annotation: &Dictionary,
    text: &str,
    appearance: &DefaultAppearance,
    alignment: Justification,
    width: f64,
    fonts: &dyn FontSource,
) -> Result<Option<Layout>> {
    let rc = optional(document, annotation, b"RC")?;
    let ds = optional(document, annotation, b"DS")?;
    if rc.is_none() && ds.is_none() {
        return Ok(None);
    }
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
        xml::Text::parse(&source, base, &limits)?
    } else {
        work.charge(text.len())?;
        xml::Text::plain(text, base, &limits)?
    };
    layout::prepare(
        document, parsed, appearance, width, fonts, &limits, &mut work,
    )
    .map(Some)
}

fn rich_content(document: &Document, annotation: &Dictionary, limits: &Limits) -> Result<String> {
    let value = annotation
        .get(b"RC")
        .ok_or(Error::syntax("missing rich text content"))?;
    if let Some(id) = value.as_reference() {
        let object = document.object(id)?;
        if object.is_stream() {
            let bytes = document.stream_data(object)?;
            if bytes.len() > limits.document_bytes || bytes.len() > limits.string_bytes {
                return Err(Error::limit("rich text source storage exceeded"));
            }
            if bytes.starts_with(&[0xfe, 0xff]) || bytes.starts_with(&[0xff, 0xfe]) {
                return decode(document, &bytes, limits);
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
