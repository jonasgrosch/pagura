//! Dictionary conformance policy, ISO 32000-2 Tables 15, 20, 21 and 25.

use super::repair;
use crate::Document;
use pagura_core::{Diagnostic, Diagnostics, Dictionary, ObjectId, Position, Repair, Result, Value};

pub(super) struct Policy {
    pub diagnostics: Diagnostics,
    object: Option<ObjectId>,
    position: Position,
}

impl Policy {
    pub fn new(document: &Document, object: Option<ObjectId>) -> Self {
        Self {
            diagnostics: Diagnostics::new(document.recovery()),
            object,
            position: repair::position(document, object),
        }
    }

    pub fn repair(&mut self, tag: Repair, message: &'static str) -> Result<()> {
        // Fields in this policy share one source context. Retain one occurrence
        // per tag even when a large CF dictionary repeats the same deviation.
        if self.diagnostics.as_slice().iter().any(|item| {
            item.repair() == Some(tag)
                && item.object == self.object
                && item.position == self.position
        }) {
            return Ok(());
        }
        let mut diagnostic = Diagnostic::new(tag, message).at(self.position);
        if let Some(id) = self.object {
            diagnostic = diagnostic.in_object(id);
        }
        self.diagnostics.repair(diagnostic)
    }

    pub fn unsigned_permissions(&mut self, dictionary: &Dictionary) -> Result<Option<i32>> {
        let Some(value) = dictionary.get(b"P").and_then(Value::as_integer) else {
            return Ok(None);
        };
        if i32::try_from(value).is_ok() {
            return Ok(None);
        }
        let Ok(unsigned) = u32::try_from(value) else {
            return Ok(None);
        };
        self.repair(
            crate::repairs::PERMISSIONS_UNSIGNED,
            "unsigned 32-bit permissions reinterpreted as signed two's complement",
        )?;
        Ok(Some(i32::from_le_bytes(unsigned.to_le_bytes())))
    }

    pub fn permissions(&mut self, key: &pagura_crypto::standard::FileKey) -> Result<()> {
        use pagura_crypto::standard::{PermissionsCheck, PermissionsValidation};
        if let PermissionsValidation::Inconsistent(check) = key.permissions_validation() {
            self.repair(
                crate::repairs::PERMS_MISMATCH,
                match check {
                    PermissionsCheck::Permissions => {
                        "authenticated Perms disagrees with dictionary permissions"
                    }
                    PermissionsCheck::ReservedBytes => {
                        "authenticated Perms has inconsistent reserved bytes"
                    }
                    PermissionsCheck::EncryptMetadata => {
                        "authenticated Perms disagrees with EncryptMetadata"
                    }
                    PermissionsCheck::Marker => {
                        "authenticated Perms has an inconsistent validation marker"
                    }
                },
            )?;
        }
        Ok(())
    }

    /// Preserve the original first identifier: Algorithms 2/3 consume its exact
    /// bytes. R5/R6 do not use identifiers for key derivation. Missing legacy
    /// key material remains an error in parameter extraction, not an invented ID.
    pub fn identifiers(&mut self, trailer: &Dictionary) -> Result<()> {
        let valid = trailer
            .get(b"ID")
            .and_then(Value::as_array)
            .is_some_and(|ids| {
                ids.len() == 2
                    && ids
                        .iter()
                        .all(|id| id.as_string().is_some_and(|id| id.len() >= 16))
            });
        if !valid {
            self.repair(crate::repairs::DOCUMENT_ID,
                "required two direct file identifiers of at least 16 bytes are malformed; existing key-derivation bytes preserved")?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn permissive() -> Self {
        Self {
            diagnostics: Diagnostics::new(pagura_core::Recovery::Repair),
            object: None,
            position: Position::Unknown,
        }
    }
}
