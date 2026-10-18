use super::*;
use crate::tests::decode;
use pagura_core::ErrorKind;
use pagura_testkit::fixture;

/// A hex-encoded UTF-8 fixture field; `-` is the empty string.
fn text(encoded: &str) -> String {
    String::from_utf8(decode(encoded)).unwrap()
}

#[test]
fn every_table_membership_matches_independent_unicode32_stringprep() {
    let data = fixture("crypto", "saslprep-table-membership.ranges");
    let expected: Vec<_> = std::str::from_utf8(&data)
        .unwrap()
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (
                u32::from_str_radix(fields[0], 16).unwrap(),
                u32::from_str_radix(fields[1], 16).unwrap(),
                u16::from_str_radix(fields[2], 16).unwrap(),
            )
        })
        .collect();
    assert_eq!(tables::RANGES, expected);
    // Includes surrogate and unassigned points that cannot occur in Rust str.
    // Also exercise lookup through every gap in the compressed table.
    let mut index = 0;
    for code in 0..=0x0010_ffff {
        while index < expected.len() && code > expected[index].1 {
            index += 1;
        }
        let flags = expected
            .get(index)
            .filter(|&&(start, _, _)| start <= code)
            .map_or(0, |&(_, _, flags)| flags);
        assert_eq!(membership(code), flags, "U+{code:04X}");
    }
    assert_eq!(membership(u32::MAX), 0);
}

#[test]
fn mapping_matches_all_b1_and_c12_points_without_case_folding() {
    let data = fixture("crypto", "saslprep-mapping.cases");
    for (index, line) in std::str::from_utf8(&data).unwrap().lines().enumerate() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let input = text(fields[0]);
        assert_eq!(map(&input).unwrap(), text(fields[1]), "case {index}");
    }
    assert!(matches!(map("USER").unwrap(), Cow::Borrowed(_)));
}

#[test]
fn output_validator_matches_prohibitions_unassigned_and_bidi_oracles() {
    let data = fixture("crypto", "saslprep-output-checks.cases");
    for (index, line) in std::str::from_utf8(&data).unwrap().lines().enumerate() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let input = text(fields[0]);
        let result = validate_output(&input);
        if fields[1] == "ok" {
            assert!(result.is_ok(), "case {index}");
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.kind, ErrorKind::Password, "case {index}");
            assert!(matches!(error.position, Position::Buffer(_)));
            assert_eq!(
                error.message,
                match fields[1] {
                    "prohibited" => "password contains a SASLprep-prohibited code point",
                    "unassigned" => "password contains an unassigned Unicode 3.2 code point",
                    "bidi" => "password violates the SASLprep bidirectional rule",
                    _ => unreachable!(),
                },
                "case {index}"
            );
        }
    }
}

#[test]
fn preparation_uses_mapping_then_explicit_normalization_then_validation() {
    let data = fixture("crypto", "saslprep-supported-subset.cases");
    for (index, line) in std::str::from_utf8(&data).unwrap().lines().enumerate() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let input = text(fields[0]);
        let result = prepare(&input);
        match fields[1] {
            "ok" => assert_eq!(result.unwrap(), text(fields[2]), "case {index}"),
            "prohibited" | "unassigned" | "bidi" => {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ErrorKind::Password, "case {index}");
            }
            _ => unreachable!(),
        }
    }
    assert!(matches!(prepare("ASCII").unwrap(), Cow::Borrowed(_)));
    // These two literal cases establish byte positions without echoing input.
    assert_eq!(prepare("ab\0").unwrap_err().position, Position::Buffer(2));
    assert_eq!(
        validate_output("a\u{0221}").unwrap_err().position,
        Position::Buffer(1)
    );
}

#[test]
fn generated_passwords_match_complete_independent_profile() {
    let data = fixture("crypto", "saslprep-generated-passwords.cases");
    let mut counts = [0; 4];
    for (index, line) in std::str::from_utf8(&data).unwrap().lines().enumerate() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let input = text(fields[0]);
        let result = prepare(&input);
        if fields[1] == "ok" {
            assert_eq!(result.unwrap(), text(fields[2]), "case {index}");
            counts[0] += 1;
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.kind, ErrorKind::Password, "case {index}");
            let (slot, message) = match fields[1] {
                "prohibited" => (1, "password contains a SASLprep-prohibited code point"),
                "unassigned" => (2, "password contains an unassigned Unicode 3.2 code point"),
                "bidi" => (3, "password violates the SASLprep bidirectional rule"),
                _ => unreachable!(),
            };
            assert_eq!(error.message, message, "case {index}");
            counts[slot] += 1;
        }
    }
    assert!(counts.iter().all(|&count| count > 0));
    println!("Independent profile cases (ok/prohibited/unassigned/bidi): {counts:?}");
}
