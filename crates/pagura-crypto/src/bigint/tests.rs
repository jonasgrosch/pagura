use super::*;
use crate::tests::decode;
use pagura_testkit::fixture_text;

#[test]
fn independent_python_integer_oracles() {
    let data = fixture_text("crypto", "modular-power.vectors");
    let mut count = 0;
    for line in data.lines() {
        let fields: Vec<_> = line.split('|').collect();
        let [n, base, exponent, expected] = fields.as_slice() else {
            unreachable!()
        };
        let modulus = Modulus::new(&decode(n), MAX_MODULUS_BITS).unwrap();
        let actual = modulus
            .pow(&decode(base), &decode(exponent), 512, 64_000_000)
            .unwrap();
        assert_eq!(
            actual,
            decode(expected),
            "n={n}, base={base}, exponent={exponent}"
        );
        count += 1;
    }
    assert_eq!(count, 810);
    eprintln!("Python modular exponentiation: {count} cases passed");
}

#[test]
fn rejects_bad_moduli_and_operands_before_work() {
    assert_eq!(
        Modulus::new(&[0, 3], 64).unwrap_err().position,
        Position::Buffer(0)
    );
    assert_eq!(
        Modulus::new(&[0xff; 9], 64).unwrap_err().position,
        Position::Buffer(8)
    );
    for n in [&[][..], &[0], &[0, 3], &[1], &[2], &[4]] {
        assert!(Modulus::new(n, MAX_MODULUS_BITS).is_err());
    }
    assert!(Modulus::new(&[17], 4).is_err());
    assert!(Modulus::new(&vec![0xff; MAX_MODULUS_BITS / 8 + 1], usize::MAX).is_err());
    let modulus = Modulus::new(&[17], 5).unwrap();
    assert!(modulus.pow(&[17], &[3], 2, 100).is_err());
    assert!(modulus.pow(&[0, 1], &[3], 2, 100).is_err());
    assert!(modulus.pow(&[2], &[0], 2, 100).is_err());
    assert!(modulus.pow(&[2], &[0, 3], 16, 100).is_err());
    assert!(modulus.pow(&[2], &[3], 1, 100).is_err());
    assert!(modulus.pow(&[2], &[3], 2, 11).is_err());
    assert_eq!(modulus.pow(&[2], &[3], 2, 12).unwrap(), [8]);
    assert_eq!(modulus.pow(&[], &[], 0, 0).unwrap(), [1]);
}
