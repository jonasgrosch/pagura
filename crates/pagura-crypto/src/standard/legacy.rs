//! Revisions 2–4: ISO 32000-2 clause 7.6, Algorithms 1–7.
use pagura_core::{Error, ObjectId, Result};

use super::{Authority, CryptMethod, FileKey, SecurityParams, equal, file_key};
use crate::{Md5, Rc4, md5};

// The parent validates parameter lengths before calling this module.
pub(super) fn authenticate(params: &SecurityParams, password: &[u8]) -> Result<FileKey> {
    let user_key = legacy_file_key(params, password);
    if legacy_user_matches(params, &user_key)? {
        return Ok(file_key(params, user_key, Authority::User));
    }
    let owner_key = legacy_owner_key(params, password);
    let mut recovered = params.owner_hash.clone();
    if params.revision == 2 {
        Rc4::new(&owner_key)?.apply_keystream(&mut recovered);
    } else {
        for round in (0..20).rev() {
            rc4_round(&owner_key, round, &mut recovered)?;
        }
    }
    let key = legacy_file_key(params, &recovered);
    if legacy_user_matches(params, &key)? {
        return Ok(file_key(params, key, Authority::Owner));
    }
    Err(Error::password(
        "password does not authenticate the document",
    ))
}

const PADDING: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];
fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut padded = [0; 32];
    let count = password.len().min(32);
    padded[..count].copy_from_slice(&password[..count]);
    padded[count..].copy_from_slice(&PADDING[..32 - count]);
    padded
}
// Algorithm 2, clause 7.6.3.3 in PDF 1.7; clause 7.6.4 in ISO 32000-2.
fn legacy_file_key(params: &SecurityParams, password: &[u8]) -> Vec<u8> {
    let mut hash = Md5::new();
    hash.update(&pad_password(password));
    hash.update(&params.owner_hash);
    hash.update(&params.permissions.to_le_bytes());
    hash.update(&params.document_id);
    if params.revision >= 4 && !params.encrypt_metadata {
        hash.update(&[0xff; 4]);
    }
    let mut digest = hash.finalize();
    if params.revision >= 3 {
        for _ in 0..50 {
            digest = md5(&digest[..params.key_length]);
        }
    }
    digest[..params.key_length].to_vec()
}
fn legacy_owner_key(params: &SecurityParams, password: &[u8]) -> Vec<u8> {
    let mut digest = md5(&pad_password(password));
    if params.revision >= 3 {
        for _ in 0..50 {
            digest = md5(&digest);
        }
    }
    digest[..params.key_length].to_vec()
}
fn rc4_round(key: &[u8], round: u8, data: &mut [u8]) -> Result<()> {
    let mut mixed = [0; 16];
    for (out, byte) in mixed.iter_mut().zip(key) {
        *out = *byte ^ round;
    }
    Rc4::new(&mixed[..key.len()])?.apply_keystream(data);
    Ok(())
}
fn legacy_user_matches(params: &SecurityParams, key: &[u8]) -> Result<bool> {
    if params.revision == 2 {
        let mut expected = PADDING;
        Rc4::new(key)?.apply_keystream(&mut expected);
        return Ok(equal(&expected, &params.user_hash));
    }
    let mut hash = Md5::new();
    hash.update(&PADDING);
    hash.update(&params.document_id);
    let mut expected = hash.finalize();
    for round in 0..20 {
        rc4_round(key, round, &mut expected)?;
    }
    Ok(equal(&expected, &params.user_hash[..16]))
}

// Algorithm 1: generation-aware key derivation for RC4 and AESV2.
pub(super) fn object_key(key: &[u8], method: CryptMethod, object: ObjectId) -> [u8; 16] {
    let mut digest = Md5::new();
    digest.update(key);
    digest.update(&object.number.to_le_bytes()[..3]);
    digest.update(&object.generation.to_le_bytes());
    if method == CryptMethod::AesV2 {
        digest.update(b"sAlT");
    }
    digest.finalize()
}
