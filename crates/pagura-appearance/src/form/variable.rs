//! Plain text and combo content inside a Tx marked-content section (12.7.4.3).

use super::{
    Font, choice,
    da::DefaultAppearance,
    layout::{self, TextLayout},
};
use crate::program::Program;
use pagura_core::{Error, Rect, Result};
use pagura_document::forms::{
    ChoiceStyle, Field, FieldFlags, FieldKind, FieldValue, FilledField, FilledValue, FormFiller,
};

/// Writes the variable text of field `index` in `area`.
pub(super) fn write(
    out: &mut Program,
    filler: &FormFiller<'_>,
    index: usize,
    area: Rect,
    font: &Font,
    da: &DefaultAppearance,
) -> Result<()> {
    let field = filler
        .form()
        .fields
        .get(index)
        .ok_or(Error::invalid_argument("filled field index is absent"))?;
    let text = match &field.kind {
        FieldKind::Text { .. } => text(filler, field, index)?,
        FieldKind::Choice {
            style: ChoiceStyle::Combo { editable },
            options,
            ..
        } => combo(filler, field, index, *editable, options)?,
        FieldKind::Choice {
            style: ChoiceStyle::List,
            ..
        } => {
            let values = choice::values(filler, field, index);
            return choice::list(out, filler, index, area, font, da, &values);
        }
        _ => {
            return Err(Error::unsupported(
                pagura_document::features::FIELD_TYPE,
                "field has no variable text appearance",
            ));
        }
    };
    out.text("BT\n")?;
    layout::write(out, font, da, area, &text, &filler.document().limits())?;
    out.text("ET\n")
}

/// A text field's value and layout flags (Table 228).
fn text<'a>(
    filler: &'a FormFiller<'_>,
    field: &'a Field<'_>,
    index: usize,
) -> Result<TextLayout<'a>> {
    let FieldKind::Text {
        multiline,
        comb,
        max_length,
        password,
        rich_text,
        ..
    } = &field.kind
    else {
        return Err(Error::syntax("field is not text"));
    };
    if *rich_text {
        return Err(Error::unsupported(
            pagura_document::features::RICH_TEXT,
            "rich text appearance generation is unavailable",
        ));
    }
    let value = match filler.get(index).map(FilledField::value) {
        Some(FilledValue::Text(s)) => s.as_str(),
        _ => match &field.value {
            Some(FieldValue::Text(s)) => s.as_str(),
            None => "",
            _ => return Err(Error::syntax("invalid text field value")),
        },
    };
    if *comb
        && (max_length.is_none_or(|n| n == 0)
            || *multiline
            || *password
            || field.flags.contains(FieldFlags::FILE_SELECT))
    {
        return Err(Error::syntax("invalid Comb flags or MaxLen"));
    }
    Ok(TextLayout {
        text: value,
        multiline: *multiline,
        comb: if *comb { *max_length } else { None },
        password: *password,
        quadding: field.justification,
        no_scroll: field.flags.contains(FieldFlags::DO_NOT_SCROLL),
    })
}

/// A combo box's single value, which must be an option unless editable
/// (12.7.5.4).
fn combo<'a>(
    filler: &'a FormFiller<'_>,
    field: &'a Field<'_>,
    index: usize,
    editable: bool,
    options: &[pagura_document::forms::ChoiceOption],
) -> Result<TextLayout<'a>> {
    let values = choice::values(filler, field, index);
    let value = values.first().map_or("", |s| *s);
    if values.len() > 1 || value.contains(['\r', '\n']) {
        return Err(Error::syntax("combo value must be a single line"));
    }
    if !editable && !values.is_empty() && !options.iter().any(|o| o.display == value) {
        return Err(Error::syntax("combo value is absent from Opt"));
    }
    Ok(TextLayout {
        text: value,
        multiline: false,
        comb: None,
        password: false,
        quadding: field.justification,
        no_scroll: false,
    })
}
