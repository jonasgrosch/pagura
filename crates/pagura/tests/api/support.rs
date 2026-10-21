//! Manifest-selected classic unsupported cases shared by transport tests.
use std::path::PathBuf;

#[expect(
    clippy::unwrap_used,
    reason = "generated test cases require path and feature fields"
)]
pub fn unsupported_cases() -> Vec<(PathBuf, String)> {
    // The independent generator writes one indented object per case.
    let cases: Vec<_> = include_str!("../../../../corpus/manifests/classic.json")
        .split("\n    {")
        .filter(|case| field(case, "expected") == Some("unsupported"))
        .map(|case| {
            let path = field(case, "path").unwrap();
            let feature = field(case, "feature").unwrap();
            (
                pagura_testkit::workspace_root().join(path),
                feature.to_owned(),
            )
        })
        .collect();
    assert!(
        !cases.is_empty(),
        "classic manifest needs an unsupported case"
    );
    cases
}

fn field<'a>(case: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("\"{name}\": \"");
    case.lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))?
        .split('"')
        .next()
}
