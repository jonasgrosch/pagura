//! Button V/AS selection without evaluating appearance programs (12.7.5.2).

use super::{ButtonValue, FormFiller, WidgetState};
use crate::forms::{ButtonKind, FieldFlags, FieldValue, Widget};
use pagura_core::{Error, Meter, Name, Result};

/// Whether widgets sharing the selected on state turn on together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    /// Only the chosen widget turns on.
    Exclusive,
    /// Check boxes, and radios in unison, turn on together (Table 229).
    Unison,
}

pub(super) fn validate(
    filler: &FormFiller<'_>,
    field: usize,
    kind: ButtonKind,
    exports: &[String],
    input: ButtonValue<'_>,
    budget: &mut Meter,
) -> Result<(Name, Vec<WidgetState>)> {
    if kind == ButtonKind::Push {
        return Err(Error::invalid_argument("push buttons have no value"));
    }
    let flags = filler
        .form
        .fields
        .get(field)
        .map_or(FieldFlags::default(), |field| field.flags);
    let widgets: Vec<(usize, &Widget<'_>)> = {
        let count = filler.widgets(field).count();
        budget.charge_items::<(usize, &Widget<'_>)>(count)?;
        filler.widgets(field).collect()
    };
    if widgets.is_empty() {
        return Err(Error::syntax("button field has no widgets"));
    }
    if !exports.is_empty() && exports.len() != widgets.len() {
        return Err(Error::syntax(
            "button Opt must have one export per widget (Table 230)",
        ));
    }
    let selection = if kind == ButtonKind::CheckBox || flags.contains(FieldFlags::RADIOS_IN_UNISON)
    {
        Selection::Unison
    } else {
        Selection::Exclusive
    };
    let selected = select(filler, &widgets, exports, input, selection)?;
    if selected.is_none()
        && kind == ButtonKind::Radio
        && flags.contains(FieldFlags::NO_TOGGLE_TO_OFF)
    {
        return Err(Error::invalid_argument(
            "NoToggleToOff requires a selected radio widget",
        ));
    }
    let name = match selected {
        Some((ordinal, requested)) => {
            let (_, widget) = widgets[ordinal];
            on_state(filler, widget, ordinal, exports, requested)?
        }
        None => Name::new(b"Off"),
    };
    budget.charge_items::<WidgetState>(widgets.len())?;
    let mut states = Vec::with_capacity(widgets.len());
    for (ordinal, &(index, widget)) in widgets.iter().enumerate() {
        let on = selected.is_some_and(|(chosen, _)| chosen == ordinal)
            || (selection == Selection::Unison
                && selected.is_some()
                && supports(filler, widget, ordinal, exports, &name)?);
        let state = if on { name.clone() } else { Name::new(b"Off") };
        budget.charge(state.len())?;
        states.push(WidgetState {
            widget: index,
            state,
        });
    }
    Ok((name, states))
}

/// The on state a widget without appearances would use.
fn fallback(
    filler: &FormFiller<'_>,
    widget: &Widget<'_>,
    ordinal: usize,
    exports: &[String],
) -> Name {
    if !exports.is_empty() {
        return Name::new(ordinal.to_string().as_bytes());
    }
    let state = |value: &FieldValue<'_>| match value {
        FieldValue::Name(name) if name.as_bytes() != b"Off" => Some((*name).clone()),
        _ => None,
    };
    let field = filler.form.fields.get(widget.field);
    widget
        .appearance_state
        .filter(|name| name.as_bytes() != b"Off")
        .cloned()
        .or_else(|| field.and_then(|field| field.value.as_ref()).and_then(state))
        .or_else(|| {
            field
                .and_then(|field| field.default_value.as_ref())
                .and_then(state)
        })
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
        [] if !widget.has_appearance => Ok(fallback(filler, widget, ordinal, exports)),
        [] => Err(Error::syntax("existing button appearance has no on state")),
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
        || (!widget.has_appearance && fallback(filler, widget, ordinal, exports) == *state))
}

/// The chosen widget's ordinal and requested state, or `None` for Off.
/// Comparisons cost no more than the parsed export and state names.
fn select<'a>(
    filler: &FormFiller<'_>,
    widgets: &[(usize, &Widget<'_>)],
    exports: &[String],
    input: ButtonValue<'a>,
    selection: Selection,
) -> Result<Option<(usize, Option<&'a Name>)>> {
    let requested = match input {
        ButtonValue::Off => return Ok(None),
        ButtonValue::State(name) if name.as_bytes() == b"Off" => return Ok(None),
        ButtonValue::Widget { index, state } => {
            let ordinal = widgets
                .iter()
                .position(|&(widget, _)| widget == index)
                .ok_or(Error::invalid_argument(
                    "widget does not belong to the field",
                ))?;
            return Ok(Some((ordinal, state)));
        }
        ButtonValue::State(name) => Some(name),
        ButtonValue::Export(_) => None,
    };
    let mut selected = None;
    let mut first_state = None;
    for (ordinal, &(_, widget)) in widgets.iter().enumerate() {
        let matches = match input {
            ButtonValue::Export(export) => exports.get(ordinal).is_some_and(|e| e == export),
            ButtonValue::State(name) => supports(filler, widget, ordinal, exports, name)?,
            _ => false,
        };
        if !matches {
            continue;
        }
        let state = on_state(filler, widget, ordinal, exports, requested)?;
        if selected.is_some()
            && (selection == Selection::Exclusive || first_state.as_ref() != Some(&state))
        {
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
