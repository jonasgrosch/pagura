//! Text flag validation, before retaining any replacement (12.7.5.3).

use crate::features;
use crate::forms::{Field, FieldFlags, FieldKind, FilledValue};
use pagura_core::{Error, Limits, Meter, Result};

/// Keep XML inert at the document layer. The shared appearance parser validates
/// the selected replacement before it can be displayed or persisted.
pub(super) fn rich(
    xml: &str,
    default_style: Option<&str>,
    limits: &Limits,
    budget: &mut Meter,
) -> Result<FilledValue> {
    if xml.len() > limits.string_bytes
        || default_style.is_some_and(|style| style.len() > limits.string_bytes)
    {
        return Err(Error::limit("form rich text input budget exceeded"));
    }
    budget.charge(xml.len().saturating_add(default_style.map_or(0, str::len)))?;
    Ok(FilledValue::RichText {
        xml: xml.to_owned(),
        default_style: default_style.map(str::to_owned),
    })
}

pub(super) fn validate(
    field: &Field<'_>,
    text: &str,
    limits: &Limits,
    budget: &mut Meter,
) -> Result<String> {
    let FieldKind::Text {
        max_length,
        multiline,
        password,
        comb,
        rich_text,
        ..
    } = field.kind
    else {
        return Err(Error::invalid_argument("field is not text"));
    };
    if text.len() > limits.string_bytes {
        return Err(Error::limit("form text input budget exceeded"));
    }
    if rich_text {
        return Err(Error::unsupported(
            features::RICH_TEXT,
            "rich text filling is unavailable",
        ));
    }
    // The field's flags contradict each other: a document defect.
    if comb
        && (max_length.is_none_or(|maximum| maximum == 0)
            || multiline
            || password
            || field.flags.contains(FieldFlags::FILE_SELECT))
    {
        return Err(Error::syntax(
            "Comb requires a positive MaxLen and excludes Multiline, Password and FileSelect",
        ));
    }
    if !multiline && text.contains(['\r', '\n']) {
        return Err(Error::invalid_argument(
            "single-line field cannot contain line breaks",
        ));
    }
    budget.charge(text.len())?;
    let mut value = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    let mut count = 0usize;
    while let Some(character) = characters.next() {
        let character = if character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            '\n'
        } else {
            character
        };
        count += 1;
        if max_length.is_some_and(|maximum| count > maximum) {
            return Err(Error::invalid_argument(
                "text exceeds field MaxLen in Unicode scalar values",
            ));
        }
        value.push(character);
    }
    Ok(value)
}
