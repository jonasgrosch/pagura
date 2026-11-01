#![forbid(unsafe_code)]
//! Tag discipline (CONTRIBUTING 3): every tag of this crate is unique, well
//! formed and in its `appearance.`, `annotation.` or `form.` area.
use pagura_appearance::{features, information};

#[test]
fn tags_are_unique_and_in_the_appearance_areas() {
    let tags: Vec<&str> = features::ALL
        .iter()
        .map(|tag| tag.as_str())
        .chain(information::ALL.iter().copied())
        .collect();
    for (index, tag) in tags.iter().enumerate() {
        let item = ["appearance.", "annotation.", "form."]
            .iter()
            .find_map(|area| tag.strip_prefix(area))
            .unwrap_or_default();
        let well_formed = !item.is_empty()
            && item
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        assert!(well_formed, "malformed tag {tag}");
        assert!(!tags[..index].contains(tag), "duplicate tag {tag}");
    }
    assert!(
        features::ALL
            .iter()
            .any(|tag| tag.as_str() == "annotation.appearance")
    );
}
