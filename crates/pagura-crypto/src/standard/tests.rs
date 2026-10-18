use super::*;
use crate::features;
use pagura_core::ErrorKind;
use pagura_testkit::{fixture, fixture_text, unhex};
use std::collections::BTreeMap;

/// A change that makes parameters invalid.
type Mutation = fn(&mut SecurityParams);

pub(super) fn params(name: &str) -> SecurityParams {
    params_in("crypto", name)
}

fn params_in(area: &str, name: &str) -> SecurityParams {
    let text = fixture_text(area, &format!("{name}.params"));
    let fields: BTreeMap<_, _> = text
        .lines()
        .map(|line| line.split_once('=').unwrap())
        .collect();
    let data = |name| fields.get(name).map(|hex| unhex(hex));
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
/// The raw file key, for comparison with the independent oracles.
pub(super) fn key_bytes(key: &FileKey) -> Vec<u8> {
    match &key.key {
        Key::Legacy(key) => key.as_bytes().to_vec(),
        Key::Aes256(aes) => aes.cipher_key().to_vec(),
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
fn non_ascii_user_and_owner_authenticate_after_complete_saslprep() {
    for name in [
        "r6-saslprep-unicode-passwords",
        "r6-saslprep-bidi-passwords",
        "r6-saslprep-normalized-boundary",
        "r6-saslprep-correction-passwords",
    ] {
        let parameters = params(name);
        let expected = fixture("crypto", &format!("{name}.key"));
        for (suffix, authority) in [("user", Authority::User), ("owner", Authority::Owner)] {
            let password = fixture("crypto", &format!("{name}.{suffix}"));
            let prepared = saslprep::prepare(std::str::from_utf8(&password).unwrap()).unwrap();
            assert_eq!(
                &prepared.as_bytes()[..prepared.len().min(127)],
                fixture("crypto", &format!("{name}.prepared-{suffix}")),
                "{name}, {suffix}"
            );
            let key = authenticate(&parameters, &password).unwrap();
            assert_eq!(key.authority(), authority, "{name}, {suffix}");
            assert!(key_bytes(&key) == expected, "{name}, {suffix} key mismatch");
            assert_eq!(
                key.permissions_validation(),
                PermissionsValidation::Validated
            );
        }
    }
}

#[test]
fn nonstandard_short_rc4_keys_require_the_explicit_compatibility_apis() {
    let object = ObjectId::new(0x0102_0304, 513);
    for name in ["r4-zero-padded-40", "r4-zero-padded-48"] {
        let params = params_in("document", name);
        assert_eq!(
            authenticate(&params, b"").unwrap_err().kind,
            ErrorKind::Syntax
        );
        let cipher = fixture("document", &format!("{name}.cipher"));
        let plain = fixture("document", &format!("{name}.plain"));
        for (password, authority) in [
            (b"".as_slice(), Authority::User),
            (b"owner".as_slice(), Authority::Owner),
        ] {
            let key = authenticate_legacy_compatibility(&params, password).unwrap();
            assert_eq!(key.authority(), authority);
            assert_eq!(key_bytes(&key), fixture("document", &format!("{name}.key")));
            assert_eq!(
                key.decrypt_legacy_zero_padded(object, &cipher).unwrap(),
                plain
            );
            assert_ne!(
                key.decrypt(CryptMethod::Rc4, object, &cipher).unwrap(),
                plain
            );
            assert_ne!(
                key.decrypt_legacy_zero_padded(ObjectId::new(object.number, 514), &cipher)
                    .unwrap(),
                plain
            );
        }
        assert_eq!(
            authenticate_legacy_compatibility(&params, b"wrong")
                .unwrap_err()
                .kind,
            ErrorKind::Password
        );
    }
    for name in [
        "r2-rc4-40-empty",
        "r3-rc4-128-empty",
        "r4-aesv2-empty",
        "r6-aesv3-empty",
    ] {
        let params = params(name);
        assert_eq!(
            authenticate_legacy_compatibility(&params, b"")
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
        let key = authenticate(&params, b"").unwrap();
        let error = key
            .decrypt_legacy_zero_padded(object, b"ciphertext")
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.object, Some(object));
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
            assert_eq!(
                key_bytes(&key),
                fixture("crypto", &format!("{name}.key")),
                "{name}"
            );
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
fn dictionary_and_preprocessing_rejections_match_independent_oracles() {
    for name in [
        "r4-key7",
        "r2-revision-permissions",
        "r3-version1-permissions",
        "r4-fixed-key-5",
        "r4-fixed-key-15",
        "r6-saslprep-ascii-control",
        "r6-saslprep-control-after-truncation",
        "r6-saslprep-unicode",
        "r6-saslprep-invalid-utf8",
        "r6-saslprep-invalid-utf8-after-truncation",
    ] {
        let parameters = params(name);
        let password = fixture("crypto", &format!("{name}.password"));
        let expected = fixture("crypto", &format!("{name}.error"));
        let error = authenticate(&parameters, &password).unwrap_err();
        if expected == b"syntax" {
            assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
        } else if expected == b"password" {
            assert_eq!(error.kind, ErrorKind::Password, "{name}");
        } else {
            panic!("unknown expected rejection for {name}");
        }
    }
}
#[test]
fn pdfdocencoding_and_raw_permission_bits_match_independent_oracles() {
    for name in [
        "r3-pdfdocencoding-euro",
        "r3-permissions-reserved-low",
        "r3-permissions-reserved-high",
        "r3-permissions-accessibility-clear",
        "r3-user-hash-tail",
    ] {
        let parameters = params(name);
        for (suffix, authority) in [("user", Authority::User), ("owner", Authority::Owner)] {
            let password = fixture("crypto", &format!("{name}.{suffix}"));
            let key = authenticate(&parameters, &password).unwrap();
            assert_eq!(key.authority(), authority, "{name}");
            assert!(
                key_bytes(&key) == fixture("crypto", &format!("{name}.key")),
                "file key oracle mismatch: {name}"
            );
        }
    }
}
#[test]
fn revision6_shared_password_has_owner_authority() {
    let name = "r6-owner-precedence";
    let parameters = params(name);
    let password = fixture("crypto", &format!("{name}.user"));
    let key = authenticate(&parameters, &password).unwrap();
    assert_eq!(key.authority(), Authority::Owner);
    assert!(
        key_bytes(&key) == fixture("crypto", &format!("{name}.key")),
        "file key oracle mismatch"
    );
}
#[test]
fn saslprep_mapping_authenticates_before_truncating_password_bytes() {
    for name in [
        "r6-saslprep-mapping-removal",
        "r6-saslprep-mapping-space",
        "r6-saslprep-mapping-empty",
        "r6-saslprep-mapping-before-truncation",
    ] {
        let parameters = params(name);
        let password = fixture("crypto", &format!("{name}.user"));
        let prepared = fixture("crypto", &format!("{name}.prepared"));
        let mapped = saslprep::prepare(std::str::from_utf8(&password).unwrap()).unwrap();
        assert_eq!(mapped.as_bytes(), prepared, "{name}");
        let key = authenticate(&parameters, &password).unwrap();
        assert_eq!(key.authority(), Authority::User, "{name}");
        assert!(
            key_bytes(&key) == fixture("crypto", &format!("{name}.key")),
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
        assert_eq!(key_bytes(&key), fixture("crypto", &format!("{name}.key")));
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
        // Each mutation, and whether it also breaks a revision 2-4 handler.
        let mutations: [(Mutation, bool); 7] = [
            (|bad| bad.version = 0, true),
            (|bad| bad.key_length = usize::MAX, true),
            (|bad| bad.owner_hash.clear(), true),
            (|bad| bad.user_hash.truncate(3), true),
            (|bad| bad.owner_encrypted_key = None, false),
            (|bad| bad.user_encrypted_key = Some(vec![0; 31]), false),
            (|bad| bad.encrypted_permissions = Some(vec![0; 15]), false),
        ];
        for (index, (mutate, breaks_legacy)) in mutations.into_iter().enumerate() {
            let mut bad = good.clone();
            mutate(&mut bad);
            if breaks_legacy || bad.revision >= 5 {
                let error = authenticate(&bad, b"").unwrap_err();
                assert_eq!(error.kind, ErrorKind::Syntax, "{name} mutation {index}");
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
            .kind,
        ErrorKind::Password
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
