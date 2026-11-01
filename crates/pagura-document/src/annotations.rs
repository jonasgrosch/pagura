//! One recovery gate for members of a page's Annots array (12.5.2).
use crate::{Document, repairs};
use pagura_core::{Diagnostic, ObjectId, Recovery, Result, Value};

impl Document {
    /// The original non-null member of page `page`'s Annots array.
    ///
    /// Table 31 and 12.5.2 require annotation references. As an explicit
    /// reader policy, Repair mode reports and skips a literal null member.
    /// Repair mode also skips an immediate reference to a defined in-use
    /// object whose parsed value is null, reporting its identity and position.
    /// Existing parser repairs compose through that value; their reports retain
    /// normal bounded suppression and are not searched to permit admission.
    /// This does not follow reference chains or validate a surviving annotation;
    /// callers retain its identity, validation and original-array budgets.
    /// Missing/free identities, other generations and chains are never skipped.
    /// This member policy does not interpret the optional Annots field itself.
    ///
    /// A literal-null diagnostic names the owning page with unknown position:
    /// the last content operator is unrelated to its Annots array.
    ///
    /// # Errors
    /// Returns Syntax with `pages.annotation-null` in Strict mode, before
    /// the caller may skip the literal null. No applied repair is reported.
    pub fn annotation_entry<'a>(
        &self,
        value: &'a Value,
        page: ObjectId,
    ) -> Result<Option<&'a Value>> {
        if value.is_null() {
            self.repair(
                Diagnostic::new(
                    repairs::ANNOTATION_NULL,
                    "literal null member skipped in page Annots array (12.5.2)",
                )
                .in_object(page),
            )?;
            return Ok(None);
        }
        // Strict stays on the original consumer path, including its complete
        // expected-dictionary error with no newly refused repair attached.
        if self.recovery() == Recovery::Repair
            && let Value::Reference(id) = value
            && self.contains_object(*id)
        {
            let object = self.object(*id)?;
            if !object.is_stream() && object.value().is_null() {
                self.repair(
                    Diagnostic::new(
                        repairs::ANNOTATION_NULL_REFERENCE,
                        "defined null object skipped in page Annots array (12.5.2)",
                    )
                    .in_object(*id)
                    .at(object.source().position()),
                )?;
                return Ok(None);
            }
        }
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests;
