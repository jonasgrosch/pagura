//! The standard security handler (ISO 32000-2 clauses 7.6.4–7.6.5).
//!
//! The document layer reads the `/Encrypt` dictionary and the first
//! `/ID` element into [`SecurityParams`], calls [`authenticate`] once, and
//! decrypts every string and stream with the returned [`FileKey`].

use pagura_core::{Error, Feature, ObjectId, Position, Result};

use crate::{Aes, Rc4};

mod legacy;
mod modern;

/// Tag for passwords that need SASLprep (non-ASCII revision 6 passwords).
pub const SASLPREP: Feature = Feature::new("security.saslprep");

/// Stable feature tags for unsupported security algorithms.
pub mod features {
    use pagura_core::Feature;
    /// A handler revision outside the implemented standard revisions.
    pub const REVISION: Feature = Feature::new("security.revision");
    /// The unpublished version 3 encryption algorithm.
    pub const VERSION3: Feature = Feature::new("security.V3");
}

/// The Algorithm 13 outcome for an authenticated file key (ISO 32000-2 clause 7.6).
///
/// Inconsistency does not invalidate password authentication. The document caller
/// decides whether to reject it or report a repair through its recovery policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionsValidation {
    /// Revisions 2–4 have no authenticated `/Perms` entry.
    NotApplicable,
    /// Algorithm 13 verified `/P`, `/EncryptMetadata`, reserved bytes and `adb`.
    Validated,
    /// The password authenticated, but this check of decrypted `/Perms` failed.
    ///
    /// Checks run in byte order: permissions, reserved bytes, metadata, marker.
    /// Only the first failure is reported; no decrypted bytes or key material
    /// are exposed. The caller must apply its strict/repair policy.
    Inconsistent(PermissionsCheck),
}

/// Which Algorithm 13 consistency check failed, without disclosing decrypted data.
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
/// trailer `/ID`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityParams {
    /// `/V`: the algorithm version, 1–5.
    pub version: u8,
    /// `/R`: the handler revision, 2–6.
    pub revision: u8,
    /// File key length in bytes: `/Length` / 8 for V2–V4, 32 for V5.
    pub key_length: usize,
    /// `/O`: owner password hash.
    pub owner_hash: Vec<u8>,
    /// `/U`: user password hash.
    pub user_hash: Vec<u8>,
    /// `/OE`: encrypted file key for the owner password (revisions 5 and 6).
    pub owner_encrypted_key: Option<Vec<u8>>,
    /// `/UE`: encrypted file key for the user password (revisions 5 and 6).
    pub user_encrypted_key: Option<Vec<u8>>,
    /// `/P`: permission flags as a signed 32-bit integer.
    pub permissions: i32,
    /// `/Perms`: encrypted permissions (revisions 5 and 6).
    pub encrypted_permissions: Option<Vec<u8>>,
    /// `/EncryptMetadata`, default true.
    pub encrypt_metadata: bool,
    /// First element of the trailer `/ID` array; empty if absent.
    pub document_id: Vec<u8>,
}

/// The cipher a crypt filter applies (clause 7.6.6, `/CFM`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CryptMethod {
    /// `/None` or the `Identity` filter: data is not encrypted.
    Identity,
    /// `/V2`: RC4 with a per-object key.
    Rc4,
    /// `/AESV2`: AES-128-CBC with a per-object key.
    AesV2,
    /// `/AESV3`: AES-256-CBC with the file key.
    AesV3,
}

/// Which password authenticated the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Authority {
    /// The user password (often empty).
    User,
    /// The owner password.
    Owner,
}

/// The authenticated file encryption key.
#[derive(Clone, PartialEq, Eq)]
pub struct FileKey {
    key: Vec<u8>,
    authority: Authority,
    revision: u8,
    permissions_validation: PermissionsValidation,
}

/// Never prints key material.
impl std::fmt::Debug for FileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileKey")
            .field("len", &self.key.len())
            .field("authority", &self.authority)
            .field("revision", &self.revision)
            .field("permissions_validation", &self.permissions_validation)
            .finish()
    }
}

impl FileKey {
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
        self.decrypt_inner(method, object, data)
            .map_err(|error| error.in_object(object))
    }

    fn decrypt_inner(&self, method: CryptMethod, object: ObjectId, data: &[u8]) -> Result<Vec<u8>> {
        if method == CryptMethod::Identity {
            return copy_bytes(data);
        }
        let object_key;
        let key: &[u8] = match method {
            CryptMethod::Rc4 | CryptMethod::AesV2 => {
                if self.revision >= 5
                    || (method == CryptMethod::AesV2
                        && (self.revision != 4 || self.key.len() != 16))
                {
                    return Err(Error::syntax(
                        "crypt filter is incompatible with the security handler",
                    ));
                }
                object_key = legacy::object_key(&self.key, method, object);
                &object_key[..(self.key.len() + 5).min(16)]
            }
            CryptMethod::AesV3 => {
                if self.revision < 5 {
                    return Err(Error::syntax("AESV3 requires revision 5 or 6"));
                }
                &self.key
            }
            CryptMethod::Identity => return copy_bytes(data),
        };
        if method == CryptMethod::Rc4 {
            let mut output = copy_bytes(data)?;
            Rc4::new(key)?.apply_keystream(&mut output);
            return Ok(output);
        }
        let iv: &[u8; 16] = data
            .get(..16)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| {
                Error::syntax("AES ciphertext is missing its IV").at(Position::Buffer(0))
            })?;
        if data.len() < 32 || !data.len().is_multiple_of(16) {
            return Err(
                Error::syntax("AES ciphertext requires an IV and whole padded blocks")
                    .at(Position::Buffer(data.len())),
            );
        }
        let mut output = copy_bytes(&data[16..])?;
        aes_key(key)?
            .decrypt_cbc(iv, &mut output)
            .map_err(cipher_position)?;
        crate::aes::unpad_pkcs7(&mut output).map_err(cipher_position)?;
        Ok(output)
    }
}

/// Authenticates `password` as the user password, then as the owner
/// password (Algorithms 6 and 7, or 11 and 12 for revision 6), and derives
/// the file key. Revision 5 is supported as the Adobe extension algorithm.
///
/// Legacy passwords are byte strings truncated/padded to 32 bytes. For revision
/// 6, ASCII input passes unchanged; non-ASCII input needs SASLprep and is
/// explicitly unsupported. Modern prepared passwords are truncated to 127 bytes.
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
/// matches, `Unsupported` for revisions outside 2–6 or for passwords that
/// need SASLprep, `Syntax` for structurally malformed parameters,
/// and `Limit` if the bounded hashing workspace cannot be allocated.
pub fn authenticate(params: &SecurityParams, password: &[u8]) -> Result<FileKey> {
    validate_params(params)?;
    if params.revision >= 5 {
        modern::authenticate(params, password)
    } else {
        legacy::authenticate(params, password)
    }
}

fn file_key(params: &SecurityParams, key: Vec<u8>, authority: Authority) -> FileKey {
    FileKey {
        key,
        authority,
        revision: params.revision,
        permissions_validation: PermissionsValidation::NotApplicable,
    }
}
fn validate_params(params: &SecurityParams) -> Result<()> {
    if !(2..=6).contains(&params.revision) {
        return Err(Error::unsupported(
            features::REVISION,
            "security handler revision is not implemented",
        ));
    }
    if params.version == 3 {
        return Err(Error::unsupported(
            features::VERSION3,
            "encryption version 3 is unpublished",
        ));
    }
    let valid = match params.revision {
        2 => params.version == 1 && params.key_length == 5,
        3 => {
            (params.version == 1 && params.key_length == 5)
                || (params.version == 2 && (5..=16).contains(&params.key_length))
        }
        4 => params.version == 4 && (5..=16).contains(&params.key_length),
        _ => params.version == 5 && params.key_length == 32,
    };
    if !valid {
        return Err(Error::syntax(
            "incompatible encryption version, revision or key length",
        ));
    }
    let hash_length = if params.revision >= 5 { 48 } else { 32 };
    if params.owner_hash.len() != hash_length || params.user_hash.len() != hash_length {
        return Err(Error::syntax("invalid security password hash length"));
    }
    if params.revision >= 5 {
        for encrypted in [&params.owner_encrypted_key, &params.user_encrypted_key] {
            if encrypted.as_ref().is_none_or(|bytes| bytes.len() != 32) {
                return Err(Error::syntax("encrypted file key must contain 32 bytes"));
            }
        }
        if params
            .encrypted_permissions
            .as_ref()
            .is_none_or(|bytes| bytes.len() != 16)
        {
            return Err(Error::syntax("encrypted permissions must contain 16 bytes"));
        }
    }
    Ok(())
}
fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}
fn copy_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(data.len())
        .map_err(|_| Error::limit("cannot allocate decrypted data"))?;
    output.extend_from_slice(data);
    Ok(output)
}
fn aes_key(key: &[u8]) -> Result<Aes> {
    match key.len() {
        16 => Ok(Aes::new128(
            key.try_into()
                .map_err(|_| Error::syntax("invalid AES-128 key"))?,
        )),
        32 => Ok(Aes::new256(
            key.try_into()
                .map_err(|_| Error::syntax("invalid AES-256 key"))?,
        )),
        _ => Err(Error::syntax("invalid AES key length")),
    }
}
fn cipher_position(mut error: Error) -> Error {
    if let Position::Buffer(offset) = error.position {
        error.position = Position::Buffer(offset.saturating_add(16));
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::ErrorKind;
    use pagura_testkit::fixture;
    use std::collections::BTreeMap;

    pub(super) fn params(name: &str) -> SecurityParams {
        let bytes = fixture("crypto", &format!("{name}.params"));
        let text = std::str::from_utf8(&bytes).unwrap();
        let fields: BTreeMap<_, _> = text
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        let data = |name| -> Option<Vec<u8>> {
            fields.get(name).map(|hex| {
                hex.as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect()
            })
        };
        SecurityParams {
            version: fields["version"].parse().unwrap(),
            revision: fields["revision"].parse().unwrap(),
            key_length: fields["key_length"].parse().unwrap(),
            permissions: fields["permissions"].parse().unwrap(),
            encrypt_metadata: fields["encrypt_metadata"] == "1",
            document_id: data("document_id").unwrap(),
            owner_hash: data("owner_hash").unwrap(),
            user_hash: data("user_hash").unwrap(),
            owner_encrypted_key: data("owner_encrypted_key"),
            user_encrypted_key: data("user_encrypted_key"),
            encrypted_permissions: data("encrypted_permissions"),
        }
    }
    fn method(name: &str) -> CryptMethod {
        match fixture("crypto", &format!("{name}.method")).as_slice() {
            b"V2" => CryptMethod::Rc4,
            b"AESV2" => CryptMethod::AesV2,
            b"AESV3" => CryptMethod::AesV3,
            _ => unreachable!(),
        }
    }
    #[test]
    fn independent_handler_oracles_all_revisions_and_authorities() {
        for name in [
            "r2-rc4-40-empty",
            "r2-rc4-40-password",
            "r3-rc4-128-empty",
            "r3-rc4-128-password",
            "r4-aesv2-empty",
            "r4-aesv2-password",
            "r4-rc4-filter-empty",
            "r4-rc4-filter-password",
            "r4-identity-strings-empty",
            "r4-identity-strings-password",
            "r4-clear-metadata-empty",
            "r4-clear-metadata-password",
            "r5-aesv3-empty",
            "r5-aesv3-password",
            "r6-aesv3-empty",
            "r6-aesv3-password",
            "r6-clear-metadata-empty",
            "r6-clear-metadata-password",
            "r3-version1",
            "r3-key5",
            "r3-key7",
            "r4-key7",
            "r2-long-password",
            "r3-long-password",
            "r5-long-password",
            "r6-long-password",
        ] {
            let params = params(name);
            for (suffix, authority) in [("user", Authority::User), ("owner", Authority::Owner)] {
                let password = fixture("crypto", &format!("{name}.{suffix}"));
                let key = authenticate(&params, &password).unwrap();
                assert_eq!(key.authority(), authority, "{name}");
                assert_eq!(key.key, fixture("crypto", &format!("{name}.key")), "{name}");
                assert_eq!(
                    key.permissions_validation(),
                    if params.revision >= 5 {
                        PermissionsValidation::Validated
                    } else {
                        PermissionsValidation::NotApplicable
                    }
                );
                let object = ObjectId::new(0x0102_0304, 513);
                let cipher = fixture("crypto", &format!("{name}.cipher"));
                assert_eq!(
                    key.decrypt(method(name), object, &cipher).unwrap(),
                    fixture("crypto", &format!("{name}.plain")),
                    "{name}"
                );
                assert_eq!(
                    key.decrypt(CryptMethod::Identity, object, b"identity")
                        .unwrap(),
                    b"identity"
                );
                if matches!(method(name), CryptMethod::AesV2 | CryptMethod::AesV3) {
                    for size in [0u8, 1, 15, 16, 17, 31, 32] {
                        let data = fixture("crypto", &format!("{name}.size{size}"));
                        assert_eq!(
                            key.decrypt(method(name), ObjectId::new(7, 0), &data)
                                .unwrap(),
                            (0..size).collect::<Vec<_>>()
                        );
                    }
                }
            }
            assert_eq!(
                authenticate(&params, b"wrong-password").unwrap_err().kind,
                ErrorKind::Password,
                "{name}"
            );
        }
    }
    #[test]
    fn user_authentication_takes_precedence() {
        for name in ["r3-same-password", "r5-same-password"] {
            let password = fixture("crypto", &format!("{name}.user"));
            let key = authenticate(&params(name), &password).unwrap();
            assert_eq!(key.authority(), Authority::User);
            assert_eq!(key.key, fixture("crypto", &format!("{name}.key")));
        }
    }
    #[test]
    fn invalid_parameters_fail_before_slicing_or_hashing() {
        for name in [
            "r2-rc4-40-empty",
            "r3-rc4-128-empty",
            "r4-aesv2-empty",
            "r5-aesv3-empty",
            "r6-aesv3-empty",
        ] {
            let good = params(name);
            for field in 0..7 {
                let mut bad = good.clone();
                match field {
                    0 => bad.version = 0,
                    1 => bad.key_length = usize::MAX,
                    2 => bad.owner_hash.clear(),
                    3 => bad.user_hash.truncate(3),
                    4 => bad.owner_encrypted_key = None,
                    5 => bad.user_encrypted_key = Some(vec![0; 31]),
                    _ => bad.encrypted_permissions = Some(vec![0; 15]),
                }
                if field < 4 || bad.revision >= 5 {
                    assert_eq!(authenticate(&bad, b"").unwrap_err().kind, ErrorKind::Syntax);
                }
            }
        }
        let mut bad = params("r2-rc4-40-empty");
        bad.revision = 1;
        assert_eq!(
            authenticate(&bad, b"").unwrap_err().feature(),
            Some(features::REVISION)
        );
        bad.revision = 3;
        bad.version = 3;
        assert_eq!(
            authenticate(&bad, b"").unwrap_err().feature(),
            Some(features::VERSION3)
        );
        let modern = params("r6-aesv3-empty");
        assert_eq!(
            authenticate(&modern, "\u{00e9}".as_bytes())
                .unwrap_err()
                .feature(),
            Some(SASLPREP)
        );
    }
    #[test]
    fn ciphertext_errors_include_object_and_buffer_context() {
        let key = authenticate(&params("r4-aesv2-empty"), b"").unwrap();
        let object = ObjectId::new(7, 3);
        for length in 0..32 {
            let error = key
                .decrypt(CryptMethod::AesV2, object, &vec![0; length])
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax);
            assert_eq!(error.object, Some(object));
            assert!(matches!(error.position, Position::Buffer(_)));
        }
        let mut data = fixture("crypto", "r4-aesv2-empty.size0");
        // The sole block is sixteen 0x10 bytes: changing IV[0] invalidates padding.
        data[0] ^= 1;
        let error = key
            .decrypt(CryptMethod::AesV2, ObjectId::new(7, 0), &data)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.position, Position::Buffer(31));
        let legacy = authenticate(&params("r2-rc4-40-empty"), b"").unwrap();
        assert!(legacy.decrypt(CryptMethod::AesV2, object, &[]).is_err());
        assert!(legacy.decrypt(CryptMethod::AesV3, object, &[]).is_err());
        let modern = authenticate(&params("r5-aesv3-empty"), b"").unwrap();
        assert!(modern.decrypt(CryptMethod::Rc4, object, &[]).is_err());
    }
    #[test]
    fn file_key_debug_is_redacted() {
        let key = authenticate(&params("r2-rc4-40-empty"), b"").unwrap();
        assert_eq!(
            format!("{key:?}"),
            "FileKey { len: 5, authority: User, revision: 2, permissions_validation: NotApplicable }"
        );
    }
}
