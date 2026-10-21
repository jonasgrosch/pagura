//! Caller-owned, atomic field changes (ISO 32000-2, 12.7.4–12.7.5).

use super::{AcroForm, Field, FieldKind, Widget};
use crate::{Document, features};
use pagura_core::{Error, INDEX_ENTRY_OVERHEAD, Meter, Name, Result};
use std::collections::BTreeMap;
use std::fmt;

mod buttons;
mod choices;
mod text;

pub use choices::{ChoiceSelection, ChoiceValue};

/// A button selection (12.7.5.2). Widget indexes refer to `AcroForm::widgets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ButtonValue<'a> {
    /// Clear the field, subject to `NoToggleToOff`.
    Off,
    /// Select a widget; `state` disambiguates widgets with several on states.
    Widget {
        /// Index in the form's widget list, never an object number or name.
        index: usize,
        /// An existing normal-appearance state, or None for its sole on state.
        state: Option<&'a Name>,
    },
    /// Select by an Opt export string. Ambiguous matches require a widget index.
    Export(&'a str),
    /// Select an existing appearance-state name.
    State(&'a Name),
}

/// Typed input for a terminal field (12.7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FillValue<'a> {
    /// Plain text. CR and CRLF are normalized to LF before length validation.
    Text(&'a str),
    /// Check box or radio selection.
    Button(ButtonValue<'a>),
    /// Choice options or an editable combo's text.
    Choice(ChoiceSelection<'a>),
}

/// A validated replacement value, separate from the original source dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilledValue {
    /// Plain Unicode text; passwords remain only in caller-owned memory.
    Text(String),
    /// The V name, including Off (12.7.5.2).
    Button(Name),
    /// Display values for V and sorted option indexes (Table 234).
    Choice(ChoiceValue),
}

/// A widget's selected AS name. Existing appearance programs are never changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetState {
    pub(super) widget: usize,
    pub(super) state: Name,
}

impl WidgetState {
    /// Index in the source form's widget list.
    pub fn widget(&self) -> usize {
        self.widget
    }

    /// The selected normal appearance, including Off.
    pub fn state(&self) -> &Name {
        &self.state
    }
}

/// One atomic change. Actions and calculations are never executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilledField {
    pub(super) field: usize,
    pub(super) value: FilledValue,
    pub(super) states: Vec<WidgetState>,
    /// The storage this change was charged, returned when it is replaced.
    charged: usize,
}

impl FilledField {
    /// Index in the source form's field list.
    pub fn field(&self) -> usize {
        self.field
    }

    /// The validated replacement for V.
    pub fn value(&self) -> &FilledValue {
        &self.value
    }

    /// Button widget states in source order; empty for variable text.
    pub fn widget_states(&self) -> &[WidgetState] {
        &self.states
    }
}

/// An in-memory form session bound to one immutable document (12.7).
///
/// Construct with `Document::form_filler`. Failed sets leave the session intact.
/// Names are lookup/display data; use field and widget indexes for identity.
/// Result storage is caller-owned, separately bounded by `document_bytes`
/// (ADR 0012); it is not published to the document's retained cache. Saving,
/// XFA, rich text, calculation and validation actions are outside this API.
pub struct FormFiller<'a> {
    document: &'a Document,
    form: AcroForm<'a>,
    changes: BTreeMap<usize, FilledField>,
    storage: Meter,
    /// Widget indexes grouped by field: field `f` owns
    /// `widgets[starts[f]..starts[f + 1]]`, in source order.
    widgets: Box<[usize]>,
    starts: Box<[usize]>,
}

impl fmt::Debug for FormFiller<'_> {
    /// Values may hold password text, so only their count is shown.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FormFiller")
            .field("fields", &self.form.fields.len())
            .field("widgets", &self.form.widgets.len())
            .field("changes", &self.changes.len())
            .finish_non_exhaustive()
    }
}

impl<'a> FormFiller<'a> {
    pub(super) fn new(document: &'a Document, form: AcroForm<'a>) -> Result<Self> {
        let mut storage = Meter::new(
            document.limits().document_bytes,
            "filled form storage budget exceeded",
        );
        storage.charge_items::<usize>(
            form.widgets
                .len()
                .saturating_add(form.fields.len())
                .saturating_add(1),
        )?;
        // A counting sort keeps each field's widgets in source order, even
        // when its widgets and child fields interleave in Kids.
        let mut starts = vec![0; form.fields.len() + 1];
        for widget in &form.widgets {
            if let Some(count) = starts.get_mut(widget.field + 1) {
                *count += 1;
            }
        }
        for index in 1..starts.len() {
            starts[index] += starts[index - 1];
        }
        let mut next = starts.clone();
        let mut widgets = vec![0; form.widgets.len()];
        for (index, widget) in form.widgets.iter().enumerate() {
            if let Some(slot) = next.get_mut(widget.field)
                && let Some(target) = widgets.get_mut(*slot)
            {
                *target = index;
                *slot += 1;
            }
        }
        Ok(Self {
            document,
            form,
            changes: BTreeMap::new(),
            storage,
            widgets: widgets.into_boxed_slice(),
            starts: starts.into_boxed_slice(),
        })
    }

    /// The unchanged source form.
    pub fn form(&self) -> &AcroForm<'a> {
        &self.form
    }

    /// The bound source document. Original bytes and dictionaries stay unchanged.
    pub fn document(&self) -> &'a Document {
        self.document
    }

    /// Latest replacement for a field, or None when unchanged.
    pub fn get(&self, field: usize) -> Option<&FilledField> {
        self.changes.get(&field)
    }

    /// Latest replacements in field-index order.
    pub fn changes(&self) -> impl Iterator<Item = &FilledField> {
        self.changes.values()
    }

    /// Removes a replacement, restoring the original value and appearances.
    pub fn reset(&mut self, field: usize) {
        if let Some(change) = self.changes.remove(&field) {
            self.storage.refund(change.charged);
        }
    }

    /// Validates and replaces a field value atomically (12.7.5).
    ///
    /// Required and NoExport affect export, not fill-in. Password values stay in
    /// memory and must never be serialized (12.7.5.3). No actions are executed.
    ///
    /// # Errors
    /// Returns positioned `InvalidArgument` errors for read-only fields, push
    /// buttons, groups, wrong input kinds, excess length and invalid or
    /// ambiguous selections; `Syntax` for field structure that cannot hold the
    /// value (such as a button whose Opt and widgets disagree); `Unsupported`
    /// for XFA, rich text and signature or extension fields; `Limit` for
    /// input and aggregate replacement-storage budgets. A missing field index
    /// returns `InvalidArgument` without invented source context.
    pub fn set(&mut self, index: usize, input: FillValue<'_>) -> Result<&FilledField> {
        let field = self
            .form
            .fields
            .get(index)
            .ok_or(Error::invalid_argument("field index is outside the form"))?;
        let previous = self.changes.get(&index).map_or(0, |change| change.charged);
        // The replacement may reuse what the previous one holds.
        let mut budget = Meter::new(
            self.storage.remaining().saturating_add(previous),
            "filled form storage budget exceeded",
        );
        let change = budget
            .charge(size_of::<FilledField>() + INDEX_ENTRY_OVERHEAD)
            .and_then(|()| self.validate(index, field, input, &mut budget))
            .map_err(|error| context(field, error))?;
        self.storage.refund(previous);
        self.storage.charge(budget.used())?;
        let change = FilledField {
            charged: budget.used(),
            ..change
        };
        Ok(self.changes.entry(index).insert_entry(change).into_mut())
    }

    fn validate(
        &self,
        index: usize,
        field: &Field<'_>,
        input: FillValue<'_>,
        budget: &mut Meter,
    ) -> Result<FilledField> {
        if self.form.has_xfa {
            return Err(Error::unsupported(
                features::XFA,
                "XFA filling is unavailable",
            ));
        }
        if field.flags.read_only() {
            return Err(Error::invalid_argument("field is read-only"));
        }
        let mut states = Vec::new();
        let value = match (&field.kind, input) {
            (FieldKind::Text { .. }, FillValue::Text(value)) => FilledValue::Text(text::validate(
                field,
                value,
                &self.document.limits(),
                budget,
            )?),
            (
                FieldKind::Button {
                    kind,
                    export_values,
                },
                FillValue::Button(value),
            ) => {
                let (name, selected) =
                    buttons::validate(self, index, *kind, export_values, value, budget)?;
                states = selected;
                FilledValue::Button(name)
            }
            (FieldKind::Choice { .. }, FillValue::Choice(value)) => FilledValue::Choice(
                choices::validate(field, value, &self.document.limits(), budget)?,
            ),
            (FieldKind::Group, _) => {
                return Err(Error::invalid_argument(
                    "a field group has no value; fill its terminal fields",
                ));
            }
            (FieldKind::Text { .. } | FieldKind::Button { .. } | FieldKind::Choice { .. }, _) => {
                return Err(Error::invalid_argument(
                    "input kind does not match field type",
                ));
            }
            _ => {
                return Err(Error::unsupported(
                    features::FIELD_TYPE,
                    "field type cannot be filled",
                ));
            }
        };
        Ok(FilledField {
            field: index,
            value,
            states,
            charged: 0,
        })
    }

    /// The widgets of `field`, as form widget indexes with their widgets.
    fn widgets(&self, field: usize) -> impl Iterator<Item = (usize, &Widget<'a>)> {
        let start = self.starts.get(field).copied().unwrap_or_default();
        let end = self.starts.get(field + 1).copied().unwrap_or(start);
        self.widgets
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .filter_map(|&index| Some((index, self.form.widgets.get(index)?)))
    }
}

fn context(field: &Field<'_>, error: Error) -> Error {
    error.at(field.position).in_object_opt(field.object)
}

#[cfg(test)]
mod tests;
