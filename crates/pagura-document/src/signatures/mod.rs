//! Inert signature views and byte-range structure checks (12.7.5.5, 12.8).
//!
//! This module performs no cryptography. A checked byte range is not verified
//! integrity. Original dictionaries and bytes remain borrowed and unmodified;
//! appearances are never evidence (12.7.5.5). No URLs, actions or scripts run.
//! Form discovery uses the form model's budgets. Signature inspection separately
//! shares its node/depth and estimated result-memory budgets across every entry.
//! Source rescans share a file-size work budget. Results are uncached and owned
//! by the caller; source objects use the document cache budget (ADR 0012).
mod byte_range;
mod model;
mod read;
mod revisions;
mod source;
mod transforms;

use crate::Document;
pub use byte_range::{ByteRange, ByteRangeError};
pub use model::{
    AlgorithmStatus, FieldLock, FieldSelection, Integrity, MdpPermission, PermittedModifications,
    RevisionCoverage, RevocationStatus, Signature, SignatureField, SignatureReference,
    SignatureResults, Signatures, SignerTrust, SubFilter, Transform, ValidationTime,
};
use pagura_core::Result;

impl Document {
    /// Inspects signature fields and the catalog's UR3 usage-rights signature.
    /// Empty fields are retained. Malformed byte ranges are per-signature results;
    /// no signature is reported as cryptographically verified (ADR 0015).
    /// Malformed typed signatures and locks have per-field errors. Catalog
    /// certification and usage-rights signatures have separate error slots.
    ///
    /// # Errors
    /// Returns discovery errors (including unreadable form values) and resource
    /// limits; errors within discovered signature dictionaries are retained.
    /// Form repairs follow the form reader's policy;
    /// signature source syntax is checked strictly and never repaired.
    pub fn signatures(&self) -> Result<Signatures<'_>> {
        read::read(self)
    }
}

#[cfg(test)]
mod tests;
