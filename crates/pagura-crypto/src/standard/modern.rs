//! Revisions 5–6: Algorithms 2.A, 2.B and 11–13 (7.6.4).
//! Revision 5 is supported as the Adobe extension.
use std::borrow::Cow;
use std::ops::Range;

use pagura_core::{Error, Position, Result};

use super::handler::{Entry, Modern, Revision};
use super::{Authority, FileKey, Key, PermissionsCheck, PermissionsValidation, equal};
use crate::aes::Aes;
use crate::secret::{SecretBytes, wipe};
use crate::{Digest, Sha256, Sha384, Sha512};

/// 7.6.4.3.3(b): the prepared password is truncated to 127 bytes.
const MAX_PASSWORD: usize = 127;
/// Algorithm 2.B step (a): `K1` repeats password, `K` and user data 64 times.
const REPETITIONS: usize = 64;
/// The longest `K1`: `K` is at most a SHA-512 digest and the user data is the
/// 48-byte `/U`, so 64 × (127 + 64 + 48) = 15,296 bytes.
const ROUND_INPUT_CAPACITY: usize = REPETITIONS * (MAX_PASSWORD + 64 + 48);
/// Algorithm 2.B: steps (a)–(d) run for rounds 0 to 63 at least.
const MIN_ROUNDS: usize = 64;
/// Algorithm 2.B step (e): rounds continue while the last byte of `E`
/// exceeds the round number minus 32. A byte is at most 255, so at most 287
/// rounds run.
const ROUND_MARGIN: usize = 32;

/// Algorithm 13: the fields of the decrypted `/Perms` block.
const PERMS_PERMISSIONS: Range<usize> = 0..4;
const PERMS_RESERVED: Range<usize> = 4..8;
const PERMS_METADATA: usize = 8;
const PERMS_MARKER: Range<usize> = 9..12;

/// Algorithm 2.B (revision 6), or revision 5's single SHA-256. `user` is
/// empty for the user password and `/U` for the owner password.
fn hash(revision: Revision, password: &[u8], salt: &[u8], user: &[u8]) -> Result<[u8; 32]> {
    let mut initial = Sha256::new();
    initial.update(password);
    initial.update(salt);
    initial.update(user);
    let initial = initial.finalize();
    if revision == Revision::R5 {
        return Ok(initial);
    }
    // K is the selected digest of 32, 48 or 64 bytes; its first two blocks
    // are the AES-128 key and IV of step (b).
    let mut key = [[0u8; 16]; 4];
    key.as_flattened_mut()[..32].copy_from_slice(&initial);
    let mut key_len = 32;
    let mut last = 0u8;
    let mut round = 0;
    let mut encrypted = SecretBytes::with_capacity(ROUND_INPUT_CAPACITY)
        .ok_or_else(|| Error::limit("cannot allocate revision 6 hashing workspace"))?;
    while round < MIN_ROUNDS || round < usize::from(last) + ROUND_MARGIN {
        encrypted.clear();
        for _ in 0..REPETITIONS {
            encrypted.extend_from_slice(password);
            encrypted.extend_from_slice(&key.as_flattened()[..key_len]);
            encrypted.extend_from_slice(user);
        }
        let [aes_key, iv, ..] = &key;
        let (blocks, rest) = encrypted.as_chunks_mut::<16>();
        debug_assert!(rest.is_empty(), "64 repetitions fill whole blocks");
        Aes::new128(aes_key).encrypt_cbc(iv, blocks);
        // Step (c): 256 is 1 modulo 3, so the first 16 bytes of E as a
        // big-endian integer reduce to their byte sum modulo 3.
        let selector = encrypted
            .iter()
            .take(16)
            .fold(0u16, |sum, byte| sum + u16::from(*byte))
            % 3;
        let digest = key.as_flattened_mut();
        key_len = match selector {
            0 => {
                digest[..32].copy_from_slice(&Sha256::digest(&encrypted));
                32
            }
            1 => {
                digest[..48].copy_from_slice(&Sha384::digest(&encrypted));
                48
            }
            _ => {
                digest.copy_from_slice(&Sha512::digest(&encrypted));
                64
            }
        };
        last = encrypted.last().copied().unwrap_or_default();
        round += 1;
    }
    // Algorithm 2.B keeps the complete selected digest between rounds, then
    // takes the leftmost 256 bits (FIPS 180-4 7, not SHA-512/256).
    let mut output = [0; 32];
    output.copy_from_slice(&key.as_flattened()[..32]);
    wipe(key.as_flattened_mut());
    Ok(output)
}

/// A password check of Algorithm 11 or 12 and the key it unlocks.
struct Candidate<'a> {
    authority: Authority,
    entry: Entry<'a>,
    /// Empty for the user password; `/U` for the owner password.
    user_data: &'a [u8],
    encrypted_key: &'a [u8; 32],
}

/// 7.6.4.3.3(a): revision 6 prepares a UTF-8 password with SASLprep. The
/// result is `None` when SASLprep leaves the caller's bytes unchanged.
fn prepare(revision: Revision, password: &[u8]) -> Result<Option<SecretBytes>> {
    if revision != Revision::R6 {
        return Ok(None);
    }
    let text = std::str::from_utf8(password).map_err(|error| {
        Error::password("revision 6 password is not valid UTF-8")
            .at(Position::Buffer(error.valid_up_to()))
    })?;
    Ok(match super::saslprep::prepare(text)? {
        Cow::Borrowed(_) => None,
        Cow::Owned(prepared) => Some(SecretBytes::from_vec(prepared.into_bytes())),
    })
}

/// Algorithms 11 and 12, then Algorithm 13 for the authenticated key.
pub(super) fn authenticate(handler: &Modern<'_>, password: &[u8]) -> Result<FileKey> {
    let prepared = prepare(handler.revision, password)?;
    let password = prepared.as_deref().unwrap_or(password);
    let password = &password[..password.len().min(MAX_PASSWORD)];
    let user = Candidate {
        authority: Authority::User,
        entry: handler.user,
        user_data: &[],
        encrypted_key: handler.user_key,
    };
    let owner = Candidate {
        authority: Authority::Owner,
        entry: handler.owner,
        user_data: handler.user.bytes(),
        encrypted_key: handler.owner_key,
    };
    // Algorithm 2.A(c) gives the owner check precedence for revision 6.
    // Revision 5 is a separately supported, unverified extension.
    let candidates = if handler.revision == Revision::R6 {
        [owner, user]
    } else {
        [user, owner]
    };
    for candidate in candidates {
        let validation = hash(
            handler.revision,
            password,
            candidate.entry.validation_salt(),
            candidate.user_data,
        )?;
        if !equal(&validation, candidate.entry.hash()) {
            continue;
        }
        let mut intermediate = hash(
            handler.revision,
            password,
            candidate.entry.key_salt(),
            candidate.user_data,
        )?;
        let mut file_key = *candidate.encrypted_key;
        Aes::new256(&intermediate).decrypt_cbc(&[0; 16], file_key.as_chunks_mut::<16>().0);
        wipe(&mut intermediate);
        let aes = Aes::new256(&file_key);
        wipe(&mut file_key);
        let validation = validate_permissions(handler, &aes);
        return Ok(FileKey::new(
            Key::Aes256(Box::new(aes)),
            candidate.authority,
            handler.revision,
        )
        .with_permissions_validation(validation));
    }
    Err(Error::password(
        "password does not authenticate the document",
    ))
}

/// Algorithm 13: decrypts `/Perms` with the file key and reports the first
/// failed check, in byte order.
fn validate_permissions(handler: &Modern<'_>, key: &Aes) -> PermissionsValidation {
    let mut block = *handler.encrypted_permissions;
    key.decrypt_block(&mut block);
    let metadata = if handler.encrypt_metadata { b'T' } else { b'F' };
    let failed = if block[PERMS_PERMISSIONS] != handler.permissions.to_le_bytes() {
        Some(PermissionsCheck::Permissions)
    } else if block[PERMS_RESERVED] != [0xff; 4] {
        Some(PermissionsCheck::ReservedBytes)
    } else if block[PERMS_METADATA] != metadata {
        Some(PermissionsCheck::EncryptMetadata)
    } else if block[PERMS_MARKER] != *b"adb" {
        Some(PermissionsCheck::Marker)
    } else {
        None
    };
    wipe(&mut block);
    failed.map_or(
        PermissionsValidation::Validated,
        PermissionsValidation::Inconsistent,
    )
}

#[cfg(test)]
mod tests;
