//! Plain/rich text and combo content in a Tx section (12.7.4.3).

use super::{
    Font, choice,
    da::DefaultAppearance,
    layout::{self, TextLayout},
};
use crate::fonts::FontSource;
use crate::program::Program;
use crate::{rich, values::optional};
use pagura_core::{Dictionary, Error, ErrorKind, Meter, Name, Rect, Result, Value};
use pagura_document::Document;
use pagura_document::forms::{
    ChoiceStyle, Field, FieldFlags, FieldKind, FieldValue, FilledField, FilledValue, FormFiller,
    Justification,
};

/// Borrowed source or edited data for the one variable-text layout path.
pub(super) struct Inputs<'a, 'd> {
    pub(super) document: &'d Document,
    pub(super) kind: &'a FieldKind<'d>,
    pub(super) flags: FieldFlags,
    pub(super) value: Option<&'a FieldValue<'d>>,
    pub(super) change: Option<&'a FilledValue>,
    pub(super) dictionary: &'d Dictionary,
    pub(super) default_appearance: Option<&'d [u8]>,
    pub(super) justification: Justification,
}

impl<'a, 'd> Inputs<'a, 'd> {
    pub(super) fn edited(filler: &'a FormFiller<'d>, index: usize, field: &'a Field<'d>) -> Self {
        Self {
            document: filler.document(),
            kind: &field.kind,
            flags: field.flags,
            value: field.value.as_ref(),
            change: filler.get(index).map(FilledField::value),
            dictionary: field.dictionary,
            default_appearance: field.default_appearance,
            justification: field.justification,
        }
    }

    pub(super) fn is_rich(&self) -> bool {
        matches!(
            self.kind,
            FieldKind::Text {
                rich_text: true,
                ..
            }
        )
    }
}

/// Writes variable text using source values unless an explicit edit replaces them.
pub(super) fn write(
    out: &mut Program,
    field: &Inputs<'_, '_>,
    area: Rect,
    fonts: &dyn FontSource,
    da: &DefaultAppearance,
    resources: &mut Dictionary,
) -> Result<()> {
    if field.is_rich() {
        return write_rich(out, field, area, fonts, da, resources).map_err(rich_error);
    }
    let font = Font::load(field.document, resources, da.font.as_bytes(), fonts)?;
    let text = match field.kind {
        FieldKind::Text { .. } => text(field)?,
        FieldKind::Choice {
            style: ChoiceStyle::Combo { editable },
            options,
            ..
        } => combo(field, *editable, options)?,
        FieldKind::Choice {
            style: ChoiceStyle::List,
            ..
        } => {
            let values = choice::values(field);
            return choice::list(out, field, area, &font, da, &values);
        }
        _ => {
            return Err(Error::unsupported(
                pagura_document::features::FIELD_TYPE,
                "field has no variable text appearance",
            ));
        }
    };
    out.text("BT\n")?;
    layout::write(out, &font, da, area, &text, &field.document.limits())?;
    out.text("ET\n")
}

/// A text field's value and layout flags (Table 228).
fn text<'a>(field: &'a Inputs<'_, '_>) -> Result<TextLayout<'a>> {
    let value = match field.change {
        Some(FilledValue::Text(s)) => s.as_str(),
        _ => match field.value {
            Some(FieldValue::Text(s)) => s.as_str(),
            None => "",
            _ => return Err(Error::syntax("invalid text field value")),
        },
    };
    text_layout(field, value)
}

fn text_layout<'a>(field: &Inputs<'_, '_>, value: &'a str) -> Result<TextLayout<'a>> {
    let FieldKind::Text {
        multiline,
        comb,
        max_length,
        password,
        ..
    } = field.kind
    else {
        return Err(Error::syntax("field is not text"));
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

fn write_rich(
    out: &mut Program,
    field: &Inputs<'_, '_>,
    area: Rect,
    fonts: &dyn FontSource,
    da: &DefaultAppearance,
    resources: &mut Dictionary,
) -> Result<()> {
    let limits = field.document.limits();
    let mut storage = Meter::new(limits.document_bytes, "rich field storage budget exceeded");
    let mut work = Meter::new(limits.operations, "rich field work budget exceeded");
    let (mut text, value) = rich_text(field, da, &mut storage, &mut work)?;
    let configuration = text_layout(field, &value)?;
    if configuration.password {
        text.mask(&mut work)?;
    }
    let width = if configuration.multiline {
        area.width()
    } else {
        f64::INFINITY
    };
    let mut layout = rich::layout(
        field.document,
        text,
        da,
        width,
        rich::Fonts {
            source: fonts,
            resources: Some(resources),
        },
        &mut storage,
        &mut work,
    )?;
    layout.write_field(out, area, &configuration, &mut work)?;
    resources.insert(Name::new(b"Font"), Value::Dictionary(layout.fonts));
    Ok(())
}

/// The shared parser's selected characters also supply V during persistence.
pub(super) fn rich_text(
    field: &Inputs<'_, '_>,
    da: &DefaultAppearance,
    storage: &mut Meter,
    work: &mut Meter,
) -> Result<(rich::Text, String)> {
    let FieldKind::Text {
        rich_text: true,
        rich_value,
        max_length,
        multiline,
        ..
    } = field.kind
    else {
        return Err(Error::invalid_argument("field does not hold rich text"));
    };
    let source;
    let default_style;
    let (xml, replacement_style) = match field.change {
        Some(FilledValue::RichText { xml, default_style }) => {
            (xml.as_str(), default_style.as_deref())
        }
        Some(_) => {
            return Err(Error::invalid_argument(
                "rich field requires a rich replacement",
            ));
        }
        None => {
            let selected = rich_value
                .map(|value| field.document.resolve(value))
                .transpose()?;
            if selected.is_none_or(Value::is_null) {
                if field.value.is_some() {
                    return Err(Error::syntax("valued RichText field requires RV"));
                }
                ("<body/>", None)
            } else {
                source = rich::content(
                    field.document,
                    rich_value.ok_or(Error::syntax("missing rich field value"))?,
                    storage,
                )?;
                (source.as_str(), None)
            }
        }
    };
    let style = if let Some(style) = replacement_style {
        Some(style)
    } else if let Some(value) = optional(field.document, field.dictionary, b"DS")? {
        let bytes = value
            .as_string()
            .ok_or(Error::syntax("rich field DS must be a text string"))?;
        default_style = rich::decode(field.document, bytes, storage)?;
        Some(default_style.as_str())
    } else {
        None
    };
    let text = rich::parse(
        field.document,
        xml,
        style,
        da,
        field.justification,
        storage,
        work,
    )?;
    let value = text.value(&field.document.limits(), storage, work)?;
    text_layout(field, &value)?;
    work.charge(value.len())?;
    if !*multiline && value.contains('\n') {
        return Err(if field.change.is_some() {
            Error::invalid_argument("single-line rich field cannot contain line breaks")
        } else {
            Error::syntax("single-line rich field contains line breaks")
        });
    }
    if field.change.is_some() && max_length.is_some_and(|maximum| value.chars().count() > maximum) {
        return Err(Error::invalid_argument(
            "rich text exceeds field MaxLen in Unicode scalar values",
        ));
    }
    Ok((text, value))
}

pub(super) fn rich_error(mut error: Error) -> Error {
    if error.feature() == Some(crate::features::ANNOTATION_APPEARANCE) {
        error.kind = ErrorKind::Unsupported(pagura_document::features::RICH_TEXT);
    }
    error
}

/// A combo box's single value, which must be an option unless editable
/// (12.7.5.4).
fn combo<'a>(
    field: &'a Inputs<'_, '_>,
    editable: bool,
    options: &[pagura_document::forms::ChoiceOption],
) -> Result<TextLayout<'a>> {
    let values = choice::values(field);
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
