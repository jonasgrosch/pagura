//! Stateless browser editing adapter; each save returns a new immutable PDF.
mod aliases;

use crate::protocol::{Encoder, Reader};
use pagura::editing::{SaveOptions, save_filled_form};
use pagura::forms::{
    AcroForm, ButtonKind, ButtonValue, ChoiceSelection, ChoiceStyle, Field, FieldFlags, FieldKind,
    FieldValue, FillValue,
};
use pagura::{Document, Error, Name, Result};

const MAX_FIELDS: usize = 1024;
const MAX_OPTIONS: usize = 4096;

pub(crate) fn fields(document: &Document) -> Result<Vec<u8>> {
    let mut out = Encoder::new(document.limits().document_bytes.min(32 * 1024 * 1024));
    let Some(form) = document.acro_form()? else {
        out.count(0)?;
        return Ok(out.finish());
    };
    if form.fields.len() > MAX_FIELDS {
        return Err(Error::limit("browser form field budget exceeded"));
    }
    out.count(form.fields.len())?;
    let mut options = 0usize;
    for (index, field) in form.fields.iter().enumerate() {
        out.count(index)?;
        let (kind, flags) = match &field.kind {
            FieldKind::Text {
                multiline,
                password: false,
                rich_text: false,
                ..
            } => (0, u8::from(*multiline) << 2),
            FieldKind::Choice {
                multi_select,
                style,
                ..
            } => (
                1,
                (u8::from(*multi_select) << 1)
                    | (u8::from(matches!(style, ChoiceStyle::Combo { editable: true })) << 3),
            ),
            FieldKind::Button {
                kind: ButtonKind::CheckBox | ButtonKind::Radio,
                ..
            } => (2, 0),
            _ => (3, 0),
        };
        out.push(&[
            kind,
            flags | u8::from(field.flags.contains(FieldFlags::READ_ONLY)),
        ])?;
        out.string(
            field
                .alternative_name
                .as_deref()
                .unwrap_or(&field.qualified_name),
        )?;
        out.string(if kind == 3 {
            ""
        } else {
            match &field.value {
                Some(FieldValue::Text(text)) => text,
                _ => "",
            }
        })?;
        let count = match &field.kind {
            FieldKind::Choice { options, .. } => options.len(),
            FieldKind::Button { .. } if kind == 2 => form
                .widgets
                .iter()
                .filter(|widget| widget.field == index)
                .map(|widget| widget.on_states.len())
                .sum(),
            _ => 0,
        };
        options = options
            .checked_add(count)
            .filter(|count| *count <= MAX_OPTIONS)
            .ok_or(Error::limit("browser form option budget exceeded"))?;
        out.count(count)?;
        write_options(&mut out, &form, field, index, kind)?;
    }
    Ok(out.finish())
}

fn write_options(
    out: &mut Encoder,
    form: &AcroForm<'_>,
    field: &Field<'_>,
    index: usize,
    kind: u8,
) -> Result<()> {
    match &field.kind {
        FieldKind::Choice { options, .. } => {
            for (index, option) in options.iter().enumerate() {
                let selected = match &field.value {
                    Some(FieldValue::Text(text)) => text == &option.display,
                    Some(FieldValue::Texts(texts)) => texts.contains(&option.display),
                    _ => false,
                };
                out.count(index)?;
                out.string(&option.display)?;
                out.push(&[u8::from(selected)])?;
                out.count(0)?;
            }
        }
        FieldKind::Button { export_values, .. } if kind == 2 => {
            for (ordinal, (widget_index, widget)) in form
                .widgets
                .iter()
                .enumerate()
                .filter(|(_, widget)| widget.field == index)
                .enumerate()
            {
                for state in &widget.on_states {
                    let label = export_values
                        .get(ordinal)
                        .cloned()
                        .unwrap_or_else(|| state.to_string());
                    out.count(widget_index)?;
                    out.string(&label)?;
                    out.push(&[u8::from(widget.appearance_state == Some(*state))])?;
                    out.count(state.len())?;
                    out.push(state.as_bytes())?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn save(document: &Document, changes: &[u8]) -> Result<Vec<u8>> {
    let mut reader = Reader::new(changes);
    let count = index(reader.u32()?)?;
    if count > MAX_FIELDS {
        return Err(Error::limit("browser form change budget exceeded"));
    }
    if count == 0 {
        reader.finish()?;
        return pagura::editing::Transaction::new(document).commit(&SaveOptions::default());
    }
    let normalized = aliases::normalize(document)?;
    let normalized = normalized
        .map(|bytes| {
            Document::open(
                bytes,
                &pagura::OpenOptions {
                    recovery: pagura::Recovery::Strict,
                    limits: document.limits(),
                    ..Default::default()
                },
            )
        })
        .transpose()?;
    let document = normalized.as_ref().unwrap_or(document);
    let mut filler = document
        .form_filler()?
        .ok_or(Error::invalid_argument("document has no form"))?;
    let mut previous = None;
    for _ in 0..count {
        let field = index(reader.u32()?)?;
        if previous.is_some_and(|previous| field <= previous) {
            return Err(Error::invalid_argument(
                "form changes must have unique ascending field indexes",
            ));
        }
        previous = Some(field);
        match reader.byte()? {
            0 => {
                filler.set(field, FillValue::Text(text(&mut reader, document)?))?;
            }
            1 => {
                let count = index(reader.u32()?)?;
                if count > MAX_OPTIONS
                    || count.saturating_mul(size_of::<usize>()) > document.limits().document_bytes
                {
                    return Err(Error::limit("browser choice index budget exceeded"));
                }
                let mut indices = Vec::new();
                indices
                    .try_reserve_exact(count)
                    .map_err(|_| Error::limit("browser choice allocation failed"))?;
                for _ in 0..count {
                    indices.push(index(reader.u32()?)?);
                }
                filler.set(field, FillValue::Choice(ChoiceSelection::Indices(&indices)))?;
            }
            2 => {
                filler.set(field, FillValue::Button(ButtonValue::Off))?;
            }
            3 => {
                let widget = index(reader.u32()?)?;
                let state = Name::new(reader.blob(document.limits().string_bytes)?);
                filler.set(
                    field,
                    FillValue::Button(ButtonValue::Widget {
                        index: widget,
                        state: Some(&state),
                    }),
                )?;
            }
            4 => {
                filler.set(
                    field,
                    FillValue::Choice(ChoiceSelection::Text(text(&mut reader, document)?)),
                )?;
            }
            _ => return Err(Error::invalid_argument("unknown browser form change kind")),
        }
    }
    reader.finish()?;
    save_filled_form(
        &filler,
        &SaveOptions {
            form_appearances: pagura::form_appearance::UpdatePolicy::PreserveExisting,
            ..Default::default()
        },
    )
}

fn text<'a>(reader: &mut Reader<'a>, document: &Document) -> Result<&'a str> {
    std::str::from_utf8(reader.blob(document.limits().string_bytes)?)
        .map_err(|_| Error::invalid_argument("form text must be UTF-8"))
}
fn index(value: u32) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::limit("form index exceeds platform range"))
}

/// Map only widgets actually listed by this page, never the optional P backlink.
pub(crate) fn widgets(document: &Document, page: u32) -> Result<Vec<u8>> {
    let page = document.page(index(page)?)?;
    let mut out = Encoder::new(document.limits().document_bytes.min(32 * 1024 * 1024));
    let Some(form) = document.acro_form()? else {
        out.count(0)?;
        return Ok(out.finish());
    };
    let mut matcher = aliases::Matcher::new(document, &form)?;
    let mut selected = std::collections::BTreeSet::new();
    for (_, index) in matcher.page(document, page)? {
        if overlay_visible(document, &form.widgets[index])? {
            selected.insert(index);
        }
    }
    out.count(selected.len())?;
    for index in selected {
        let widget = &form.widgets[index];
        let field = &form.fields[widget.field];
        out.count(index)?;
        out.count(widget.field)?;
        let radio = matches!(
            field.kind,
            FieldKind::Button {
                kind: ButtonKind::Radio,
                ..
            }
        );
        let list = matches!(
            field.kind,
            FieldKind::Choice {
                style: ChoiceStyle::List,
                ..
            }
        );
        out.push(&[u8::from(radio)
            | (u8::from(field.flags.contains(FieldFlags::NO_TOGGLE_TO_OFF)) << 1)
            | (u8::from(field.flags.contains(FieldFlags::RADIOS_IN_UNISON)) << 2)
            | (u8::from(list) << 3)])?;
        let rect = widget.rectangle;
        for value in [rect.x0(), rect.y0(), rect.x1(), rect.y1()] {
            out.push(&value.to_le_bytes())?;
        }
    }
    Ok(out.finish())
}

fn overlay_visible(document: &Document, widget: &pagura::forms::Widget<'_>) -> Result<bool> {
    let integer = |value: &pagura::Value| {
        document
            .resolve(value)?
            .as_integer()
            .ok_or(Error::syntax("annotation flag or rotation must be integer"))
    };
    let flags = widget
        .dictionary
        .get(b"F")
        .map(integer)
        .transpose()?
        .unwrap_or(0);
    if flags < 0 || flags & !1023 != 0 {
        return Err(Error::syntax("invalid annotation flags"));
    }
    let rotation = widget
        .dictionary
        .get(b"MK")
        .map(|v| document.resolve_dictionary(v))
        .transpose()?
        .and_then(|d| d.get(b"R"));
    let rotated = rotation.map(integer).transpose()?.unwrap_or(0) % 360 != 0;
    // View-dependent/optional-content widgets need their own visual policy.
    Ok(flags & (2 | 8 | 16 | 32 | 256) == 0
        && !widget.dictionary.contains_key(b"OC")
        && !rotated
        && widget.rectangle.width() > 0.0
        && widget.rectangle.height() > 0.0)
}
