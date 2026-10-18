//! Button V/AS selection without evaluating appearance programs (12.7.5.2).
use super::super::{ButtonKind, Widget};
use super::{ButtonValue, FormFiller, WidgetState};
use pagura_core::{Error, Name, Result};

pub(super) fn validate(
    filler: &FormFiller<'_>,
    field: usize,
    kind: ButtonKind,
    exports: &[String],
    input: ButtonValue<'_>,
) -> Result<(Name, Vec<WidgetState>)> {
    if kind == ButtonKind::Push {
        return Err(Error::invalid_argument("push buttons have no value"));
    }
    let config = &filler.form.fields[field];
    let limits = filler.document.limits();
    if filler
        .form
        .widgets
        .len()
        .saturating_mul(size_of::<(usize, &Widget<'_>)>())
        > limits.document_bytes
    {
        return Err(Error::limit("button widget selection budget exceeded"));
    }
    let widgets: Vec<_> = filler.widgets(field).collect();
    if widgets.len() > limits.value_nodes
        || widgets.len().saturating_mul(128) > limits.document_bytes
    {
        return Err(Error::limit("button widget selection budget exceeded"));
    }
    if !exports.is_empty() && exports.len() != widgets.len() {
        return Err(Error::invalid_argument(
            "button Opt must have one export per widget",
        ));
    }
    if widgets.is_empty() {
        return Err(Error::invalid_argument("button field has no widgets"));
    }
    let unison = kind == ButtonKind::CheckBox || config.flags.contains(26);
    let selected = select(filler, &widgets, exports, input, unison)?;
    if selected.is_none() && kind == ButtonKind::Radio && config.flags.contains(15) {
        return Err(Error::invalid_argument(
            "NoToggleToOff requires a selected radio widget",
        ));
    }
    let name = if let Some((ordinal, requested)) = selected {
        on_state(filler, widgets[ordinal].1, ordinal, exports, requested)?
    } else {
        Name::new(b"Off")
    };
    let mut bytes = 0usize;
    let mut states = Vec::with_capacity(widgets.len());
    for (ordinal, &(index, widget)) in widgets.iter().enumerate() {
        let on = selected.is_some_and(|(i, _)| i == ordinal)
            || (unison && selected.is_some() && supports(filler, widget, ordinal, exports, &name)?);
        let state = if on { name.clone() } else { Name::new(b"Off") };
        bytes = bytes
            .saturating_add(size_of::<WidgetState>())
            .saturating_add(state.len());
        if bytes > limits.document_bytes {
            return Err(Error::limit("button state storage budget exceeded"));
        }
        states.push(WidgetState {
            widget: index,
            state,
        });
    }
    Ok((name, states))
}
fn has_appearance(filler: &FormFiller<'_>, widget: &Widget<'_>) -> Result<bool> {
    widget
        .dictionary
        .get(b"AP")
        .map(|ap| filler.document.resolve(ap).map(|v| !v.is_null()))
        .transpose()
        .map(|v| v.unwrap_or(false))
}
fn fallback(
    filler: &FormFiller<'_>,
    widget: &Widget<'_>,
    ordinal: usize,
    exports: &[String],
) -> Name {
    if !exports.is_empty() {
        return Name::new(ordinal.to_string().as_bytes());
    }
    let field = &filler.form.fields[widget.field];
    let value = |v: &super::super::FieldValue<'_>| match v {
        super::super::FieldValue::Name(n) if n.as_bytes() != b"Off" => Some((*n).clone()),
        _ => None,
    };
    widget
        .appearance_state
        .filter(|n| n.as_bytes() != b"Off")
        .cloned()
        .or_else(|| field.value.as_ref().and_then(value))
        .or_else(|| field.default_value.as_ref().and_then(value))
        .unwrap_or_else(|| Name::new(b"Yes"))
}
fn on_state(
    filler: &FormFiller<'_>,
    widget: &Widget<'_>,
    ordinal: usize,
    exports: &[String],
    requested: Option<&Name>,
) -> Result<Name> {
    if let Some(requested) = requested {
        if requested.as_bytes() != b"Off" && supports(filler, widget, ordinal, exports, requested)?
        {
            return Ok(requested.clone());
        }
        return Err(Error::invalid_argument(
            "widget has no requested on appearance",
        ));
    }
    match widget.on_states.as_slice() {
        [state] => Ok((*state).clone()),
        [] if !has_appearance(filler, widget)? => Ok(fallback(filler, widget, ordinal, exports)),
        [] => Err(Error::invalid_argument(
            "existing button appearance has no on state",
        )),
        _ => Err(Error::invalid_argument(
            "widget has several on appearances; specify a state",
        )),
    }
}
fn supports(
    filler: &FormFiller<'_>,
    widget: &Widget<'_>,
    ordinal: usize,
    exports: &[String],
    state: &Name,
) -> Result<bool> {
    Ok(widget.on_states.contains(&state)
        || (!has_appearance(filler, widget)?
            && fallback(filler, widget, ordinal, exports) == *state))
}
fn select<'a>(
    filler: &FormFiller<'_>,
    widgets: &[(usize, &Widget<'_>)],
    exports: &[String],
    input: ButtonValue<'a>,
    unison: bool,
) -> Result<Option<(usize, Option<&'a Name>)>> {
    if matches!(input, ButtonValue::Off)
        || matches!(input, ButtonValue::State(n) if n.as_bytes() == b"Off")
    {
        return Ok(None);
    }
    if let ButtonValue::Widget { index, state } = input {
        let ordinal =
            widgets
                .iter()
                .position(|(i, _)| *i == index)
                .ok_or(Error::invalid_argument(
                    "widget does not belong to the field",
                ))?;
        return Ok(Some((ordinal, state)));
    }
    let input_bytes = match input {
        ButtonValue::Export(s) => s.len(),
        ButtonValue::State(n) => n.len(),
        _ => 0,
    };
    if input_bytes > filler.document.limits().string_bytes
        || widgets.len().saturating_mul(input_bytes.max(1)) > filler.document.limits().operations
    {
        return Err(Error::limit("button lookup work budget exceeded"));
    }
    let mut selected = None;
    let mut first_state = None;
    for (ordinal, &(_, widget)) in widgets.iter().enumerate() {
        let matches = match input {
            ButtonValue::Export(s) => exports.get(ordinal).is_some_and(|e| e == s),
            ButtonValue::State(n) => supports(filler, widget, ordinal, exports, n)?,
            _ => false,
        };
        if !matches {
            continue;
        }
        let requested = match input {
            ButtonValue::State(n) => Some(n),
            _ => None,
        };
        let state = on_state(filler, widget, ordinal, exports, requested)?;
        if selected.is_some() && (!unison || first_state.as_ref() != Some(&state)) {
            return Err(Error::invalid_argument(
                "ambiguous button value; select a widget index",
            ));
        }
        if selected.is_none() {
            selected = Some((ordinal, requested));
            first_state = Some(state);
        }
    }
    selected.map(Some).ok_or(Error::invalid_argument(
        "button value has no matching widget",
    ))
}
