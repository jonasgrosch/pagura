//! Terminal types and values (12.7.5); no appearances or actions are evaluated.
use super::{
    ButtonKind, ChoiceOption, FieldFlags, FieldKind, FieldValue,
    attributes::{self, Entry, Inherited},
};
use crate::navigation::{
    context::{Context, Source, identity},
    text,
};
use pagura_core::{Dictionary, Error, Name, Result, Value};

pub(super) fn kind<'a>(
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    attrs: &Inherited<'a>,
    flags: FieldFlags,
    source: Source,
) -> Result<FieldKind<'a>> {
    let ft = attrs
        .get(b"FT")
        .ok_or_else(|| source.error(Error::syntax("terminal form field requires FT")))?;
    let (value, type_source) = ctx.resolve(ft.value, ft.source)?;
    let name = value
        .as_name()
        .ok_or_else(|| type_source.error(Error::syntax("form FT must be a name")))?;
    Ok(match name.as_bytes() {
        b"Btn" => FieldKind::Button {
            kind: if flags.contains(17) {
                ButtonKind::Push
            } else if flags.contains(16) {
                ButtonKind::Radio
            } else {
                ButtonKind::CheckBox
            },
            export_values: attrs
                .get(b"Opt")
                .map(|e| strings(ctx, e))
                .transpose()?
                .unwrap_or_default(),
        },
        b"Tx" => FieldKind::Text {
            max_length: attrs
                .get(b"MaxLen")
                .map(|e| {
                    attributes::unsigned(e, ctx).and_then(|v| {
                        usize::try_from(v).map_err(|_| {
                            e.source
                                .error(Error::limit("form MaxLen exceeds address space"))
                        })
                    })
                })
                .transpose()?,
            multiline: flags.contains(13),
            password: flags.contains(14),
            comb: flags.contains(25),
            rich_text: flags.contains(26),
            rich_value: ctx.present(d, b"RV", source)?,
        },
        b"Ch" => FieldKind::Choice {
            combo: flags.contains(18),
            editable: flags.contains(19),
            multi_select: flags.contains(22),
            options: options(ctx, attrs.get(b"Opt"))?,
        },
        b"Sig" => FieldKind::Signature {
            signed: attrs.get(b"V").is_some(),
        },
        _ => FieldKind::Other(name),
    })
}
fn strings<'a>(ctx: &mut Context<'a>, entry: Entry<'a>) -> Result<Vec<String>> {
    let values = attributes::array(ctx, entry.value, entry.source)?;
    ctx.allocate::<String>(values.len(), entry.source)?;
    values
        .iter()
        .map(|v| {
            ctx.step(0, entry.source)?;
            attributes::string(ctx, v, entry.source)
        })
        .collect()
}
fn options<'a>(ctx: &mut Context<'a>, entry: Option<Entry<'a>>) -> Result<Vec<ChoiceOption>> {
    let Some(entry) = entry else {
        return Ok(Vec::new());
    };
    let values = attributes::array(ctx, entry.value, entry.source)?;
    ctx.allocate::<ChoiceOption>(values.len(), entry.source)?;
    values
        .iter()
        .map(|v| {
            ctx.step(0, entry.source)?;
            let (v, source) = ctx.resolve(v, entry.source)?;
            let (export, display) = if let Value::Array(pair) = v {
                let [export, display] = pair.as_slice() else {
                    return Err(
                        source.error(Error::syntax("choice option pair requires two strings"))
                    );
                };
                (
                    attributes::string(ctx, export, source)?,
                    attributes::string(ctx, display, source)?,
                )
            } else {
                let display = attributes::string(ctx, v, source)?;
                ctx.allocate::<u8>(display.len(), source)?;
                (display.clone(), display)
            };
            Ok(ChoiceOption { export, display })
        })
        .collect()
}
pub(super) fn read<'a>(
    ctx: &mut Context<'a>,
    kind: &FieldKind<'a>,
    entry: Option<Entry<'a>>,
) -> Result<Option<FieldValue<'a>>> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    let (value, source) = ctx.resolve(entry.value, entry.source)?;
    Ok(Some(match kind {
        FieldKind::Text { .. } => FieldValue::Text(text_value(ctx, value, source)?),
        FieldKind::Button { .. } => FieldValue::Name(
            value
                .as_name()
                .ok_or_else(|| source.error(Error::syntax("button value must be a name")))?,
        ),
        FieldKind::Choice { .. } => {
            if value.as_string().is_some() {
                FieldValue::Text(attributes::string(ctx, value, source)?)
            } else {
                FieldValue::Texts(strings(ctx, Entry { value, source })?)
            }
        }
        FieldKind::Signature { .. } => {
            FieldValue::Signature(value.as_dictionary().ok_or_else(|| {
                source.error(Error::syntax("signature value must be a dictionary"))
            })?)
        }
        FieldKind::Group | FieldKind::Other(_) => FieldValue::Other(value),
    }))
}
fn text_value(ctx: &mut Context<'_>, value: &Value, source: Source) -> Result<String> {
    if let Some(bytes) = value.as_string() {
        return text::decode(ctx, bytes, source);
    }
    let object = source
        .object
        .map(|id| ctx.doc.object(id))
        .transpose()?
        .filter(|o| identity(o.value()) == identity(value) && o.stream_range().is_some())
        .ok_or_else(|| {
            source.error(Error::syntax("text field value must be a string or stream"))
        })?;
    let bytes = ctx.doc.stream_data(object)?;
    if bytes.len() > ctx.doc.limits().string_bytes {
        return Err(source.error(Error::limit("form text stream exceeds string budget")));
    }
    ctx.allocate::<u8>(bytes.len(), source)?;
    text::decode(ctx, &bytes, source)
}
pub(super) fn field_type<'a>(
    ctx: &mut Context<'a>,
    attrs: &Inherited<'a>,
) -> Result<Option<&'a Name>> {
    attrs
        .get(b"FT")
        .map(|e| {
            let (value, source) = ctx.resolve(e.value, e.source)?;
            value
                .as_name()
                .ok_or_else(|| source.error(Error::syntax("form FT must be a name")))
        })
        .transpose()
}
