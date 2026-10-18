//! The standard security handler (ISO 32000-2 clause 7.6.4).
//!
//! The document layer reads the `/Encrypt` dictionary and the first
//! `/ID` element into [`SecurityParams`], calls [`authenticate`] once, and
//! decrypts every string and stream with the returned [`FileKey`].

use pagura_core::{Error, ObjectId, Position, Result, ResultExt};

use crate::aes::{Aes, pkcs7_padding};
use crate::rc4::Rc4;
use crate::secret::wipe;
use handler::{Conformance, Handler, KeyLength, Revision, restricts_revision3_permissions};
use legacy::LegacyKey;

mod handler;
mod legacy;
mod modern;
mod saslprep;

/// The Algorithm 13 outcome for an authenticated file key (ISO 32000-2 clause 7.6).
///
/// Inconsistency does not invalidate password authentication. The document caller
/// decides whether to reject it or report a repair through its recovery policy.
/// Algorithm 13 either applies or not, and passes or fails, so the set is
/// closed and the enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionsValidation {
    /// Revisions 2–4 have no authenticated `/Perms` entry.
    NotApplicable,
    /// Algorithm 13 checks passed; the reserved bytes from Algorithm 10 also match.
    Validated,
    /// The password authenticated, but this check of decrypted `/Perms` failed.
    ///
    /// Checks run in byte order: permissions, reserved bytes, metadata, marker.
    /// Only the first failure is reported; no decrypted bytes or key material
    /// are exposed. The caller must apply its strict/repair policy.
    Inconsistent(PermissionsCheck),
}

/// Which Algorithm 13 consistency check failed, without disclosing decrypted
/// data. The four checks are those of Algorithm 13, so the enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionsCheck {
    /// Bytes 0–3 disagree with the little-endian `/P` value.
    Permissions,
    /// Reserved bytes 4–7 are not all `0xff`.
    ReservedBytes,
    /// Byte 8 disagrees with `/EncryptMetadata` (`T` or `F`).
    EncryptMetadata,
    /// Bytes 9–11 are not the `adb` marker.
    Marker,
}

/// Plain values from the `/Encrypt` dictionary (Tables 20 and 21) and the
/// trailer `/ID`, as the document layer read them. [`authenticate`] checks
/// them. The fields mirror those tables, and the document layer builds the
/// record with a struct literal, so it is exhaustive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityParams {
    /// `/V`: the algorithm version, 1–5.
    pub version: u8,
    /// `/R`: the handler revision, 2–6.
    pub revision: u8,
    /// File key length in bytes: 5 for V1, `/Length` / 8 for V2,
    /// 16 for V4 and 32 for V5 (clause 7.6.2, Table 20).
    pub key_length: usize,
    /// `/O`: owner password hash.
    pub owner_hash: Vec<u8>,
    /// `/U`: user password hash.
    pub user_hash: Vec<u8>,
    /// `/OE`: encrypted file key for the owner password (revisions 5 and 6).
    pub owner_encrypted_key: Option<Vec<u8>>,
    /// `/UE`: encrypted file key for the user password (revisions 5 and 6).
    pub user_encrypted_key: Option<Vec<u8>>,
    /// `/P`: raw permission bits in signed two's-complement form (Table 22).
    /// Reserved bits are preserved for hashing; readers ignore them when
    /// interpreting access rights (clause 7.6.4.2).
    pub permissions: i32,
    /// `/Perms`: encrypted permissions (revisions 5 and 6).
    pub encrypted_permissions: Option<Vec<u8>>,
    /// `/EncryptMetadata`, default true.
    pub encrypt_metadata: bool,
    /// First element of the trailer `/ID` array; empty if absent.
    pub document_id: Vec<u8>,
}

/// The cipher a crypt filter applies (clause 7.6.6, `/CFM`). Table 25 closes
/// the set of methods, so the enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CryptMethod {
    /// The standard `Identity` filter: pass bytes through (Table 26).
    /// A crypt filter with `/CFM /None` instead delegates to its handler; it
    /// must not be mapped here merely because of that method name (Table 25).
    Identity,
    /// `/V2`: RC4 with a per-object key.
    Rc4,
    /// `/AESV2`: AES-128-CBC with a per-object key.
    AesV2,
    /// `/AESV3`: AES-256-CBC with the file key.
    AesV3,
}

/// Which password authenticated the document. A standard security handler
/// has exactly a user and an owner password (7.6.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Authority {
    /// The user password (often empty).
    User,
    /// The owner password.
    Owner,
}

/// The authenticated file encryption key. It is not comparable, never
/// printed, and its key material is cleared when dropped.
#[derive(Clone)]
pub struct FileKey {
    key: Key,
    authority: Authority,
    revision: Revision,
    permissions_validation: PermissionsValidation,
}

/// The key material a revision uses.
#[derive(Clone)]
enum Key {
    /// Revisions 2–4: the file key for Algorithm 1's object keys.
    Legacy(LegacyKey),
    /// Revisions 5 and 6: AESV3 uses the file key directly, so it is expanded
    /// once instead of for every string and stream.
    Aes256(Box<Aes>),
}

/// Never prints key material.
impl std::fmt::Debug for FileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let length = match &self.key {
            Key::Legacy(key) => key.len(),
            Key::Aes256(_) => 32,
        };
        f.debug_struct("FileKey")
            .field("len", &length)
            .field("authority", &self.authority)
            .field("revision", &u8::from(self.revision))
            .field("permissions_validation", &self.permissions_validation)
            .finish()
    }
}

impl FileKey {
    const fn new(key: Key, authority: Authority, revision: Revision) -> Self {
        Self {
            key,
            authority,
            revision,
            permissions_validation: PermissionsValidation::NotApplicable,
        }
    }

    const fn with_permissions_validation(mut self, validation: PermissionsValidation) -> Self {
        self.permissions_validation = validation;
        self
    }

    /// Which password produced this key.
    pub fn authority(&self) -> Authority {
        self.authority
    }

    /// Reports Algorithm 13 consistency independently of password authentication.
    ///
    /// R5/R6 keys can report `Inconsistent`; the document caller applies its
    /// recovery policy. Revisions 2–4 report `NotApplicable`.
    pub fn permissions_validation(&self) -> PermissionsValidation {
        self.permissions_validation
    }

    /// Decrypts the string or stream data of `object` (Algorithm 1 for RC4
    /// and AESV2; the file key directly for AESV3).
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for malformed ciphertext, invalid PKCS #7 padding or
    /// a cipher incompatible with this revision; `Limit` if allocation fails.
    /// Ciphertext positions are relative to `data`, including its IV.
    pub fn decrypt(&self, method: CryptMethod, object: ObjectId, data: &[u8]) -> Result<Vec<u8>> {
        match (method, &self.key) {
            (CryptMethod::Identity, _) => copy_bytes(data),
            (CryptMethod::Rc4, Key::Legacy(key)) => {
                let mut object_key = legacy::object_key(key.as_bytes(), method, object);
                let length = (key.len() + 5).min(object_key.len());
                let output = decrypt_rc4(&object_key[..length], data);
                wipe(&mut object_key);
                output
            }
            (CryptMethod::AesV2, Key::Legacy(key))
                if self.revision == Revision::R4 && key.len() == KeyLength::MAX =>
            {
                let mut object_key = legacy::object_key(key.as_bytes(), method, object);
                let aes = Aes::new128(&object_key);
                wipe(&mut object_key);
                decrypt_aes(&aes, data)
            }
            (CryptMethod::AesV3, Key::Aes256(aes)) => decrypt_aes(aes, data),
            _ => Err(Error::syntax(
                "crypt filter is incompatible with the security handler",
            )),
        }
        .in_object(object)
    }

    /// Decrypts a legacy RC4 object whose writer zero-padded a short R4 file
    /// key to 16 bytes before MD5 object-key derivation, then used the full
    /// 16-byte RC4 object key. This is an explicit compatibility deviation from
    /// ISO 32000-2 7.6.3.2 Algorithm 1, not normal RC4 key derivation.
    /// The caller must establish the convention from authenticated structure
    /// and report its use; no private key bytes are exposed.
    ///
    /// # Errors
    ///
    /// Returns Syntax unless this is an authenticated R4 key of 5–15 bytes;
    /// returns Limit on allocation failure. Errors carry the object identity.
    pub fn decrypt_legacy_zero_padded(&self, object: ObjectId, data: &[u8]) -> Result<Vec<u8>> {
        match &self.key {
            Key::Legacy(key) if self.revision == Revision::R4 && key.len() < KeyLength::MAX => {
                let mut object_key =
                    legacy::object_key(key.zero_padded(), CryptMethod::Rc4, object);
                let output = decrypt_rc4(&object_key, data);
                wipe(&mut object_key);
                output
            }
            _ => Err(Error::syntax(
                "zero-padded object derivation requires a short R4 key",
            )),
        }
        .in_object(object)
    }
}

fn decrypt_rc4(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut output = copy_bytes(data)?;
    Rc4::new(key).apply_keystream(&mut output);
    Ok(output)
}

/// 7.6.3.2: a 16-byte IV, then CBC blocks whose plaintext ends in PKCS #7
/// padding.
fn decrypt_aes(aes: &Aes, data: &[u8]) -> Result<Vec<u8>> {
    let Some((iv, (middle, last))) = data
        .split_first_chunk::<16>()
        .and_then(|(iv, rest)| Some((iv, rest.split_last_chunk::<16>()?)))
        .filter(|(_, (middle, _))| middle.len().is_multiple_of(16))
    else {
        return Err(
            Error::syntax("AES ciphertext requires an IV and whole padded blocks")
                .at(Position::Buffer(data.len())),
        );
    };
    let mut output = buffer(middle.len() + last.len())?;
    output.extend_from_slice(middle);
    aes.decrypt_cbc(iv, output.as_chunks_mut::<16>().0);
    // The padded last block is decrypted apart, chained to the block before it.
    let mut block = *last;
    aes.decrypt_cbc(
        middle.last_chunk::<16>().unwrap_or(iv),
        std::slice::from_mut(&mut block),
    );
    let padding = pkcs7_padding(&block).within(Position::Buffer(data.len() - 16))?;
    output.extend_from_slice(&block[..16 - padding]);
    Ok(output)
}

/// Authenticates a `password` (Algorithms 6/7 or 11/12), and derives
/// the file key. Revision 5 is supported as the Adobe extension algorithm.
///
/// For revisions 2–4 the caller must supply PDFDocEncoding bytes, after host
/// codepage conversion (7.6.4.3.2(a)); this byte API does not interpret UTF-8 as
/// legacy text. The bytes are truncated/padded to 32 bytes. Revision 6 accepts
/// UTF-8, applies SASLprep mapping, normalization, output and bidi checks, then
/// truncates the prepared UTF-8 to 127 bytes (7.6.4.3.3(a–b)). NFKC uses Unicode
/// 3.2.0, including its normalization correction, without later-version changes.
/// Prohibited output and stored-string unassigned code points are rejected;
/// the latter restriction also applies to authentication (RFC 3454 section 7).
/// Revision 6 tries the owner entry first as Algorithm 2.A(c) requires; legacy
/// revisions and the unverified revision 5 extension retain user-first order.
/// Successful R5/R6 authentication also checks Algorithm 13. A mismatch in
/// decrypted `/Perms` still returns the authenticated key, with
/// [`PermissionsValidation::Inconsistent`] identifying the first failed check.
/// The document caller decides whether to reject it or report a repair; this
/// crate has no recovery policy. Structurally invalid parameters remain errors.
/// This module has no file offsets; the document layer should attach the
/// encryption dictionary's object and position to authentication errors.
///
/// # Errors
///
/// Returns [`pagura_core::ErrorKind::Password`] if neither password
/// matches or the input violates SASLprep/UTF-8 rules; `Unsupported` for
/// revisions outside 2–6 or version 3; `Syntax` for structurally
/// malformed parameters; and `Limit` if mapping or hashing allocation fails.
pub fn authenticate(params: &SecurityParams, password: &[u8]) -> Result<FileKey> {
    authenticate_with(params, password, Conformance::Strict)
}

/// Authenticates an explicitly nonconforming legacy document: either R4 with
/// a 5–15 byte RC4 key, or R2 with revision-3-only permissions restricted.
/// Normal [`authenticate`] still enforces Tables 20–22's revision/key selection.
/// The document caller must report the deviation before using this operation;
/// no password, permissions, identifier or hash bytes are inferred or changed.
/// Algorithms 2–7 otherwise apply without modification (7.6.4.3–7.6.4.4).
///
/// # Errors
///
/// As [`authenticate`], and Syntax unless one of the two documented legacy
/// deviations is present. Raw permissions bytes remain part of the file-key hash.
pub fn authenticate_legacy_compatibility(
    params: &SecurityParams,
    password: &[u8],
) -> Result<FileKey> {
    let short_key =
        params.version == 4 && params.revision == 4 && (5..16).contains(&params.key_length);
    let early_selector = params.version == 1
        && params.revision == 2
        && restricts_revision3_permissions(params.permissions);
    if !short_key && !early_selector {
        return Err(Error::syntax(
            "legacy compatibility requires an explicit revision or key-size deviation",
        ));
    }
    authenticate_with(params, password, Conformance::LegacyCompatibility)
}

fn authenticate_with(
    params: &SecurityParams,
    password: &[u8],
    conformance: Conformance,
) -> Result<FileKey> {
    match Handler::parse(params, conformance)? {
        Handler::Legacy(handler) => legacy::authenticate(&handler, password),
        Handler::Modern(handler) => modern::authenticate(&handler, password),
    }
}

/// Compares password hashes without an early exit. They are file data, not
/// secrets, so this is defence in depth rather than a timing guarantee.
fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn buffer(capacity: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(capacity)
        .map_err(|_| Error::limit("cannot allocate decrypted data"))?;
    Ok(output)
}

fn copy_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut output = buffer(data.len())?;
    output.extend_from_slice(data);
    Ok(output)
}

#[cfg(test)]
mod tests;
