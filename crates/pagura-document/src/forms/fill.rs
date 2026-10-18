//! Caller-owned, atomic field changes (ISO 32000-2, 12.7.4–12.7.5).
use super::{AcroForm, Field, FieldKind, Widget};
use crate::Document;
use pagura_core::{Error, Name, Result};
use std::collections::BTreeMap;

mod buttons;
mod choices;
mod text;
pub use choices::{ChoiceSelection, ChoiceValue};

/// A button selection (12.7.5.2). Widget indexes refer to `AcroForm::widgets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
pub enum FilledValue {
    /// Plain Unicode text; passwords remain only in caller-owned memory.
    Text(String),
    /// The V name, including Off (12.7.5.2).
    Button(Name),
    /// Display values for V and sorted option indexes for I (Table 234).
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
    fn bytes(&self) -> usize {
        let value = match &self.value {
            FilledValue::Text(text) => text.capacity(),
            FilledValue::Button(name) => name.len(),
            FilledValue::Choice(value) => value.bytes(),
        };
        value
            .saturating_add(
                self.states.iter().fold(
                    self.states
                        .capacity()
                        .saturating_mul(size_of::<WidgetState>()),
                    |bytes, s| bytes.saturating_add(s.state.len()),
                ),
            )
            .saturating_add(size_of::<Self>() + 64)
    }
}

/// An in-memory form session bound to one immutable document (12.7).
///
/// Construct with `Document::form_filler`. Failed sets leave the session intact.
/// Names are lookup/display data; use field and widget indexes for identity.
/// Result storage is caller-owned, separately bounded by `document_bytes`
/// (ADR 0012); it is not published to the document's retained cache. Saving,
/// XFA, rich text, calculation and validation actions are outside this API.
#[derive(Debug)]
pub struct FormFiller<'a> {
    document: &'a Document,
    form: AcroForm<'a>,
    changes: BTreeMap<usize, FilledField>,
    bytes: usize,
}
impl<'a> FormFiller<'a> {
    pub(super) fn new(document: &'a Document, form: AcroForm<'a>) -> Self {
        Self {
            document,
            form,
            changes: BTreeMap::new(),
            bytes: 0,
        }
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
            self.bytes -= change.bytes();
        }
    }
    /// Validates and replaces a field value atomically (12.7.5).
    ///
    /// Required and NoExport affect export, not fill-in. Password values stay in
    /// memory and must never be serialized (12.7.5.3). No actions are executed.
    ///
    /// # Errors
    /// Returns positioned InvalidArgument errors for read-only fields, push
    /// buttons, wrong input kinds, excess length, invalid/ambiguous selections or
    /// flags; Unsupported for XFA, rich text and non-fillable field types; Limit for
    /// input, work or aggregate replacement-storage budgets. A missing field
    /// index returns InvalidArgument without invented source context.
    pub fn set(&mut self, index: usize, input: FillValue<'_>) -> Result<&FilledField> {
        let field = self
            .form
            .fields
            .get(index)
            .ok_or(Error::invalid_argument("field index is outside the form"))?;
        let change = self
            .validate(index, field, input)
            .map_err(|e| context(field, e))?;
        let previous = self.changes.get(&index).map_or(0, FilledField::bytes);
        let bytes = self
            .bytes
            .checked_sub(previous)
            .and_then(|n| n.checked_add(change.bytes()))
            .filter(|n| *n <= self.document.limits().document_bytes)
            .ok_or_else(|| context(field, Error::limit("filled form storage budget exceeded")))?;
        self.bytes = bytes;
        Ok(self.changes.entry(index).insert_entry(change).into_mut())
    }
    fn validate(
        &self,
        index: usize,
        field: &Field<'_>,
        input: FillValue<'_>,
    ) -> Result<FilledField> {
        if self.form.has_xfa {
            return Err(Error::unsupported(
                crate::features::XFA,
                "XFA filling is unavailable",
            ));
        }
        if field.flags.read_only() {
            return Err(Error::invalid_argument("field is read-only"));
        }
        let mut states = Vec::new();
        let value = match (&field.kind, input) {
            (FieldKind::Text { .. }, FillValue::Text(value)) => {
                FilledValue::Text(text::validate(field, value, &self.document.limits())?)
            }
            (
                FieldKind::Button {
                    kind,
                    export_values,
                },
                FillValue::Button(value),
            ) => {
                let (name, selected) = buttons::validate(self, index, *kind, export_values, value)?;
                states = selected;
                FilledValue::Button(name)
            }
            (FieldKind::Choice { .. }, FillValue::Choice(value)) => {
                FilledValue::Choice(choices::validate(field, value, &self.document.limits())?)
            }
            (FieldKind::Group | FieldKind::Signature { .. } | FieldKind::Other(_), _) => {
                return Err(Error::unsupported(
                    crate::features::FIELD_TYPE,
                    "field type cannot be filled",
                ));
            }
            _ => {
                return Err(Error::invalid_argument(
                    "input kind does not match field type",
                ));
            }
        };
        Ok(FilledField {
            field: index,
            value,
            states,
        })
    }
    fn widgets(&self, field: usize) -> impl Iterator<Item = (usize, &Widget<'a>)> {
        self.form
            .widgets
            .iter()
            .enumerate()
            .filter(move |(_, w)| w.field == field)
    }
}
pub(super) fn context(field: &Field<'_>, mut error: Error) -> Error {
    error = error.at(field.position);
    if let Some(object) = field.object {
        error = error.in_object(object);
    }
    error
}

#[cfg(test)]
mod tests;
