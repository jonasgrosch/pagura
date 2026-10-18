use super::*;

#[test]
fn resolved_defaults_and_parameter_rejection() {
    let defaults = PssParameters::default();
    assert_eq!(defaults.hash(), HashAlgorithm::Sha1);
    assert_eq!(defaults.mgf_hash(), HashAlgorithm::Sha1);
    assert_eq!(defaults.salt_bytes(), 20);
    assert!(defaults.check_strength().is_err());
    for trailer in [0, 2, 0xbc, u64::MAX] {
        assert!(
            PssParameters::new(HashAlgorithm::Sha256, HashAlgorithm::Sha256, 32, trailer).is_err()
        );
    }
    assert!(
        PssParameters::new(HashAlgorithm::Sha256, HashAlgorithm::Sha256, usize::MAX, 1).is_err()
    );
    let mixed = PssParameters::new(HashAlgorithm::Sha256, HashAlgorithm::Sha1, 0, 1).unwrap();
    assert!(mixed.check_strength().is_err());
    let strong = PssParameters::new(HashAlgorithm::Sha256, HashAlgorithm::Sha256, 32, 1).unwrap();
    assert!(strong.check_strength().is_ok());
    assert!(strong.verify(&[0; 64], 511, &[0; 32]).is_err());
    assert_eq!(
        strong.verify(&[0; 128], 1023, &[0; 32]).unwrap(),
        Verification::Invalid
    );
    let mut high_bit = [0; 128];
    high_bit[0] = 0x80;
    high_bit[127] = 0xbc;
    assert_eq!(
        strong.verify(&high_bit, 1023, &[0; 32]).unwrap(),
        Verification::Invalid
    );
    assert_eq!(
        strong.verify(&[1; 129], 1024, &[0; 32]).unwrap(),
        Verification::Invalid
    );
}
