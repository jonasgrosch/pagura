use super::*;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

fn hex(value: &str) -> Vec<u8> {
    let padded = if value.len() % 2 == 1 {
        format!("0{value}")
    } else {
        value.to_owned()
    };
    padded
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn uint(value: &str) -> Uint {
    Uint::from_be_bytes(&hex(value)).unwrap()
}

fn curve(value: &str) -> Curve {
    match value {
        "P256" => Curve::P256,
        "P384" => Curve::P384,
        "P521" => Curve::P521,
        _ => panic!("unknown test curve {value}"),
    }
}

fn python(script: &str, arguments: &[&str]) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/ecdsa/tests")
        .join(script);
    let output = Command::new("python3")
        .arg(path)
        .args(arguments)
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

#[test]
fn modular_arithmetic_matches_independent_python_integers() {
    let output = python("oracle.py", &["arithmetic"]);
    let mut count = 0;
    for line in output.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        let parameters = curve(fields[0]).parameters();
        let modulus = if fields[1] == "field" {
            &parameters.field
        } else {
            &parameters.scalar
        };
        let a = uint(fields[2]);
        let b = uint(fields[3]);
        let am = modulus.encode(a);
        let bm = modulus.encode(b);
        assert_eq!(modulus.decode(am), a, "encode/decode: {line}");
        assert_eq!(
            modulus.decode(modulus.add(am, bm)),
            uint(fields[4]),
            "add: {line}"
        );
        assert_eq!(
            modulus.decode(modulus.sub(am, bm)),
            uint(fields[5]),
            "sub: {line}"
        );
        assert_eq!(
            modulus.decode(modulus.mul(am, bm)),
            uint(fields[6]),
            "mul: {line}"
        );
        let inverse = modulus.inverse(am);
        if a.is_zero() {
            assert_eq!(inverse, None);
        } else {
            let inverse = inverse.unwrap();
            assert_eq!(modulus.decode(inverse), uint(fields[7]), "inverse: {line}");
            assert_eq!(modulus.mul(inverse, am), modulus.one);
        }
        count += 1;
    }
    assert_eq!(count, 6 * (49 + 256));
    eprintln!("Python integer arithmetic: {count}/{count} cases, six modular operations each");
}

fn verify_vectors(output: &str) -> usize {
    let mut counts: BTreeMap<_, (usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields.len(), 7, "{line}");
        let curve = curve(fields[2]);
        let key = PublicKey::from_sec1(curve, &hex(fields[4]));
        let actual = key.and_then(|key| key.verify_der(&hex(fields[3]), &hex(fields[5])));
        let accepted = matches!(actual, Ok(true));
        let expected = fields[6] == "1";
        let entry = counts.entry((fields[0], fields[2])).or_default();
        entry.0 += usize::from(accepted == expected);
        entry.1 += 1;
        if accepted != expected {
            failures.push(format!(
                "{} case {} {} expected {expected}, got {actual:?}",
                fields[0], fields[1], fields[2]
            ));
        }
    }
    let mut total = 0;
    for ((suite, curve), (passed, count)) in counts {
        eprintln!("{suite} {curve}: {passed}/{count}");
        total += count;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    total
}

#[test]
fn random_signatures_match_independent_affine_python_oracle() {
    assert_eq!(
        verify_vectors(&python("oracle.py", &["vectors"])),
        3 * (4 * 64 + 2)
    );
}

#[test]
fn complete_cached_nist_and_wycheproof_signature_suites() {
    let output = python("vectors.py", &[]);
    let count = verify_vectors(&output);
    eprintln!(
        "Cached ECDSA reference vectors: {count} (cache is optional; independent oracles always run)"
    );
}

fn coordinate_bytes(value: Uint, size: usize) -> Vec<u8> {
    let bytes: Vec<_> = value
        .0
        .iter()
        .rev()
        .flat_map(|word| word.to_be_bytes())
        .collect();
    bytes[bytes.len() - size..].to_vec()
}

fn generator_key(curve: Curve, compressed: bool) -> Vec<u8> {
    let parameters = curve.parameters();
    let mut out = vec![if compressed {
        2 | u8::try_from(parameters.gy.0[0] & 1).unwrap()
    } else {
        4
    }];
    out.extend(coordinate_bytes(parameters.gx, curve.coordinate_bytes()));
    if !compressed {
        out.extend(coordinate_bytes(parameters.gy, curve.coordinate_bytes()));
    }
    out
}

#[test]
fn sec1_validation_and_group_exceptional_cases() {
    for curve in [Curve::P256, Curve::P384, Curve::P521] {
        let parameters = curve.parameters();
        let generator = parameters.generator();
        assert_eq!(
            generator.affine_coordinates(parameters),
            Some((parameters.gx, parameters.gy))
        );
        assert_eq!(Point::IDENTITY.add(generator, parameters), generator);
        assert_eq!(generator.add(Point::IDENTITY, parameters), generator);
        assert_eq!(
            generator.add(generator, parameters),
            generator.double(parameters)
        );
        let negative = Point {
            y: parameters.field.sub(Uint::ZERO, generator.y),
            ..generator
        };
        assert!(generator.add(negative, parameters).z.is_zero());
        assert!(
            generator
                .double_scalar(
                    Uint::ZERO,
                    parameters.scalar.value,
                    parameters,
                    curve.bits()
                )
                .z
                .is_zero()
        );
        let uncompressed = generator_key(curve, false);
        let compressed = generator_key(curve, true);
        let key = PublicKey::from_sec1(curve, &uncompressed).unwrap();
        assert_eq!(key.curve(), curve);
        assert_eq!(key, PublicKey::from_sec1(curve, &compressed).unwrap());
        let mut opposite = compressed.clone();
        opposite[0] ^= 1;
        assert_eq!(
            PublicKey::from_sec1(curve, &opposite).unwrap().point,
            negative
        );
        for prefix in [0, 1, 5, 6, 7, 255] {
            let mut malformed = uncompressed.clone();
            malformed[0] = prefix;
            assert!(PublicKey::from_sec1(curve, &malformed).is_err());
        }
        for length in 0..uncompressed.len() {
            assert!(PublicKey::from_sec1(curve, &uncompressed[..length]).is_err());
        }
        let mut trailing = uncompressed.clone();
        trailing.push(0);
        assert!(PublicKey::from_sec1(curve, &trailing).is_err());
        let mut off_curve = uncompressed.clone();
        *off_curve.last_mut().unwrap() ^= 1;
        assert!(PublicKey::from_sec1(curve, &off_curve).is_err());
        for coordinate in 0..2 {
            let mut out_of_range = uncompressed.clone();
            let size = curve.coordinate_bytes();
            out_of_range[1 + coordinate * size..=(coordinate + 1) * size]
                .copy_from_slice(&coordinate_bytes(parameters.field.value, size));
            assert!(PublicKey::from_sec1(curve, &out_of_range).is_err());
        }
        let mut zeros = vec![0; uncompressed.len()];
        zeros[0] = 4;
        assert!(PublicKey::from_sec1(curve, &zeros).is_err());
        if curve == Curve::P521 {
            let mut padding = uncompressed;
            padding[1] |= 2;
            assert!(PublicKey::from_sec1(curve, &padding).is_err());
        }
        // Find a quadratic nonresidue in the decompression equation.
        let f = &parameters.field;
        let mut x = Uint::ZERO;
        while f.sqrt(parameters.equation(f.encode(x))).is_some() {
            x = f.add(x, Uint::ONE);
        }
        let mut nonsquare = vec![2];
        nonsquare.extend(coordinate_bytes(x, curve.coordinate_bytes()));
        assert!(PublicKey::from_sec1(curve, &nonsquare).is_err());
    }
}

#[test]
fn strict_der_rejects_lengths_types_signs_padding_ranges_and_trailing_bytes() {
    let valid = hex("3006020101020101");
    for curve in [Curve::P256, Curve::P384, Curve::P521] {
        let signature = Signature::from_der(curve, &valid).unwrap();
        assert_eq!(signature.curve(), curve);
        for invalid in [
            "",
            "3000",
            "3003020101",
            "300602010102010100",
            "3005020101020101",
            "30800201010201010000",
            "308106020101020101",
            "30820006020101020101",
            "3006020180020101",
            "300702020001020101",
            "3006020100020101",
            "30050200020101",
            "30060201ff020101",
            "3006030101020101",
            "3006020101220101",
            "3009020101020101020101",
            "3006020101020100",
            "3106020101020101",
            "3006028101020101",
            "30060201010281ff",
        ] {
            let error = Signature::from_der(curve, &hex(invalid)).unwrap_err();
            assert!(
                matches!(error.position, Position::Buffer(_)),
                "{invalid}: {error}"
            );
        }
        for length in 0..valid.len() {
            assert!(Signature::from_der(curve, &valid[..length]).is_err());
        }
        let oversized = vec![0; 140];
        assert_eq!(
            Signature::from_der(curve, &oversized).unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
        let key = PublicKey::from_sec1(curve, &generator_key(curve, false)).unwrap();
        assert_eq!(
            key.verify_digest(&[], &signature).unwrap_err().kind,
            pagura_core::ErrorKind::InvalidArgument
        );
        assert!(!key.verify_digest(&[1], &signature).unwrap());
        let other = if curve == Curve::P256 {
            Curve::P384
        } else {
            Curve::P256
        };
        assert!(
            key.verify_digest(&[1], &Signature::from_der(other, &valid).unwrap())
                .is_err()
        );
    }
}

#[test]
fn digest_truncation_is_leftmost_bits_not_modulo_or_byte_truncation() {
    assert_eq!(
        digest_integer(Curve::P256, &[255; 80]),
        uint(&"ff".repeat(32))
    );
    assert_eq!(
        digest_integer(Curve::P384, &[255; 80]),
        uint(&"ff".repeat(48))
    );
    assert_eq!(
        digest_integer(Curve::P521, &[255; 80]),
        Curve::P521.parameters().field.value
    );
    assert_eq!(
        digest_integer(Curve::P521, &[255; 65]),
        uint(&"ff".repeat(65))
    );
    assert_eq!(digest_integer(Curve::P521, &[1]), Uint::ONE);
}

#[test]
fn generator_key_verifies_its_own_public_equation() {
    // Q=G, e=1, r=Gx mod n, s=1+r satisfies the verification equation.
    for curve in [Curve::P256, Curve::P384, Curve::P521] {
        let parameters = curve.parameters();
        let r = parameters.scalar.reduce_once(parameters.gx);
        let s = parameters.scalar.add(r, Uint::ONE);
        let signature = Signature { curve, r, s };
        let key = PublicKey::from_sec1(curve, &generator_key(curve, true)).unwrap();
        assert!(key.verify_digest(&[1], &signature).unwrap());
    }
}
