use super::*;
use pagura_core::SignatureLimits;
use std::process::Command;

pub(crate) fn decode(hex: &str) -> Vec<u8> {
    if hex == "-" {
        return Vec::new();
    }
    assert!(hex.len().is_multiple_of(2), "non-byte hexadecimal input");
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

pub(crate) fn vectors(suite: &str) -> String {
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tools/fixturegen/rsa_oracle.py"
        ))
        .arg(suite)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn algorithm(name: &str) -> HashAlgorithm {
    match name {
        "SHA-1" | "SHA1" => HashAlgorithm::Sha1,
        "SHA-224" | "SHA224" | "SHA2-224" => HashAlgorithm::Sha224,
        "SHA-256" | "SHA256" | "SHA2-256" => HashAlgorithm::Sha256,
        "SHA-384" | "SHA384" | "SHA2-384" => HashAlgorithm::Sha384,
        "SHA-512" | "SHA512" | "SHA2-512" => HashAlgorithm::Sha512,
        _ => unreachable!("unsupported vector hash: {name}"),
    }
}

fn check_vectors(text: &str) {
    let mut counts = std::collections::BTreeMap::<(&str, &str), usize>::new();
    let mut previous = None;
    for line in text
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
    {
        let fields: Vec<_> = line.split('|').collect();
        let [
            source,
            id,
            kind,
            hash,
            mgf,
            salt,
            n,
            e,
            digest,
            signature,
            outcome,
        ] = fields.as_slice()
        else {
            unreachable!("bad vector: {line}");
        };
        if *kind == "malformed-hex" {
            assert!(!digest.len().is_multiple_of(2));
            assert_eq!(*outcome, "invalid");
            *counts
                .entry((source, "non-byte input rejected"))
                .or_default() += 1;
            continue;
        }
        if previous
            .as_ref()
            .is_none_or(|(pn, pe, _)| pn != n || pe != e)
        {
            previous = Some((
                *n,
                *e,
                RsaPublicKey::new(&decode(n), &decode(e), RsaLimits::default()).unwrap(),
            ));
        }
        let key = &previous.as_ref().unwrap().2;
        let digest = decode(digest);
        let signature = decode(signature);
        let hash = algorithm(hash);
        let result = if *kind == "v15" {
            key.verify_pkcs1v15(hash, &digest, &signature)
        } else {
            let parameters =
                PssParameters::new(hash, algorithm(mgf), salt.parse().unwrap(), 1).unwrap();
            key.verify_pss(parameters, &digest, &signature)
        };
        if *source == "RFC8017" {
            // These independently constructed signatures all have the right
            // width and an in-range representative, including bad encodings.
            assert!(result.is_ok(), "{source} {id}: {result:?}");
        }
        // Strict DER deliberately rejects every Wycheproof "acceptable" case.
        assert_eq!(
            matches!(result, Ok(Verification::Valid)),
            *outcome == "valid",
            "{source} {id} ({outcome}): {result:?}"
        );
        *counts.entry((source, outcome)).or_default() += 1;
    }
    for ((source, outcome), count) in counts {
        eprintln!("{source}: {outcome} {count} passed");
    }
}

#[test]
fn nist_minimized_vectors() {
    check_vectors(include_str!("nist-subset.txt"));
    let fields: Vec<_> = include_str!("nist-subset.txt")
        .lines()
        .find(|line| !line.starts_with('#'))
        .unwrap()
        .split('|')
        .collect();
    let key =
        RsaPublicKey::new(&decode(fields[6]), &decode(fields[7]), RsaLimits::default()).unwrap();
    // A zero representative is in range but cannot match the expected encoding.
    assert_eq!(
        key.verify_pkcs1v15(
            HashAlgorithm::Sha256,
            &[0; 32],
            &vec![0; key.signature_bytes()]
        )
        .unwrap(),
        Verification::Invalid
    );
    assert!(key.check_strength().is_ok());
}

#[test]
fn independently_encoded_padding_and_boundary_cases() {
    check_vectors(&vectors("encodings"));
}

#[test]
fn cached_wycheproof_and_nist_suites() {
    let data = vectors("rsa");
    eprintln!(
        "{}",
        data.lines()
            .filter(|line| line.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
    );
    check_vectors(&data);
}

#[test]
fn malformed_keys_and_resource_limits() {
    for (n, e) in [
        (vec![], vec![3]),
        (vec![0, 17], vec![3]),
        (vec![16], vec![3]),
        (vec![17], vec![0]),
        (vec![17], vec![1]),
        (vec![17], vec![2]),
        (vec![17], vec![17]),
        (vec![17], vec![19]),
    ] {
        assert_eq!(
            RsaPublicKey::new(&n, &e, RsaLimits::default())
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Syntax
        );
    }
    let limits = RsaLimits {
        modulus_bits: 4,
        ..RsaLimits::default()
    };
    assert_eq!(
        RsaPublicKey::new(&[17], &[3], limits).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
    let limits = RsaLimits {
        exponent_bits: 1,
        ..RsaLimits::default()
    };
    assert_eq!(
        RsaPublicKey::new(&[17], &[3], limits).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
    let key = RsaPublicKey::new(&[17], &[3], RsaLimits::default()).unwrap();
    assert!(key.check_strength().is_err());
    assert!(HashAlgorithm::Sha1.check_strength().is_err());
    assert!(HashAlgorithm::Sha256.check_strength().is_ok());
    assert!(
        key.verify_pkcs1v15(HashAlgorithm::Sha256, &[0; 31], &[1])
            .is_err()
    );
    assert!(key.recover(&[]).is_err());
    assert!(key.recover(&[17]).is_err());
}

#[test]
fn shared_limits_and_verification_outcome_contract() {
    let fields: Vec<_> = include_str!("nist-subset.txt")
        .lines()
        .find(|line| !line.starts_with('#'))
        .unwrap()
        .split('|')
        .collect();
    let n = decode(fields[6]);
    let e = decode(fields[7]);
    let limits = Limits::default();
    assert_eq!(RsaLimits::from(&limits), RsaLimits::default());
    let key = RsaPublicKey::new(&n, &e, &limits).unwrap();
    let digest = [0; 32];
    let signature = vec![0; key.signature_bytes()];
    let pss = PssParameters::new(HashAlgorithm::Sha256, HashAlgorithm::Sha256, 32, 1).unwrap();
    assert_eq!(
        key.verify_pkcs1v15(HashAlgorithm::Sha256, &digest, &signature)
            .unwrap(),
        Verification::Invalid
    );
    assert_eq!(
        key.verify_pss(pss, &digest, &signature).unwrap(),
        Verification::Invalid
    );
    for malformed in [&[][..], &n[..]] {
        assert!(
            key.verify_pkcs1v15(HashAlgorithm::Sha256, &digest, malformed)
                .is_err()
        );
        assert!(key.verify_pss(pss, &digest, malformed).is_err());
    }
    assert!(
        key.verify_pkcs1v15(HashAlgorithm::Sha256, &digest[..31], &signature)
            .is_err()
    );
    assert!(key.verify_pss(pss, &digest[..31], &signature).is_err());
    let limits = Limits {
        signatures: SignatureLimits {
            rsa_modulus_bits: u32::try_from(key.modulus_bits() - 1).unwrap(),
            ..limits.signatures
        },
        ..limits
    };
    assert_eq!(
        RsaPublicKey::new(&n, &e, &limits).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
    let limits = Limits {
        signatures: SignatureLimits {
            rsa_exponent_bits: 1,
            ..SignatureLimits::default()
        },
        ..Limits::default()
    };
    assert_eq!(
        RsaPublicKey::new(&n, &e, &limits).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
    let limits = Limits {
        signatures: SignatureLimits {
            rsa_work: 0,
            ..SignatureLimits::default()
        },
        ..Limits::default()
    };
    let key = RsaPublicKey::new(&n, &e, &limits).unwrap();
    assert_eq!(
        key.verify_pkcs1v15(HashAlgorithm::Sha256, &digest, &signature)
            .unwrap_err()
            .kind,
        pagura_core::ErrorKind::Limit
    );
    assert_eq!(
        key.verify_pss(pss, &digest, &signature).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
}

#[test]
fn verification_timing_context() {
    // Explicit opt-in: ordinary tests never perform timings.
    if std::env::var_os("PAGURA_RSA_TIMING").is_none() {
        return;
    }
    let data = vectors("timing");
    for line in data.lines().filter(|line| !line.starts_with('#')) {
        let fields: Vec<_> = line.split('|').collect();
        let [bits, n, e, digest, signature] = fields.as_slice() else {
            unreachable!()
        };
        let key = RsaPublicKey::new(&decode(n), &decode(e), RsaLimits::default()).unwrap();
        let digest = decode(digest);
        let signature = decode(signature);
        assert_eq!(
            key.verify_pkcs1v15(HashAlgorithm::Sha256, &digest, &signature)
                .unwrap(),
            Verification::Valid
        );
        for _ in 0..100 {
            assert_eq!(
                key.verify_pkcs1v15(HashAlgorithm::Sha256, &digest, &signature)
                    .unwrap(),
                Verification::Valid
            );
        }
        let start = std::time::Instant::now();
        for _ in 0..2000 {
            assert_eq!(
                key.verify_pkcs1v15(
                    HashAlgorithm::Sha256,
                    std::hint::black_box(&digest),
                    std::hint::black_box(&signature),
                )
                .unwrap(),
                Verification::Valid
            );
        }
        eprintln!(
            "RSA-{bits}: {:.3} us/verification (2000 operations, prepared key)",
            start.elapsed().as_secs_f64() * 500.0
        );
    }
}
