use super::*;
use pagura_testkit::{fixture, workspace_root};

fn scalars(field: &str) -> String {
    field
        .split_whitespace()
        .map(|code| char::from_u32(u32::from_str_radix(code, 16).unwrap()).unwrap())
        .collect()
}

#[test]
fn every_official_unicode32_nfkc_relation() {
    // Test data only: the official corpus is never linked into the product.
    let corpus =
        std::fs::read_to_string(workspace_root().join("data/unicode/NormalizationTest-3.2.0.txt"))
            .unwrap();
    let mut counts = [0; 4];
    let mut part = None;
    let mut relations = 0;
    for (line_number, line) in corpus.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if let Some(value) = line.strip_prefix("@Part") {
            part = Some(value.parse::<usize>().unwrap());
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let part = part.unwrap();
        counts[part] += 1;
        let columns: Vec<_> = line.split(';').take(5).map(scalars).collect();
        assert_eq!(columns.len(), 5);
        for (column, input) in columns.iter().enumerate() {
            let normalized = nfkc32(Cow::Borrowed(input)).unwrap();
            assert_eq!(
                normalized,
                columns[3],
                "part {part}, line {}, column {}",
                line_number + 1,
                column + 1
            );
            assert_eq!(nfkc32(normalized).unwrap(), columns[3]);
            relations += 1;
        }
    }
    // This published version contains Parts 0–2; there is no Part 3.
    assert_eq!(counts, [23, 16_315, 654, 0]);
    assert_eq!(relations, 84_960);
}

#[test]
fn staged_independent_normalization_oracles_and_utf8_byte_truncation() {
    for suffix in [
        "feminine-ordinal",
        "roman-nine",
        "already-normalized",
        "angstrom",
        "canonical-reorder",
        "hangul-composition",
        "unicode32-correction",
        "expansion-before-truncation",
        "utf8-byte-truncation",
        "normalization-before-prohibition",
    ] {
        let name = format!("saslprep-nfkc-{suffix}");
        let input = String::from_utf8(fixture("crypto", &format!("{name}.input"))).unwrap();
        let expected = fixture("crypto", &format!("{name}.normalized"));
        let truncated = fixture("crypto", &format!("{name}.truncated"));
        let result = super::super::prepare(&input).unwrap();
        assert_eq!(result.as_bytes(), expected, "{name}");
        assert_eq!(&result.as_bytes()[..result.len().min(127)], truncated);
    }
}

#[test]
fn corrections_use_the_stringprep_version_instead_of_current_unicode() {
    let data = fixture("crypto", "nfkc32-corrections.cases");
    for line in std::str::from_utf8(&data).unwrap().lines() {
        let (input, expected) = line.split_once(' ').unwrap();
        assert_eq!(
            nfkc32(Cow::Owned(scalars(input))).unwrap(),
            scalars(expected)
        );
    }
}

#[test]
fn unchanged_inputs_preserve_ownership_and_notice_is_available() {
    for input in ["ASCII", "café", "\u{4e00}", "\u{1f600}", "\u{0301}"] {
        assert!(matches!(
            nfkc32(Cow::Borrowed(input)).unwrap(),
            Cow::Borrowed(_)
        ));
    }
    assert!(matches!(
        nfkc32(Cow::Owned("café".to_owned())).unwrap(),
        Cow::Owned(_)
    ));
    let notice = std::fs::read_to_string(workspace_root().join("data/unicode/LICENSE")).unwrap();
    assert_eq!(crate::unicode_notice(), notice);
}

// `tools/check.py` runs `tools/gen/unicode32.py --check` for reproducibility.
#[test]
fn generated_tables_remain_bounded() {
    let bytes = size_of_val(tables::DECOMPOSITIONS)
        + tables::TEXT.len()
        + size_of_val(tables::COMBINING)
        + size_of_val(tables::COMPOSITIONS)
        + size_of_val(tables::QUICK_CHECK);
    assert!(bytes < 100_000, "normalization table bytes: {bytes}");
    // Counts and payload sizes make unexpected table growth reviewable.
    println!(
        "NFKC tables: {} decompositions, {} pool bytes, {} combining ranges, {} compositions, {} quick-check ranges; {bytes} payload bytes",
        tables::DECOMPOSITIONS.len(),
        tables::TEXT.len(),
        tables::COMBINING.len(),
        tables::COMPOSITIONS.len(),
        tables::QUICK_CHECK.len()
    );
}
