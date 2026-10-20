//! Immutable transactions and incremental saves (ISO 32000-2, 7.5.6).
//!
//! Changes belong to the caller, never to a document's lazy caches. Reopen a
//! committed byte vector to create a new snapshot. This is not redaction:
//! previous values remain in the preserved original bytes.

mod forms;
mod transaction;

pub use forms::save_filled_form;
pub use transaction::Transaction;

use pagura_core::Feature;

/// Explicit treatment of signatures when appending a modified revision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignaturePolicy {
    /// Refuse present or indeterminate signatures. The safe default.
    #[default]
    Reject,
    /// Preserve every original byte, without asserting that changes are
    /// permitted by DocMDP/FieldMDP or that any signature remains valid.
    PreserveSignedBytes,
}

/// Policy for an incremental save; no host IO is performed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// Treatment of existing signatures. No-op saves need no exception.
    pub signatures: SignaturePolicy,
}

/// Stable unsupported editing capabilities.
pub mod features {
    use super::Feature;
    /// Encrypted output needs a separate encryption-on-write implementation.
    pub const ENCRYPTED: Feature = Feature::new("edit.encrypted");
    /// Sources opened in repair mode require full rewrite before modification.
    pub const REPAIRED: Feature = Feature::new("edit.repaired");
    /// Signed or uncertain input requires an explicit preservation policy.
    pub const SIGNED: Feature = Feature::new("edit.signed");
    /// A nested direct field/widget has no unambiguous writable identity.
    pub const DIRECT_FIELD: Feature = Feature::new("edit.direct-field");
    /// Password values and appearances must not be saved.
    pub const PASSWORD: Feature = Feature::new("edit.password");
    /// A button without reusable state appearances cannot yet be saved.
    pub const BUTTON_APPEARANCE: Feature = Feature::new("edit.button-appearance");
    /// Every editing capability tag.
    pub const ALL: &[Feature] = &[
        ENCRYPTED,
        REPAIRED,
        SIGNED,
        DIRECT_FIELD,
        PASSWORD,
        BUTTON_APPEARANCE,
    ];
}
