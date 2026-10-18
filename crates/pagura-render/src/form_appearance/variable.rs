//! Plain text and combo content inside a Tx marked-content section (12.7.4.3).
use super::{
    Program, choice,
    da::DefaultAppearance,
    font::Font,
    layout::{self, TextLayout},
};
use pagura_core::{Error, Rect, Result};
use pagura_document::forms::{FieldKind, FieldValue, FilledField, FilledValue, FormFiller};

pub(super) fn write(
    out: &mut Program,
    filler: &FormFiller<'_>,
    index: usize,
    area: Rect,
    font: &Font,
    da: &DefaultAppearance,
) -> Result<()> {
    let field = &filler.form().fields[index];
    match &field.kind {
        FieldKind::Text { .. } => text(out, filler, index, area, font, da),
        FieldKind::Choice { combo: true, .. } => combo(out, filler, index, area, font, da),
        FieldKind::Choice { combo: false, .. } => choice::list(
            out,
            filler,
            index,
            area,
            font,
            da,
            &choice::values(filler, field, index),
        ),
        _ => Err(Error::unsupported(
            pagura_document::forms::features::FIELD_TYPE,
            "field has no variable text appearance",
        )),
    }
}
fn text(
    out: &mut Program,
    filler: &FormFiller<'_>,
    index: usize,
    area: Rect,
    font: &Font,
    da: &DefaultAppearance,
) -> Result<()> {
    let field = &filler.form().fields[index];
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
            pagura_document::forms::features::RICH_TEXT,
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
            || field.flags.contains(21))
    {
        return Err(Error::syntax("invalid Comb flags or MaxLen"));
    }
    let layout = TextLayout {
        text: value,
        multiline: *multiline,
        comb: if *comb { *max_length } else { None },
        password: *password,
        quadding: field.justification,
        no_scroll: field.flags.contains(24),
    };
    out.text("BT\n")?;
    layout::write(out, font, da, area, &layout, filler.document().limits())?;
    out.text("ET\n")
}
fn combo(
    out: &mut Program,
    filler: &FormFiller<'_>,
    index: usize,
    area: Rect,
    font: &Font,
    da: &DefaultAppearance,
) -> Result<()> {
    let field = &filler.form().fields[index];
    let FieldKind::Choice {
        editable, options, ..
    } = &field.kind
    else {
        return Err(Error::syntax("field is not a choice"));
    };
    let values = choice::values(filler, field, index);
    let value = values.first().map_or("", |s| *s);
    if values.len() > 1 || value.contains(['\r', '\n']) {
        return Err(Error::syntax("combo value must be a single line"));
    }
    if !editable && !values.is_empty() && !options.iter().any(|o| o.display == value) {
        return Err(Error::syntax("combo value is absent from Opt"));
    }
    let layout = TextLayout {
        text: value,
        multiline: false,
        comb: None,
        password: false,
        quadding: field.justification,
        no_scroll: false,
    };
    out.text("BT\n")?;
    layout::write(out, font, da, area, &layout, filler.document().limits())?;
    out.text("ET\n")
}
