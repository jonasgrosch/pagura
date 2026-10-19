//! Per-case tolerances from the fixture manifests (ADR 0004).

use pagura_testkit::{Json, Manifest};

/// Cases whose oracle samples coverage densely instead of integrating it
/// exactly. ADR 0004 records them as the only cases above one level.
const SAMPLED_ORACLES: &[&str] = &[
    "content/adjust-arch",
    "content/annotation-icon-Comment",
    "content/annotation-icon-Help",
    "content/annotation-icon-Insert",
    "content/annotation-icon-Key",
    "content/annotation-icon-NewParagraph",
    "content/annotation-icon-Note",
    "content/annotation-icon-Paragraph",
    "content/stroke-closed-duplicates",
    "content/stroke-closed-point",
];

/// The per-channel tolerance `manifest` records for `name`: none unless the
/// case lists one, at most one level unless ADR 0004 names the case.
#[expect(
    clippy::panic,
    clippy::unwrap_used,
    reason = "a missing or invalid manifest entry fails the test"
)]
pub(crate) fn tolerance(manifest: &Manifest, name: &str) -> u8 {
    let case = manifest
        .case(name)
        .unwrap_or_else(|| panic!("{name} is not in the {} manifest", manifest.area()));
    let tolerance = case
        .get("tolerance")
        .map_or(Some(0), Json::as_u64)
        .unwrap_or_else(|| panic!("{}: tolerance must be an integer", case.id()));
    assert!(
        tolerance <= 1 || SAMPLED_ORACLES.contains(&case.id()),
        "{}: tolerance {tolerance} exceeds the ADR 0004 contract",
        case.id()
    );
    u8::try_from(tolerance).unwrap()
}

/// Asserts that every channel of an RGB `actual` is within `tolerance` of
/// `expected`.
pub(crate) fn assert_close(name: &str, actual: &[u8], expected: &[u8], tolerance: u8) {
    assert_eq!(actual.len(), expected.len(), "{name}");
    for (index, (&a, &e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            a.abs_diff(e) <= tolerance,
            "{name} byte {index}: {a} vs {e} (tolerance {tolerance})"
        );
    }
}

#[test]
fn sampled_oracles_are_exactly_the_cases_above_one_level() {
    let mut above: Vec<_> = ["content", "paint"]
        .into_iter()
        .flat_map(|area| {
            let manifest = Manifest::load(area);
            manifest
                .cases()
                .filter(|case| {
                    case.get("tolerance")
                        .and_then(Json::as_u64)
                        .is_some_and(|t| t > 1)
                })
                .map(|case| case.id().to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    above.sort();
    assert_eq!(above, SAMPLED_ORACLES);
}
