//! Text flag validation, before retaining any replacement (12.7.5.3).
use super::super::{Field, FieldKind};
use pagura_core::{Error, Limits, Result};

pub(super) fn validate(field: &Field<'_>, text: &str, limits: &Limits) -> Result<String> {
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
    if text.len() > limits.string_bytes
        || text.len() > limits.operations
        || text.len().saturating_add(size_of::<String>()) > limits.document_bytes
    {
        return Err(Error::limit("form text input budget exceeded"));
    }
    if rich_text {
        return Err(Error::unsupported(
            crate::forms::features::RICH_TEXT,
            "rich text filling is unavailable",
        ));
    }
    if comb
        && (max_length.is_none_or(|n| n == 0) || multiline || password || field.flags.contains(21))
    {
        return Err(Error::invalid_argument(
            "Comb requires positive MaxLen and excludes Multiline, Password and FileSelect",
        ));
    }
    if !multiline && text.contains(['\r', '\n']) {
        return Err(Error::invalid_argument(
            "single-line field cannot contain line breaks",
        ));
    }
    let mut value = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut count = 0usize;
    while let Some(c) = chars.next() {
        let c = if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            '\n'
        } else {
            c
        };
        count += 1;
        if max_length.is_some_and(|n| count > n) {
            return Err(Error::invalid_argument(
                "text exceeds field MaxLen in Unicode scalar values",
            ));
        }
        value.push(c);
    }
    Ok(value)
}
