//! Parsing [`SecurityParams`] into a typed handler (Tables 20–22).
//!
//! The version, revision and key length are checked together, and every
//! string becomes a fixed-size array, once. The algorithms in `legacy` and
//! `modern` then read only fixed-size arrays and have no length errors.

use pagura_core::{Error, Result};

use super::SecurityParams;
use crate::features;

/// Table 21's `/R` values that this handler implements. Revision 5 is the
/// unpublished Adobe extension that ISO 32000-2 deprecates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Revision {
    R2,
    R3,
    R4,
    R5,
    R6,
}

impl TryFrom<u8> for Revision {
    type Error = Error;

    fn try_from(revision: u8) -> Result<Self> {
        match revision {
            2 => Ok(Self::R2),
            3 => Ok(Self::R3),
            4 => Ok(Self::R4),
            5 => Ok(Self::R5),
            6 => Ok(Self::R6),
            _ => Err(Error::unsupported(
                features::REVISION,
                "security handler revision is not implemented",
            )),
        }
    }
}

impl From<Revision> for u8 {
    fn from(revision: Revision) -> Self {
        match revision {
            Revision::R2 => 2,
            Revision::R3 => 3,
            Revision::R4 => 4,
            Revision::R5 => 5,
            Revision::R6 => 6,
        }
    }
}

/// Whether Tables 20–22's revision and key-size rules apply in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Conformance {
    Strict,
    /// Also accepts the two documented legacy deviations: an R4 RC4 key of
    /// 5–15 bytes, and R2 with revision-3-only permissions restricted.
    LegacyCompatibility,
}

/// Table 22 bits 9, 11 and 12 (1-based): the permissions that only
/// revision 3 or later can restrict. Obsolete bit 10 is ignored by readers.
const REVISION3_PERMISSIONS: u32 = (1 << 8) | (1 << 10) | (1 << 11);

/// Whether `/P` restricts a permission that only revision 3 or later defines
/// (Table 21's `/R` selector).
pub(super) const fn restricts_revision3_permissions(permissions: i32) -> bool {
    permissions.cast_unsigned() & REVISION3_PERMISSIONS != REVISION3_PERMISSIONS
}

/// A revision 2–4 file key length in bytes: 5 to 16 (Table 20 `/Length`,
/// 40 to 128 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct KeyLength(usize);

impl KeyLength {
    pub(super) const MAX: usize = 16;

    const fn new(length: usize) -> Option<Self> {
        if 5 <= length && length <= Self::MAX {
            Some(Self(length))
        } else {
            None
        }
    }

    pub(super) const fn get(self) -> usize {
        self.0
    }
}

/// A revision 2–4 handler (Algorithms 2–7).
#[derive(Debug)]
pub(super) struct Legacy<'a> {
    pub(super) revision: Revision,
    pub(super) key_length: KeyLength,
    /// `/O`.
    pub(super) owner: &'a [u8; 32],
    /// `/U`.
    pub(super) user: &'a [u8; 32],
    pub(super) permissions: i32,
    pub(super) encrypt_metadata: bool,
    pub(super) document_id: &'a [u8],
}

/// A revision 5 or 6 `/O` or `/U` string (Algorithms 8 and 9): a 32-byte
/// hash, an 8-byte validation salt and an 8-byte key salt.
#[derive(Debug, Clone, Copy)]
pub(super) struct Entry<'a>(&'a [u8; 48]);

impl<'a> Entry<'a> {
    pub(super) const fn bytes(self) -> &'a [u8; 48] {
        self.0
    }

    pub(super) fn hash(self) -> &'a [u8] {
        &self.0[..32]
    }

    pub(super) fn validation_salt(self) -> &'a [u8] {
        &self.0[32..40]
    }

    pub(super) fn key_salt(self) -> &'a [u8] {
        &self.0[40..]
    }
}

/// A revision 5 or 6 handler (Algorithms 2.A, 2.B and 11–13).
#[derive(Debug)]
pub(super) struct Modern<'a> {
    pub(super) revision: Revision,
    pub(super) owner: Entry<'a>,
    pub(super) user: Entry<'a>,
    /// `/OE`.
    pub(super) owner_key: &'a [u8; 32],
    /// `/UE`.
    pub(super) user_key: &'a [u8; 32],
    /// `/Perms`.
    pub(super) encrypted_permissions: &'a [u8; 16],
    pub(super) permissions: i32,
    pub(super) encrypt_metadata: bool,
}

#[derive(Debug)]
pub(super) enum Handler<'a> {
    Legacy(Legacy<'a>),
    Modern(Modern<'a>),
}

impl<'a> Handler<'a> {
    /// Checks Tables 20–22's version, revision and key-length combination
    /// and every string length.
    ///
    /// # Errors
    /// `Unsupported` for a revision outside 2–6 or version 3; otherwise
    /// `Syntax` for an incompatible combination or a wrong length.
    pub(super) fn parse(params: &'a SecurityParams, conformance: Conformance) -> Result<Self> {
        let revision = Revision::try_from(params.revision)?;
        if params.version == 3 {
            return Err(Error::unsupported(
                features::VERSION3,
                "encryption version 3 is unpublished",
            ));
        }
        match revision {
            Revision::R5 | Revision::R6 => Modern::parse(params, revision).map(Self::Modern),
            _ => Legacy::parse(params, revision, conformance).map(Self::Legacy),
        }
    }
}

fn incompatible() -> Error {
    Error::syntax("incompatible encryption version, revision or key length")
}

fn hash_length() -> Error {
    Error::syntax("invalid security password hash length")
}

impl<'a> Legacy<'a> {
    fn parse(
        params: &'a SecurityParams,
        revision: Revision,
        conformance: Conformance,
    ) -> Result<Self> {
        let compatible = conformance == Conformance::LegacyCompatibility;
        let later_restrictions = restricts_revision3_permissions(params.permissions);
        let key_length = KeyLength::new(params.key_length).ok_or_else(incompatible)?;
        let (version, length) = (params.version, key_length.get());
        let valid = match revision {
            Revision::R2 => version == 1 && length == 5 && (!later_restrictions || compatible),
            Revision::R3 => (version == 1 && length == 5 && later_restrictions) || version == 2,
            _ => version == 4 && (length == KeyLength::MAX || compatible),
        };
        if !valid {
            return Err(incompatible());
        }
        Ok(Self {
            revision,
            key_length,
            owner: params
                .owner_hash
                .as_slice()
                .try_into()
                .map_err(|_| hash_length())?,
            user: params
                .user_hash
                .as_slice()
                .try_into()
                .map_err(|_| hash_length())?,
            permissions: params.permissions,
            encrypt_metadata: params.encrypt_metadata,
            document_id: &params.document_id,
        })
    }
}

impl<'a> Modern<'a> {
    fn parse(params: &'a SecurityParams, revision: Revision) -> Result<Self> {
        if params.version != 5 || params.key_length != 32 {
            return Err(incompatible());
        }
        let entry = |bytes: &'a [u8]| bytes.try_into().map(Entry).map_err(|_| hash_length());
        let key = |bytes: &'a Option<Vec<u8>>| {
            bytes
                .as_deref()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| Error::syntax("encrypted file key must contain 32 bytes"))
        };
        Ok(Self {
            revision,
            owner: entry(&params.owner_hash)?,
            user: entry(&params.user_hash)?,
            owner_key: key(&params.owner_encrypted_key)?,
            user_key: key(&params.user_encrypted_key)?,
            encrypted_permissions: params
                .encrypted_permissions
                .as_deref()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| Error::syntax("encrypted permissions must contain 16 bytes"))?,
            permissions: params.permissions,
            encrypt_metadata: params.encrypt_metadata,
        })
    }
}
