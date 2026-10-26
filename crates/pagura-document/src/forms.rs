//! Interactive form source views and caller-owned filling (ISO 32000-2, 12.7).
//!
//! Fields and widgets borrow unmodified source dictionaries. The field list is
//! flat preorder; parent indexes precede children. Actions, calculation order,
//! rich text and XFA are data only: nothing is executed or interpreted.
//!
//! One traversal shares navigation's depth, node and estimated result-memory
//! budgets. Results are caller-owned and uncached; loaded source objects use
//! the document's retained-cache budget (ADR 0012). No partial result is returned
//! on failure. Names are display/lookup data, never unique object identities.
mod attributes;
mod display;
mod fill;
mod model;
mod names;
mod tree;
mod values;
mod widgets;

use crate::Document;

pub use fill::{
    ButtonValue, ChoiceSelection, ChoiceValue, FillValue, FilledField, FilledValue, FormFiller,
    WidgetState,
};

pub use model::{
    AcroForm, AdditionalAction, ButtonKind, ChoiceOption, ChoiceStyle, Field, FieldFlags,
    FieldKind, FieldValue, Justification, Widget, WidgetField,
};
use pagura_core::{Dictionary, Result};

impl Document {
    /// Starts an in-memory form session, or returns None without an AcroForm.
    /// Original values, bytes, actions and appearances remain unchanged.
    ///
    /// # Errors
    /// As [`Self::acro_form`]; changes have their own bounded storage.
    pub fn form_filler(&self) -> Result<Option<FormFiller<'_>>> {
        self.acro_form()?
            .map(|form| FormFiller::new(self, form))
            .transpose()
    }

    /// Reads only the appearance source data of `annotation` (12.7.4.3–12.7.5).
    ///
    /// Parent ancestry must agree with Fields/Kids membership. Only those
    /// membership arrays, selected ancestors and owning-field child names are
    /// visited; unrelated field subtrees, actions and default values are not
    /// read. Returns None without AcroForm or
    /// for a field type other than Tx/Ch/Btn. Call after stored/hidden appearance
    /// selection, so unused field data stays lazy.
    ///
    /// # Errors
    /// Syntax for inconsistent membership or malformed selected
    /// entries, Cycle for cyclic Parent ancestry, and existing reference,
    /// traversal, depth and result-memory Limit errors. Resolved objects retain
    /// their source locations; a caller-supplied dictionary has no known location.
    /// This narrow view does not search the whole tree to repair inconsistent
    /// backlinks; [`Self::acro_form`]
    /// retains its full-tree repair contract.
    pub fn widget_field<'a>(
        &'a self,
        annotation: &'a Dictionary,
    ) -> Result<Option<WidgetField<'a>>> {
        display::read(self, annotation)
    }

    /// Reads the AcroForm and its field/widget tree (12.7.3–12.7.5).
    /// Returns `None` when AcroForm is absent or null (7.3.9).
    ///
    /// # Errors
    /// Returns positioned syntax, cycle/reference or resource-limit errors.
    /// Safe structural and naming corrections are reported repairs (ADR 0003);
    /// strict mode rejects them with their repair tag.
    pub fn acro_form(&self) -> Result<Option<AcroForm<'_>>> {
        tree::read(self)
    }
}

#[cfg(test)]
mod tests;
