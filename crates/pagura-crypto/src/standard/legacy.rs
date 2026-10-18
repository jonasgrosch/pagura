//! Revisions 2–4: Algorithms 1–7 (7.6.3.2, 7.6.4.3.2 and 7.6.4.4).
use pagura_core::{Error, ObjectId, Result};

use super::handler::{KeyLength, Legacy, Revision};
use super::{Authority, CryptMethod, FileKey, Key, equal};
use crate::rc4::Rc4;
use crate::secret::wipe;
use crate::{Digest, Md5};

/// Algorithm 2 step (a): the bytes that pad or replace the password.
const PADDING: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];
/// Algorithms 2 and 3: revision 3 and later rehash the MD5 output fifty times.
const KEY_REHASHES: usize = 50;
/// Algorithms 3, 5 and 7: revision 3 and later apply RC4 twenty times, each
/// key byte XORed with the iteration counter.
const RC4_ITERATIONS: u8 = 20;
/// Algorithm 6: revision 3 and later compare the first sixteen bytes of `/U`.
const USER_HASH: usize = 16;
/// Algorithm 2: revision 4 with `/EncryptMetadata false` also hashes four
/// `0xff` bytes.
const UNENCRYPTED_METADATA: [u8; 4] = [0xff; 4];

/// A revision 2–4 file key of 5 to 16 bytes, zero-filled to sixteen and
/// cleared when dropped.
#[derive(Clone)]
pub(super) struct LegacyKey {
    bytes: [u8; KeyLength::MAX],
    length: KeyLength,
}

impl LegacyKey {
    fn new(digest: [u8; 16], length: KeyLength) -> Self {
        let mut bytes = digest;
        bytes[length.get()..].fill(0);
        Self { bytes, length }
    }

    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length.get()]
    }

    /// The key zero-filled to sixteen bytes, as some writers use it to derive
    /// object keys (`FileKey::decrypt_legacy_zero_padded`).
    pub(super) const fn zero_padded(&self) -> &[u8; KeyLength::MAX] {
        &self.bytes
    }

    pub(super) const fn len(&self) -> usize {
        self.length.get()
    }
}

impl Drop for LegacyKey {
    fn drop(&mut self) {
        wipe(&mut self.bytes);
    }
}

/// Algorithms 6 and 7. The user password is tried first.
pub(super) fn authenticate(handler: &Legacy<'_>, password: &[u8]) -> Result<FileKey> {
    let user_key = file_key(handler, password);
    if user_matches(handler, &user_key) {
        return Ok(FileKey::new(
            Key::Legacy(user_key),
            Authority::User,
            handler.revision,
        ));
    }
    // Algorithm 7: decrypt /O with the owner key to recover the user password.
    let owner_key = owner_key(handler, password);
    let mut recovered = *handler.owner;
    if handler.revision == Revision::R2 {
        Rc4::new(owner_key.as_bytes()).apply_keystream(&mut recovered);
    } else {
        for iteration in (0..RC4_ITERATIONS).rev() {
            rc4_iteration(owner_key.as_bytes(), iteration, &mut recovered);
        }
    }
    let key = file_key(handler, &recovered);
    wipe(&mut recovered);
    if user_matches(handler, &key) {
        return Ok(FileKey::new(
            Key::Legacy(key),
            Authority::Owner,
            handler.revision,
        ));
    }
    Err(Error::password(
        "password does not authenticate the document",
    ))
}

/// Algorithm 2 step (a): truncates or pads the password to 32 bytes.
fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut padded = [0; 32];
    let count = password.len().min(32);
    padded[..count].copy_from_slice(&password[..count]);
    padded[count..].copy_from_slice(&PADDING[..32 - count]);
    padded
}

/// Algorithm 2 (7.6.4.3.2). The caller supplies PDFDocEncoding.
fn file_key(handler: &Legacy<'_>, password: &[u8]) -> LegacyKey {
    let mut padded = pad_password(password);
    let mut hash = Md5::new();
    hash.update(&padded);
    wipe(&mut padded);
    hash.update(handler.owner);
    hash.update(&handler.permissions.to_le_bytes());
    hash.update(handler.document_id);
    if handler.revision == Revision::R4 && !handler.encrypt_metadata {
        hash.update(&UNENCRYPTED_METADATA);
    }
    let mut digest = hash.finalize();
    if handler.revision != Revision::R2 {
        for _ in 0..KEY_REHASHES {
            digest = Md5::digest(&digest[..handler.key_length.get()]);
        }
    }
    LegacyKey::new(digest, handler.key_length)
}

/// Algorithm 3: the RC4 key that encrypts `/O`.
fn owner_key(handler: &Legacy<'_>, password: &[u8]) -> LegacyKey {
    let mut padded = pad_password(password);
    let mut digest = Md5::digest(&padded);
    wipe(&mut padded);
    if handler.revision != Revision::R2 {
        for _ in 0..KEY_REHASHES {
            digest = Md5::digest(&digest);
        }
    }
    LegacyKey::new(digest, handler.key_length)
}

/// One RC4 pass with each key byte XORed with `iteration` (Algorithms 3, 5, 7).
fn rc4_iteration(key: &[u8], iteration: u8, data: &mut [u8]) {
    let mut mixed = [0; KeyLength::MAX];
    for (out, byte) in mixed.iter_mut().zip(key) {
        *out = *byte ^ iteration;
    }
    Rc4::new(&mixed[..key.len()]).apply_keystream(data);
    wipe(&mut mixed);
}

/// Algorithms 4 and 5 as Algorithm 6 uses them: whether `key` reproduces `/U`.
fn user_matches(handler: &Legacy<'_>, key: &LegacyKey) -> bool {
    if handler.revision == Revision::R2 {
        let mut expected = PADDING;
        Rc4::new(key.as_bytes()).apply_keystream(&mut expected);
        return equal(&expected, handler.user);
    }
    let mut hash = Md5::new();
    hash.update(&PADDING);
    hash.update(handler.document_id);
    let mut expected = hash.finalize();
    for iteration in 0..RC4_ITERATIONS {
        rc4_iteration(key.as_bytes(), iteration, &mut expected);
    }
    equal(&expected, &handler.user[..USER_HASH])
}

/// Algorithm 1: the generation-aware object key for RC4 and AESV2. RC4 uses
/// its first `min(n + 5, 16)` bytes; AESV2 uses all sixteen.
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
