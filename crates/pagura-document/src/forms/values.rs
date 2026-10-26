//! Terminal types and values (12.7.5); no appearances or actions are evaluated.
use super::{
    ButtonKind, ChoiceOption, ChoiceStyle, FieldFlags, FieldKind, FieldValue,
    attributes::{self, Entry, Inherited, Key},
};
use crate::source::ReadResult;
use crate::view::{Context, Source, identity};
use pagura_core::{Dictionary, Error, Name, Value};

pub(super) fn kind<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    effective: &Inherited<'a>,
    flags: FieldFlags,
    source: Source,
) -> ReadResult<FieldKind<'a>> {
    let ft = effective
        .get(Key::Ft)
        .ok_or_else(|| source.error(Error::syntax("terminal form field requires FT")))?;
    let (value, type_source) = context.resolve(ft.value, ft.source)?;
    let name = value
        .as_name()
        .ok_or_else(|| type_source.fault(Error::syntax("form FT must be a name")))?;
    Ok(match name.as_bytes() {
        b"Btn" => FieldKind::Button {
            kind: if flags.contains(FieldFlags::PUSHBUTTON) {
                ButtonKind::Push
            } else if flags.contains(FieldFlags::RADIO) {
                ButtonKind::Radio
            } else {
                ButtonKind::CheckBox
            },
            export_values: effective
                .get(Key::Opt)
                .map(|e| strings(context, e))
                .transpose()?
                .unwrap_or_default(),
        },
        b"Tx" => FieldKind::Text {
            max_length: effective
                .get(Key::MaxLen)
                .map(|e| {
                    attributes::unsigned(e, context).and_then(|v| {
                        Ok(usize::try_from(v).map_err(|_| {
                            e.source
                                .error(Error::limit("form MaxLen exceeds address space"))
                        })?)
                    })
                })
                .transpose()?,
            multiline: flags.contains(FieldFlags::MULTILINE),
            password: flags.contains(FieldFlags::PASSWORD),
            comb: flags.contains(FieldFlags::COMB),
            rich_text: flags.contains(FieldFlags::RICH_TEXT),
            rich_value: context.present(dictionary, b"RV", source)?,
        },
        b"Ch" => FieldKind::Choice {
            // Edit applies only to combo boxes (Table 233); a list ignores it.
            style: if flags.contains(FieldFlags::COMBO) {
                ChoiceStyle::Combo {
                    editable: flags.contains(FieldFlags::EDIT),
                }
            } else {
                ChoiceStyle::List
            },
            multi_select: flags.contains(FieldFlags::MULTI_SELECT),
            options: options(context, effective.get(Key::Opt))?,
        },
        b"Sig" => FieldKind::Signature {
            signed: effective.get(Key::V).is_some(),
        },
        _ => FieldKind::Other(name),
    })
}

fn strings<'a>(context: &mut Context<'a>, entry: Entry<'a>) -> ReadResult<Vec<String>> {
    let values = attributes::array(context, entry.value, entry.source)?;
    context.allocate::<String>(values.len(), entry.source)?;
    values
        .iter()
        .map(|v| {
            context.step(0, entry.source)?;
            attributes::string(context, v, entry.source)
        })
        .collect()
}

fn options<'a>(
    context: &mut Context<'a>,
    entry: Option<Entry<'a>>,
) -> ReadResult<Vec<ChoiceOption>> {
    let Some(entry) = entry else {
        return Ok(Vec::new());
    };
    let values = attributes::array(context, entry.value, entry.source)?;
    context.allocate::<ChoiceOption>(values.len(), entry.source)?;
    values
        .iter()
        .map(|v| {
            context.step(0, entry.source)?;
            let (v, source) = context.resolve(v, entry.source)?;
            let (export, display) = if let Value::Array(pair) = v {
                let [export, display] = pair.as_slice() else {
                    return Err(source
                        .error(Error::syntax("choice option pair requires two strings"))
                        .into());
                };
                (
                    attributes::string(context, export, source)?,
                    attributes::string(context, display, source)?,
                )
            } else {
                let display = attributes::string(context, v, source)?;
                context.allocate::<u8>(display.len(), source)?;
                (display.clone(), display)
            };
            Ok(ChoiceOption { export, display })
        })
        .collect()
}

/// The value of an inherited V or DV entry, and where it was read.
pub(super) fn read<'a>(
    context: &mut Context<'a>,
    kind: &FieldKind<'a>,
    entry: Option<Entry<'a>>,
) -> ReadResult<Option<(FieldValue<'a>, Source)>> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    let (value, source) = context.resolve(entry.value, entry.source)?;
    let value = match kind {
        FieldKind::Text { .. } => FieldValue::Text(text_value(context, value, source)?),
        FieldKind::Button { .. } => FieldValue::Name(
            value
                .as_name()
                .ok_or_else(|| source.error(Error::syntax("button value must be a name")))?,
        ),
        FieldKind::Choice { .. } => {
            if value.as_string().is_some() {
                FieldValue::Text(attributes::string(context, value, source)?)
            } else {
                FieldValue::Texts(strings(context, Entry { value, source })?)
            }
        }
        FieldKind::Signature { .. } => {
            FieldValue::Signature(value.as_dictionary().ok_or_else(|| {
                source.error(Error::syntax("signature value must be a dictionary"))
            })?)
        }
        FieldKind::Group | FieldKind::Other(_) => FieldValue::Other(value),
    };
    Ok(Some((value, source)))
}

fn text_value(context: &mut Context<'_>, value: &Value, source: Source) -> ReadResult<String> {
    if let Some(bytes) = value.as_string() {
        return Ok(context.text(bytes, source)?);
    }
    let object = source
        .object
        .map(|id| context.document.object(id))
        .transpose()?
        .filter(|o| identity(o.value()) == identity(value) && o.stream_range().is_some())
        .ok_or_else(|| {
            source.error(Error::syntax("text field value must be a string or stream"))
        })?;
    let bytes = context.document.stream_data(object)?;
    if bytes.len() > context.document.limits().string_bytes {
        return Err(source
            .error(Error::limit("form text stream exceeds string budget"))
            .into());
    }
    context.allocate::<u8>(bytes.len(), source)?;
    Ok(context.text(&bytes, source)?)
}

pub(super) fn field_type<'a>(
    context: &mut Context<'a>,
    effective: &Inherited<'a>,
) -> ReadResult<Option<&'a Name>> {
    effective
        .get(Key::Ft)
        .map(|e| {
            let (value, source) = context.resolve(e.value, e.source)?;
            value
                .as_name()
                .ok_or_else(|| source.fault(Error::syntax("form FT must be a name")))
        })
        .transpose()
}
