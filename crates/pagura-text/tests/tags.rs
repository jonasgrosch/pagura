#![forbid(unsafe_code)]
//! Tag discipline (CONTRIBUTING 3): every font tag is unique and in the
//! `font.` area.
use pagura_text::{features, information, repairs};

#[test]
fn tags_are_unique_and_in_the_font_area() {
    let tags: Vec<&str> = features::ALL
        .iter()
        .map(|tag| tag.as_str())
        .chain(repairs::ALL.iter().map(|tag| tag.as_str()))
        .chain(information::ALL.iter().copied())
        .collect();
    for (index, tag) in tags.iter().enumerate() {
        let item = tag.strip_prefix("font.").unwrap_or_default();
        let well_formed = !item.is_empty()
            && item
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        assert!(well_formed, "malformed tag {tag}");
        assert!(!tags[..index].contains(tag), "duplicate tag {tag}");
    }
}
