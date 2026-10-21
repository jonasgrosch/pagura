//! Inert signature views and byte-range structure checks (12.7.5.5, 12.8).
//!
//! This module performs no cryptography. A checked byte range is not verified
//! integrity. Original dictionaries and bytes remain borrowed and unmodified;
//! appearances are never evidence (12.7.5.5). No URLs, actions or scripts run.
//! Supported modification rules follow 12.8.2.2–12.8.2.4 and Tables 236,
//! 257 and 259. Comparisons freeze policy in the signed revision, preserve every
//! reachable use of shared objects, and retain violations later reverted.
//! `Allowed` describes structural policy only: digest verification, trust and
//! algorithm acceptance remain independent. Unknown roles, usage rights,
//! template instantiation and DSS/timestamp exceptions never grant permission.
//! Earlier signatures hidden by field removal/replacement are reported in
//! `Signatures::historical`; `history_complete` exposes inaccessible history.
//! Form discovery uses the form model's budgets. Signature inspection separately
//! shares its node/depth and estimated result-memory budgets across every entry
//! and source revision. Change analysis has a separate shared budget; exhaustion
//! is inconclusive without discarding structural or byte-range results.
//! Source rescans share a file-size work budget. Results are uncached and owned
//! by the caller; source objects use the document cache budget (ADR 0012).
mod byte_range;
mod changes;
mod classification;
mod comparison;
mod deltas;
mod equality;
mod history;
mod model;
mod modifications;
mod policy;
mod read;
mod revisions;
mod roles;
mod source;
mod transforms;

use crate::Document;
pub use byte_range::{ByteRange, ByteRangeError};
pub use changes::{
    ChangeDecision, ChangeKind, ChangeTarget, ModificationAnalysis, ModificationOutcome,
    ModificationRule, ObjectChange, ObjectRole, PermittedModifications, RevisionChanges,
};
pub use model::{
    AlgorithmStatus, FieldLock, FieldSelection, HistoricalSignature, Integrity, MdpPermission,
    RevisionCoverage, RevocationStatus, Signature, SignatureField, SignatureReference,
    SignatureResults, Signatures, SignerTrust, SubFilter, Transform, ValidationTime,
};
use pagura_core::Result;

impl Document {
    /// Inspects signature fields and the catalog's UR3 usage-rights signature.
    /// Empty fields are retained, and historical signatures remain visible after
    /// field removal/replacement. Each later update receives object-level policy
    /// comparisons against its predecessor and the signed revision. Malformed
    /// byte ranges and exhausted change-analysis budgets are per-signature results;
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
        let mut ctx = crate::view::Context::new(self);
        let mut result = read::read(self, &ctx)?;
        let facts = history::discover(self, &mut ctx, &mut result);
        if facts.is_err() {
            result.history_complete = false;
        }
        modifications::apply(self, &mut result, facts);
        Ok(result)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod change_tests;

#[cfg(test)]
mod test_pdf;
