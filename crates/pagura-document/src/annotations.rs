//! One recovery gate for members of a page's Annots array (12.5.2).
use crate::{Document, repairs};
use pagura_core::{Diagnostic, ObjectId, Result, Value};

impl Document {
    /// The original non-null member of page `page`'s Annots array.
    ///
    /// Table 31 and 12.5.2 require annotation references. As an explicit
    /// reader policy, Repair mode reports and skips a literal null member.
    /// This does not resolve references or validate a surviving annotation;
    /// callers retain its identity, validation and original-array budgets.
    /// A reference to a null or missing object is never skipped here.
    /// This member policy does not interpret the optional Annots field itself.
    ///
    /// The diagnostic names the owning page, with unknown byte position:
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
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }
}
