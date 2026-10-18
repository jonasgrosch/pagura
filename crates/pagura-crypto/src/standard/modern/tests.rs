use super::super::SecurityParams;
use super::super::handler::{Conformance, Handler};
use super::super::tests::{key_bytes, params};
use super::super::{CryptMethod, authenticate};
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
            hash(Revision::R6, &password, &salt, &user).unwrap(),
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
            for (suffix, authority) in [("user", Authority::User), ("owner", Authority::Owner)] {
                let password = fixture("crypto", &format!("{name}.{suffix}"));
                let key = authenticate(&params, &password).unwrap();
                assert_eq!(key.authority(), authority, "{name}");
                assert_eq!(key.permissions_validation(), expected, "{name}");
                assert_eq!(
                    key_bytes(&key),
                    fixture("crypto", &format!("{name}.key")),
                    "{name}"
                );
                let cipher = fixture("crypto", &format!("{name}.cipher"));
                assert_eq!(
                    key.decrypt(
                        CryptMethod::AesV3,
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

fn modern(params: &SecurityParams) -> Modern<'_> {
    match Handler::parse(params, Conformance::Strict).unwrap() {
        Handler::Modern(handler) => handler,
        Handler::Legacy(_) => panic!("not a revision 5 or 6 handler"),
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
        let aes = Aes::new256(&key.as_slice().try_into().unwrap());
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
                validate_permissions(&modern(&bad), &aes),
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
            validate_permissions(&modern(&changed), &aes),
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
