//! Read-only interactive forms (ISO 32000-2, 12.7).
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
mod model;
mod names;
mod tree;
mod values;
mod widgets;

use crate::Document;
pub use model::{
    AcroForm, AdditionalAction, ButtonKind, ChoiceOption, Field, FieldFlags, FieldKind, FieldValue,
    Justification, Widget,
};
use pagura_core::Result;

impl Document {
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
