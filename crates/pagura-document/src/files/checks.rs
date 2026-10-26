//! Explicit checks of file-parameter declarations against decoded bytes (Table 45).

use super::EmbeddedFile;
use crate::source::ReadResult;
use pagura_core::convert::usize_to_u64;
use pagura_crypto::{Digest, Md5};

/// Comparison of one declared embedded-file parameter with the decoded payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParameterCheck {
    /// No value was declared; no successful comparison is implied.
    NotDeclared,
    /// The declaration matches the decoded bytes.
    Matches,
    /// The declaration differs from the decoded bytes.
    Mismatch,
}

/// Size and checksum observations for one completely decoded embedded file.
/// These compare producer declarations; they do not authenticate a file or
/// validate its content format, dates or associated-file conformance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FileParameterChecks {
    /// Actual uncompressed byte length, independent of the declared Size.
    pub decoded_size: u64,
    /// Comparison against Params/Size, when present.
    pub size: ParameterCheck,
    /// MD5 comparison against Params/CheckSum, when present (Table 45).
    pub checksum: ParameterCheck,
}

impl EmbeddedFile<'_> {
    /// Decodes this payload and checks Params/Size and Params/CheckSum (7.11.4.1,
    /// Table 45). CheckSum applies to the uncompressed bytes, after decryption
    /// and every filter. Missing declarations remain [`ParameterCheck::NotDeclared`].
    ///
    /// Mismatches are returned as observations, not repaired, hidden or turned
    /// into stream errors. Source bytes and declarations remain unchanged.
    /// Decoding uses the document's stream limits and attachment authorization.
    /// The fixed-storage checksum runs once over the bounded decoded bytes only
    /// when a checksum is declared. The payload is not retained by this result.
    ///
    /// # Errors
    /// As [`Self::data`]. Even without declarations, the complete payload must
    /// decode successfully before its actual length is reported.
    pub fn check_parameters(&self) -> ReadResult<FileParameterChecks> {
        let bytes = self.data()?;
        let decoded_size = usize_to_u64(bytes.len());
        let parameters = self.parameters();
        let size = match parameters.and_then(|parameters| parameters.size) {
            None => ParameterCheck::NotDeclared,
            Some(size) if size == decoded_size => ParameterCheck::Matches,
            Some(_) => ParameterCheck::Mismatch,
        };
        let checksum = match parameters.and_then(|parameters| parameters.checksum) {
            None => ParameterCheck::NotDeclared,
            Some(expected) if *expected == Md5::digest(&bytes) => ParameterCheck::Matches,
            Some(_) => ParameterCheck::Mismatch,
        };
        Ok(FileParameterChecks {
            decoded_size,
            size,
            checksum,
        })
    }
}
