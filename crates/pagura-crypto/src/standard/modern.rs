//! Revisions 5–6: ISO 32000-2 clause 7.6, Algorithms 2.A, 2.B and 11–13.
//! Revision 5 is supported as the Adobe extension.
use pagura_core::{Error, Result};

use super::{
    Authority, FileKey, PermissionsCheck, PermissionsValidation, SASLPREP, SecurityParams, aes_key,
    equal, file_key,
};
use crate::{Aes, Sha256, sha256, sha384, sha512};

// The parent validates parameter lengths before calling this module.
// Algorithms 2.A and 2.B. All intermediate lengths are bounded: password <=127,
// user data <=48, hash <=64. The repeated plaintext occupies at most 15,296 bytes.
fn modern_hash(revision: u8, password: &[u8], salt: &[u8], user: &[u8]) -> Result<[u8; 32]> {
    let mut initial = Sha256::new();
    initial.update(password);
    initial.update(salt);
    initial.update(user);
    let initial = initial.finalize();
    if revision == 5 {
        return Ok(initial);
    }
    let mut key = [0; 64];
    key[..32].copy_from_slice(&initial);
    let mut key_len = 32;
    let mut last = 0u8;
    let mut round = 0usize;
    let mut encrypted = Vec::new();
    encrypted
        .try_reserve_exact(15_296)
        .map_err(|_| Error::limit("cannot allocate revision 6 hashing workspace"))?;
    // The last byte is at most 255: this always terminates within 287 rounds.
    while round < 64 || round < usize::from(last) + 32 {
        encrypted.clear();
        for _ in 0..64 {
            encrypted.extend_from_slice(password);
            encrypted.extend_from_slice(&key[..key_len]);
            encrypted.extend_from_slice(user);
        }
        let aes = Aes::new128(
            key[..16]
                .try_into()
                .map_err(|_| Error::syntax("invalid revision 6 hash key"))?,
        );
        let iv = key[16..32]
            .try_into()
            .map_err(|_| Error::syntax("invalid revision 6 hash IV"))?;
        aes.encrypt_cbc(iv, &mut encrypted)?;
        // 256 is 1 modulo 3: the first big-endian 128-bit integer reduces
        // to the byte sum without a wide integer conversion.
        let selector = encrypted[..16]
            .iter()
            .fold(0u16, |sum, byte| sum + u16::from(*byte))
            % 3;
        key_len = match selector {
            0 => {
                key[..32].copy_from_slice(&sha256(&encrypted));
                32
            }
            1 => {
                key[..48].copy_from_slice(&sha384(&encrypted));
                48
            }
            _ => {
                key.copy_from_slice(&sha512(&encrypted));
                64
            }
        };
        last = encrypted[encrypted.len() - 1];
        round += 1;
    }
    // Algorithm 2.B keeps the complete selected digest between rounds, then
    // takes the leftmost 256 bits (FIPS 180-4 section 7, not SHA-512/256).
    key[..32]
        .try_into()
        .map_err(|_| Error::syntax("invalid revision 6 hash"))
}
pub(super) fn authenticate(params: &SecurityParams, password: &[u8]) -> Result<FileKey> {
    if params.revision == 6 && !password.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
        return Err(Error::unsupported(
            SASLPREP,
            "revision 6 password requires unimplemented SASLprep processing",
        ));
    }
    let password = &password[..password.len().min(127)];
    let user = (
        Authority::User,
        params.user_hash.as_slice(),
        &[][..],
        &params.user_encrypted_key,
    );
    let owner = (
        Authority::Owner,
        params.owner_hash.as_slice(),
        params.user_hash.as_slice(),
        &params.owner_encrypted_key,
    );
    // Algorithm 2.A(c) gives the owner check precedence for revision 6.
    // Revision 5 is a separately supported, unverified extension.
    let candidates = if params.revision == 6 {
        [owner, user]
    } else {
        [user, owner]
    };
    for (authority, entry, user, encrypted) in candidates {
        let validation = modern_hash(params.revision, password, &entry[32..40], user)?;
        if !equal(&validation, &entry[..32]) {
            continue;
        }
        let intermediate = modern_hash(params.revision, password, &entry[40..48], user)?;
        let encrypted = encrypted
            .as_ref()
            .ok_or_else(|| Error::syntax("encrypted file key is missing"))?;
        let mut key = encrypted.clone();
        Aes::new256(&intermediate).decrypt_cbc(&[0; 16], &mut key)?;
        let validation = validate_permissions(params, &key)?;
        let mut result = file_key(params, key, authority);
        result.permissions_validation = validation;
        return Ok(result);
    }
    Err(Error::password(
        "password does not authenticate the document",
    ))
}
fn validate_permissions(params: &SecurityParams, key: &[u8]) -> Result<PermissionsValidation> {
    let encrypted = params
        .encrypted_permissions
        .as_ref()
        .ok_or_else(|| Error::syntax("encrypted permissions are missing"))?;
    let mut block: [u8; 16] = encrypted
        .as_slice()
        .try_into()
        .map_err(|_| Error::syntax("invalid encrypted permissions length"))?;
    aes_key(key)?.decrypt_block(&mut block);
    let metadata = if params.encrypt_metadata { b'T' } else { b'F' };
    let failed = if block[..4] != params.permissions.to_le_bytes() {
        Some(PermissionsCheck::Permissions)
    } else if block[4..8] != [0xff; 4] {
        Some(PermissionsCheck::ReservedBytes)
    } else if block[8] != metadata {
        Some(PermissionsCheck::EncryptMetadata)
    } else if &block[9..12] != b"adb" {
        Some(PermissionsCheck::Marker)
    } else {
        None
    };
    Ok(failed.map_or(
        PermissionsValidation::Validated,
        PermissionsValidation::Inconsistent,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::{authenticate, tests::params};
    use super::*;
    use pagura_core::ErrorKind;
    use pagura_testkit::fixture;

    #[test]
    fn algorithm_2_b_preserves_full_digests_then_truncates_leftmost_bits() {
        for length in [32, 48, 64] {
            let name = format!("r6-hash-truncation-{length}");
            let password = fixture("crypto", &format!("{name}.password"));
            let salt = fixture("crypto", &format!("{name}.salt"));
            let user = fixture("crypto", &format!("{name}.user"));
            let full = fixture("crypto", &format!("{name}.full-hash"));
            let expected = fixture("crypto", &format!("{name}.hash"));
            let selectors = fixture("crypto", &format!("{name}.selectors"));
            assert_eq!(full.len(), length);
            assert_eq!(expected, full[..32]);
            assert!((64..=287).contains(&selectors.len()));
            assert_eq!(
                length,
                [32, 48, 64][usize::from(*selectors.last().unwrap())]
            );
            for selector in 0..3 {
                assert!(selectors.contains(&selector));
            }
            assert_eq!(
                modern_hash(6, &password, &salt, &user).unwrap(),
                expected.as_slice()
            );
        }
    }

    #[test]
    fn independent_permissions_oracles_return_authenticated_keys() {
        for revision in [5, 6] {
            for outcome in [
                "validated",
                "permissions",
                "reserved-bytes",
                "encrypt-metadata",
                "marker",
                "multiple",
                "random-tail",
            ] {
                let name = format!("r{revision}-perms-{outcome}");
                let params = params(&name);
                let expected = match fixture("crypto", &format!("{name}.validation")).as_slice() {
                    b"validated" => PermissionsValidation::Validated,
                    b"permissions" => {
                        PermissionsValidation::Inconsistent(PermissionsCheck::Permissions)
                    }
                    b"reserved-bytes" => {
                        PermissionsValidation::Inconsistent(PermissionsCheck::ReservedBytes)
                    }
                    b"encrypt-metadata" => {
                        PermissionsValidation::Inconsistent(PermissionsCheck::EncryptMetadata)
                    }
                    b"marker" => PermissionsValidation::Inconsistent(PermissionsCheck::Marker),
                    _ => unreachable!(),
                };
                for (suffix, authority) in [("user", Authority::User), ("owner", Authority::Owner)]
                {
                    let password = fixture("crypto", &format!("{name}.{suffix}"));
                    let key = authenticate(&params, &password).unwrap();
                    assert_eq!(key.authority(), authority, "{name}");
                    assert_eq!(key.permissions_validation(), expected, "{name}");
                    assert_eq!(key.key, fixture("crypto", &format!("{name}.key")), "{name}");
                    let cipher = fixture("crypto", &format!("{name}.cipher"));
                    assert_eq!(
                        key.decrypt(
                            super::super::CryptMethod::AesV3,
                            pagura_core::ObjectId::new(0x0102_0304, 513),
                            &cipher
                        )
                        .unwrap(),
                        fixture("crypto", &format!("{name}.plain")),
                        "{name}"
                    );
                    // Debug exposes the check result, never the decrypted block or key.
                    assert_eq!(
                        format!("{key:?}"),
                        format!(
                            "FileKey {{ len: 32, authority: {authority:?}, revision: {revision}, permissions_validation: {expected:?} }}"
                        )
                    );
                }
                assert_eq!(
                    authenticate(&params, b"wrong-password").unwrap_err().kind,
                    ErrorKind::Password,
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn structurally_invalid_permissions_remain_syntax_errors() {
        for revision in [5, 6] {
            let good = params(&format!("r{revision}-aesv3-empty"));
            for encrypted in [None, Some(vec![]), Some(vec![0; 15]), Some(vec![0; 17])] {
                let mut invalid = good.clone();
                invalid.encrypted_permissions = encrypted;
                for password in [b"".as_slice(), b"owner-secret", b"wrong-password"] {
                    assert_eq!(
                        authenticate(&invalid, password).unwrap_err().kind,
                        ErrorKind::Syntax
                    );
                }
            }
        }
    }

    #[test]
    fn permissions_tampering_reports_the_first_failed_check() {
        for name in [
            "r5-aesv3-empty",
            "r6-aesv3-empty",
            "r6-clear-metadata-empty",
        ] {
            let good = params(name);
            let key = fixture("crypto", &format!("{name}.key"));
            let mut decrypted: [u8; 16] = good
                .encrypted_permissions
                .as_ref()
                .unwrap()
                .as_slice()
                .try_into()
                .unwrap();
            let aes = aes_key(&key).unwrap();
            aes.decrypt_block(&mut decrypted);
            for index in 0..12 {
                let mut block = decrypted;
                block[index] ^= 1;
                aes.encrypt_block(&mut block);
                let mut bad = good.clone();
                bad.encrypted_permissions = Some(block.to_vec());
                let check = match index {
                    0..=3 => PermissionsCheck::Permissions,
                    4..=7 => PermissionsCheck::ReservedBytes,
                    8 => PermissionsCheck::EncryptMetadata,
                    _ => PermissionsCheck::Marker,
                };
                assert_eq!(
                    validate_permissions(&bad, &key).unwrap(),
                    PermissionsValidation::Inconsistent(check),
                    "byte {index}"
                );
            }
            // Bytes 12..16 are random and must not be interpreted as permission flags.
            let mut block = decrypted;
            block[12..].fill(0x5a);
            aes.encrypt_block(&mut block);
            let mut changed = good;
            changed.encrypted_permissions = Some(block.to_vec());
            assert_eq!(
                validate_permissions(&changed, &key).unwrap(),
                PermissionsValidation::Validated
            );
        }
        let mut bad = params("r5-aesv3-empty");
        bad.permissions ^= 4;
        assert_eq!(
            authenticate(&bad, b"").unwrap().permissions_validation(),
            PermissionsValidation::Inconsistent(PermissionsCheck::Permissions)
        );
        let mut bad = params("r5-aesv3-empty");
        bad.encrypt_metadata = false;
        assert_eq!(
            authenticate(&bad, b"owner-secret")
                .unwrap()
                .permissions_validation(),
            PermissionsValidation::Inconsistent(PermissionsCheck::EncryptMetadata)
        );
    }
}
