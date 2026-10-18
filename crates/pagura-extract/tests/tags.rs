#![forbid(unsafe_code)]
//! Tag discipline (CONTRIBUTING 3): every page-text tag is unique and in
//! the `text.` area.
use pagura_extract::{features, repairs};

#[test]
fn tags_are_unique_and_in_the_text_area() {
    let tags: Vec<&str> = features::ALL
        .iter()
        .map(|tag| tag.as_str())
        .chain(repairs::ALL.iter().map(|tag| tag.as_str()))
        .collect();
    for (index, tag) in tags.iter().enumerate() {
        let item = tag.strip_prefix("text.").unwrap_or_default();
        let well_formed =
            !item.is_empty() && item.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
        assert!(well_formed, "malformed tag {tag}");
        assert!(!tags[..index].contains(tag), "duplicate tag {tag}");
    }
}
